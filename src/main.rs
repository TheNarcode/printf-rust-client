#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{ApiOrder, Config, JobInfo, PrintAttributes};
use ftail::Ftail;
use log::LevelFilter;
use redis::AsyncCommands;
use tokio::sync::Mutex;

pub mod ipp;
pub mod types;

const BASE_URL: &str = "https://printfs.thenarcode.workers.dev";

pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: Arc<reqwest::Client>,
    pub is_running: Arc<AtomicBool>,
    pub cancel_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub printer_manager: Arc<Mutex<Option<PrinterManager>>>,
}

fn current_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

async fn update_job_status(
    redis_client: &redis::Client,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
) {
    let job_info = JobInfo {
        file_id: attributes.file_id.clone(),
        order_id,
        attributes: attributes.clone(),
        status: status.to_string(),
        updated_at: current_timestamp(),
    };

    match serde_json::to_string(&job_info) {
        Ok(json) => match redis_client.get_multiplexed_async_connection().await {
            Ok(mut con) => {
                match con
                    .hset::<_, _, _, ()>("printf_jobs_status", &attributes.file_id, json)
                    .await
                {
                    Ok(_) => log::info!(
                        "updated job status for {} to {}",
                        attributes.file_id,
                        status
                    ),
                    Err(e) => log::error!("failed to update job status in redis: {}", e),
                }
            }
            Err(e) => log::error!("failed to connect to redis for status update: {}", e),
        },
        Err(e) => log::error!("failed to serialize job info: {}", e),
    }
}

#[tauri::command]
async fn start_client(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    if state.is_running.load(Ordering::SeqCst) {
        return Ok("Client is already running".to_string());
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    *state.cancel_tx.lock().await = Some(tx);
    state.is_running.store(true, Ordering::SeqCst);

    let config = Arc::clone(&state.config);
    let http_client = Arc::clone(&state.http_client);
    let is_running_flag = Arc::clone(&state.is_running);

    log::info!("starting printf background client loop");

    let pm = Arc::clone(&state.printer_manager);

    tauri::async_runtime::spawn(async move {
        // Initialize or refresh the printer manager
        match get_ipp_printers().await {
            Ok(printers) => {
                let mut pm_lock = pm.lock().await;
                if pm_lock.is_none() {
                    *pm_lock = Some(PrinterManager::new(printers));
                } else {
                    // Refresh printer list but preserve paused state
                    let existing = pm_lock.take().unwrap();
                    let paused_uris: Vec<String> = existing
                        .get_printers()
                        .iter()
                        .filter(|p| p.paused)
                        .map(|p| p.uri.clone())
                        .collect();
                    let mut new_pm = PrinterManager::new(printers);
                    for uri in &paused_uris {
                        new_pm.set_printer_paused(uri, true);
                    }
                    *pm_lock = Some(new_pm);
                }
                log::info!("printer manager initialized in background task");
            }
            Err(e) => {
                log::error!("failed to get ipp printers: {}", e);
                is_running_flag.store(false, Ordering::SeqCst);
                return;
            }
        }

        let redis_client = match redis::Client::open(config.redis_url.as_str()) {
            Ok(c) => c,
            Err(e) => {
                log::error!("failed to create redis client: {}", e);
                is_running_flag.store(false, Ordering::SeqCst);
                return;
            }
        };

        let mut reconnect_delay = Duration::from_secs(5);
        let mut first_connect = true;

        loop {
            if !is_running_flag.load(Ordering::SeqCst) {
                log::info!("client stop requested before redis connect");
                break;
            }

            let mut con = match redis_client.get_multiplexed_async_connection().await {
                Ok(mut c) => {
                    if first_connect {
                        log::info!("connected to redis, clearing old job status hash");
                        let _: Result<(), _> = c.del("printf_jobs_status").await;
                    } else {
                        log::info!("reconnected to redis");
                    }
                    first_connect = false;
                    reconnect_delay = Duration::from_secs(5);
                    c
                }
                Err(e) => {
                    log::error!("failed to connect to redis: {}", e);
                    tokio::select! {
                        _ = tokio::time::sleep(reconnect_delay) => {}
                        _ = &mut rx => {
                            log::info!("client loop cancelled during reconnect sleep");
                            is_running_flag.store(false, Ordering::SeqCst);
                            break;
                        }
                    }
                    reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(60));
                    continue;
                }
            };

            loop {
                if !is_running_flag.load(Ordering::SeqCst) {
                    log::info!("client stop requested in queue loop");
                    break;
                }

                tokio::select! {
                    _ = &mut rx => {
                        log::info!("client loop cancelled via channel");
                        is_running_flag.store(false, Ordering::SeqCst);
                        break;
                    }
                    res = con.brpop::<_, Option<(String, String)>>("printf_queue", 1.0) => {
                        match res {
                            Ok(Some((_key, data))) => {
                                log::info!("got new print command from queue");

                                let attributes_list: Vec<PrintAttributes> = match serde_json::from_str(&data) {
                                    Ok(list) => list,
                                    Err(err) => {
                                        log::error!("failed to parse print attributes: {}", err);
                                        continue;
                                    }
                                };

                                let has_color = attributes_list.iter().any(|a| a.color == crate::types::ColorMode::Color);
                                let has_mono = attributes_list.iter().any(|a| a.color == crate::types::ColorMode::Monochrome);

                                let (color_printer, mono_printer, color_media_source, mono_media_source) = {
                                    let mut pm_guard = pm.lock().await;
                                    let pm_ref = pm_guard.as_mut().unwrap();
                                    pm_ref.get_printers_for_order(has_color, has_mono)
                                };

                                // Extract order_id from the first attribute (since they share the same order)
                                let order_id: Option<String> = attributes_list.first().and_then(|a| a.order.clone());

                                for mut attributes in attributes_list {
                                    let is_color = attributes.color == crate::types::ColorMode::Color;
                                    let printer = if let Some(target) = &attributes.target_printer {
                                        crate::types::Printer {
                                            uri: target.clone(),
                                            name: target.clone(),
                                            color_mode: attributes.color.clone(),
                                            paused: false,
                                        }
                                    } else {
                                        match attributes.color {
                                            crate::types::ColorMode::Color => match &color_printer {
                                                Some(p) => p.clone(),
                                                None => {
                                                    log::error!("no color printer found for order");
                                                    update_job_status(&redis_client, &attributes, order_id.clone(), "Failed").await;
                                                    continue;
                                                }
                                            },
                                            crate::types::ColorMode::Monochrome => match &mono_printer {
                                                Some(p) => p.clone(),
                                                None => {
                                                    log::error!("no monochrome printer found for order");
                                                    update_job_status(&redis_client, &attributes, order_id.clone(), "Failed").await;
                                                    continue;
                                                }
                                            },
                                        }
                                    };

                                    let config = Arc::clone(&config);
                                    let http_client = Arc::clone(&http_client);
                                    let redis_client = redis_client.clone();
                                    let media_source = if is_color {
                                        color_media_source.clone()
                                    } else {
                                        mono_media_source.clone()
                                    };
                                    let order_id_cloned = order_id.clone();

                                    attributes.target_printer = Some(printer.name.clone());

                                    update_job_status(&redis_client, &attributes, order_id_cloned.clone(), "Processing").await;

                                    tokio::spawn(async move {
                                        log::info!("using printer {} ({}) for print", printer.name, printer.uri);

                                        let failed = match printer.uri.parse() {
                                            Ok(uri) => match print_job(uri, printer.name.clone(), attributes.clone(), media_source, config, http_client).await {
                                                Ok(_) => { log::info!("print job successful"); None }
                                                Err(e) => {
                                                    let err_str = e.to_string();
                                                    log::error!("print job failed: {}", err_str);
                                                    Some(err_str)
                                                }
                                            },
                                            Err(e) => {
                                                let err_str = e.to_string();
                                                log::error!("failed to parse printer URI: {}", err_str);
                                                Some(err_str)
                                            }
                                        };

                                        if let Some(err_msg) = failed {
                                            if err_msg.contains("PendingTimeout") {
                                                update_job_status(&redis_client, &attributes, order_id_cloned.clone(), "Stuck").await;
                                            } else {
                                                update_job_status(&redis_client, &attributes, order_id_cloned.clone(), "Failed").await;
                                            }
                                        } else {
                                            update_job_status(&redis_client, &attributes, order_id_cloned.clone(), "Completed").await;
                                        }
                                    });
                                }
                            }
                            Ok(None) => {
                                // Timeout expired, loop around to check is_running_flag / rx
                            }
                            Err(e) => {
                                log::error!("redis connection lost: {}", e);
                                break;
                            }
                        }
                    }
                }
            }

            if !is_running_flag.load(Ordering::SeqCst) {
                break;
            }

            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(60));
        }

        log::info!("printf background client loop stopped");
        is_running_flag.store(false, Ordering::SeqCst);
    });

    Ok("Client started".to_string())
}

#[tauri::command]
async fn stop_client(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Ok("Client is not running".to_string());
    }

    state.is_running.store(false, Ordering::SeqCst);
    if let Some(tx) = state.cancel_tx.lock().await.take() {
        let _ = tx.send(());
    }

    log::info!("sent stop signal to client loop");
    Ok("Client stopped".to_string())
}

#[tauri::command]
async fn get_client_status(state: tauri::State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.is_running.load(Ordering::SeqCst))
}

#[tauri::command]
async fn get_jobs(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<JobInfo>, String> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }

    let redis_client = redis::Client::open(state.config.redis_url.as_str())
        .map_err(|e| format!("Failed to create redis client: {}", e))?;

    let mut con = redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("Failed to connect to redis: {}", e))?;

    let queue_items: Vec<String> = con
        .lrange("printf_queue", 0, -1)
        .await
        .map_err(|e| format!("Failed to fetch printf_queue: {}", e))?;

    let mut jobs: Vec<JobInfo> = Vec::new();

    for item in queue_items {
        if let Ok(attrs_list) = serde_json::from_str::<Vec<PrintAttributes>>(&item) {
            for attrs in attrs_list {
                jobs.push(JobInfo {
                    file_id: attrs.file_id.clone(),
                    order_id: attrs.order.clone(),
                    attributes: attrs,
                    status: "Queued".to_string(),
                    updated_at: current_timestamp(),
                });
            }
        }
    }

    let status_items: redis::Value = con
        .hgetall("printf_jobs_status")
        .await
        .map_err(|e| format!("Failed to fetch printf_jobs_status: {}", e))?;

    if let redis::Value::Bulk(items) = status_items {
        let mut i = 1;
        while i < items.len() {
            if let redis::Value::Data(ref data) = items[i] {
                if let Ok(json_str) = std::str::from_utf8(data) {
                    if let Ok(job_info) = serde_json::from_str::<JobInfo>(json_str) {
                        if !jobs.iter().any(|j| j.file_id == job_info.file_id) {
                            jobs.push(job_info);
                        }
                    }
                }
            }
            i += 2;
        }
    }

    jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    Ok(jobs)
}

#[tauri::command]
async fn get_orders(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<ApiOrder>, String> {
    let url = format!("{}/client/orders", BASE_URL);
    let mut req = state.http_client.get(&url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("Failed to fetch orders: {}", e))?;
    let orders = resp.json::<Vec<ApiOrder>>().await.map_err(|e| format!("Failed to parse orders: {}", e))?;
    Ok(orders)
}

#[tauri::command]
async fn get_printer_list(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<crate::types::Printer>, String> {
    let pm_lock = state.printer_manager.lock().await;
    if let Some(ref pm) = *pm_lock {
        Ok(pm.get_printers())
    } else {
        // Manager not initialized yet, query fresh
        drop(pm_lock);
        crate::ipp::get_ipp_printers().await.map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn pause_printer(uri: String, state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        pm.set_printer_paused(&uri, true);
        log::info!("Paused printer: {}", uri);
        Ok(())
    } else {
        Err("Printer manager not initialized".to_string())
    }
}

#[tauri::command]
async fn unpause_printer(uri: String, state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        pm.set_printer_paused(&uri, false);
        log::info!("Unpaused printer: {}", uri);
        Ok(())
    } else {
        Err("Printer manager not initialized".to_string())
    }
}

#[tauri::command]
async fn reprint_job(
    file_id: String,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let redis_client = redis::Client::open(state.config.redis_url.as_str())
        .map_err(|e| format!("Failed to create redis client: {}", e))?;

    let mut con = redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("Failed to connect to redis: {}", e))?;

    let data: Option<String> = con
        .hget("printf_jobs_status", &file_id)
        .await
        .map_err(|e| format!("Failed to fetch job info: {}", e))?;

    if let Some(json_str) = data {
        let job_info: JobInfo = serde_json::from_str(&json_str)
            .map_err(|e| format!("Failed to parse job info: {}", e))?;

        let payload = serde_json::to_string(&[&job_info.attributes])
            .map_err(|e| format!("Failed to serialize attributes: {}", e))?;

        con.lpush::<_, _, ()>("printf_queue", payload)
            .await
            .map_err(|e| format!("Failed to lpush printf_queue: {}", e))?;

        update_job_status(&redis_client, &job_info.attributes, job_info.order_id.clone(), "Queued").await;
        log::info!("re-queued job {} for reprint", file_id);
        Ok(())
    } else {
        Err(format!("Job {} not found in status hash", file_id))
    }
}

#[tauri::command]
async fn minimize_window(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
async fn maximize_window(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().map_err(|e| e.to_string())? {
        window.unmaximize().map_err(|e| e.to_string())
    } else {
        window.maximize().map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn close_window(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_available_printers() -> Result<Vec<crate::types::Printer>, String> {
    crate::ipp::get_ipp_printers()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn requeue_to_printer(
    file_id: String,
    printer_uri: String,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let redis_client = redis::Client::open(state.config.redis_url.as_str())
        .map_err(|e| format!("Failed to create redis client: {}", e))?;

    let mut con = redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| format!("Failed to connect to redis: {}", e))?;

    let data: Option<String> = con
        .hget("printf_jobs_status", &file_id)
        .await
        .map_err(|e| format!("Failed to fetch job info: {}", e))?;

    if let Some(json_str) = data {
        let mut job_info: JobInfo = serde_json::from_str(&json_str)
            .map_err(|e| format!("Failed to parse job info: {}", e))?;

        job_info.attributes.target_printer = Some(printer_uri);

        let payload = serde_json::to_string(&vec![job_info.attributes.clone()])
            .map_err(|e| format!("Failed to serialize attributes: {}", e))?;

        con.lpush::<_, _, ()>("printf_queue", payload)
            .await
            .map_err(|e| format!("Failed to lpush printf_queue: {}", e))?;

        update_job_status(&redis_client, &job_info.attributes, job_info.order_id.clone(), "Queued").await;
        log::info!("re-queued stuck job {} to new printer", file_id);
        Ok(())
    } else {
        Err(format!("Job {} not found in status hash", file_id))
    }
}

#[tauri::command]
async fn get_stats(
    month: Option<String>,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let mut url = format!("{}/client/stats", BASE_URL);
    if let Some(m) = month {
        if !m.is_empty() {
            url = format!("{}/client/stats?month={}", BASE_URL, m);
        }
    }

    let mut req = state.http_client.get(url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    match req.send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => {
                println!("{}", text);
                Ok(text)
            }
            Err(e) => Err(format!("Failed to read stats text: {}", e)),
        },
        Err(e) => Err(format!("Failed to fetch stats: {}", e)),
    }
}

#[tauri::command]
async fn get_completed_orders(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    let url = format!("{}/client/completed", BASE_URL);
    let mut req = state.http_client.get(url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    match req.send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => Ok(text),
            Err(e) => Err(format!("Failed to read response text: {}", e)),
        },
        Err(e) => Err(format!("Failed to fetch completed orders: {}", e)),
    }
}

#[tauri::command]
async fn mark_order_collected(
    order_id: String,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let url = format!("{}/client/collect", BASE_URL);
    let payload = serde_json::json!({ "orderId": order_id });
    let mut req = state.http_client.post(url).json(&payload);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    match req.send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => Ok(text),
            Err(e) => Err(format!("Failed to read response text: {}", e)),
        },
        Err(e) => Err(format!("Failed to mark order as collected: {}", e)),
    }
}

fn main() {
    let logs_dir = dirs::data_local_dir().unwrap().join("printf").join("logs");
    let config_path = get_config_path().expect("failed to get config path");

    fs::create_dir_all(&logs_dir).expect("failed to create logs dir");
    fs::create_dir_all(config_path.parent().unwrap()).expect("failed to create config dir");

    Ftail::new()
        .console(LevelFilter::Info)
        .daily_file(&logs_dir, LevelFilter::Info)
        .init()
        .expect("failed to initialize ftail");

    log::info!("printf tauri client started");

    let config = match read_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log::warn!("Failed to read config, using fallback: {}", e);
            Arc::new(Config {
                redis_url: "redis://127.0.0.1:6379".to_string(),
                s3_base_url: "http://localhost:8000/".to_string(),
                webhook_url: None,
                printf_key: None,
                base_url: BASE_URL.to_string(),
            })
        }
    };

    let http_client = Arc::new(reqwest::Client::new());

    let app_state = Arc::new(AppState {
        config,
        http_client,
        is_running: Arc::new(AtomicBool::new(false)),
        cancel_tx: Mutex::new(None),
        printer_manager: Arc::new(Mutex::new(None)),
    });

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_client,
            stop_client,
            get_client_status,
            get_jobs,
            get_orders,
            reprint_job,
            minimize_window,
            maximize_window,
            close_window,
            get_stats,
            get_completed_orders,
            mark_order_collected,
            get_available_printers,
            get_printer_list,
            pause_printer,
            unpause_printer,
            requeue_to_printer
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn read_config() -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let config_file = get_config_path()?;
    let file = File::open(&config_file)?;
    Ok(serde_json::from_reader(file)?)
}

pub fn get_config_path() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let config_file = dirs::config_local_dir()
        .unwrap()
        .join("printf")
        .join("config.json");

    Ok(config_file)
}
