use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{Config, PrintAttributes, JobInfo};
use ftail::Ftail;
use log::LevelFilter;
use redis::AsyncCommands;
use tokio::sync::Mutex;

pub mod ipp;
pub mod types;

pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: Arc<reqwest::Client>,
    pub is_running: Arc<AtomicBool>,
    pub cancel_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

fn current_timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    now.as_secs().to_string()
}

async fn update_job_status(redis_client: &redis::Client, attributes: &PrintAttributes, status: &str) {
    let job_info = JobInfo {
        file_id: attributes.file_id.clone(),
        attributes: attributes.clone(),
        status: status.to_string(),
        updated_at: current_timestamp(),
    };

    match serde_json::to_string(&job_info) {
        Ok(json) => match redis_client.get_multiplexed_async_connection().await {
            Ok(mut con) => {
                match con.hset::<_, _, _, ()>("printf_jobs_status", &attributes.file_id, json).await {
                    Ok(_) => log::info!("updated job status for {} to {}", attributes.file_id, status),
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

    tauri::async_runtime::spawn(async move {
        let printers = match get_ipp_printers().await {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to get ipp printers: {}", e);
                is_running_flag.store(false, Ordering::SeqCst);
                return;
            }
        };

        let pm = Arc::new(Mutex::new(PrinterManager::new(printers)));
        log::info!("printer manager initialized in background task");

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

                                let (color_printer, mono_printer, media_source) = {
                                    let mut pm_guard = pm.lock().await;
                                    pm_guard.get_printers_for_order(has_color, has_mono)
                                };

                                for attributes in attributes_list {
                                    let printer = match attributes.color {
                                        crate::types::ColorMode::Color => match &color_printer {
                                            Some(p) => p.clone(),
                                            None => {
                                                log::error!("no color printer found for order");
                                                update_job_status(&redis_client, &attributes, "Failed").await;
                                                continue;
                                            }
                                        },
                                        crate::types::ColorMode::Monochrome => match &mono_printer {
                                            Some(p) => p.clone(),
                                            None => {
                                                log::error!("no monochrome printer found for order");
                                                update_job_status(&redis_client, &attributes, "Failed").await;
                                                continue;
                                            }
                                        },
                                    };

                                    let config = Arc::clone(&config);
                                    let http_client = Arc::clone(&http_client);
                                    let redis_client = redis_client.clone();
                                    let media_source = media_source.clone();

                                    update_job_status(&redis_client, &attributes, "Processing").await;

                                    tokio::spawn(async move {
                                        log::info!("using printer {} ({}) for print", printer.name, printer.uri);

                                        let failed = match printer.uri.parse() {
                                            Ok(uri) => match print_job(uri, printer.name.clone(), attributes.clone(), media_source, config, http_client).await {
                                                Ok(_) => { log::info!("print job successful"); false }
                                                Err(e) => { log::error!("print job failed: {}", e); true }
                                            },
                                            Err(e) => { log::error!("failed to parse printer URI: {}", e); true }
                                        };

                                        if failed {
                                            update_job_status(&redis_client, &attributes, "Failed").await;
                                        } else {
                                            update_job_status(&redis_client, &attributes, "Completed").await;
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

    let mut con = redis_client.get_multiplexed_async_connection().await
        .map_err(|e| format!("Failed to connect to redis: {}", e))?;

    let queue_items: Vec<String> = con.lrange("printf_queue", 0, -1).await
        .map_err(|e| format!("Failed to fetch printf_queue: {}", e))?;

    let mut jobs: Vec<JobInfo> = Vec::new();

    for item in queue_items {
        if let Ok(attrs_list) = serde_json::from_str::<Vec<PrintAttributes>>(&item) {
            for attrs in attrs_list {
                jobs.push(JobInfo {
                    file_id: attrs.file_id.clone(),
                    attributes: attrs,
                    status: "Queued".to_string(),
                    updated_at: current_timestamp(),
                });
            }
        }
    }

    let status_items: redis::Value = con.hgetall("printf_jobs_status").await
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
async fn reprint_job(file_id: String, state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    let redis_client = redis::Client::open(state.config.redis_url.as_str())
        .map_err(|e| format!("Failed to create redis client: {}", e))?;

    let mut con = redis_client.get_multiplexed_async_connection().await
        .map_err(|e| format!("Failed to connect to redis: {}", e))?;

    let data: Option<String> = con.hget("printf_jobs_status", &file_id).await
        .map_err(|e| format!("Failed to fetch job info: {}", e))?;

    if let Some(json_str) = data {
        let job_info: JobInfo = serde_json::from_str(&json_str)
            .map_err(|e| format!("Failed to parse job info: {}", e))?;

        let payload = serde_json::to_string(&[&job_info.attributes])
            .map_err(|e| format!("Failed to serialize attributes: {}", e))?;

        con.lpush::<_, _, ()>("printf_queue", payload).await
            .map_err(|e| format!("Failed to lpush printf_queue: {}", e))?;

        update_job_status(&redis_client, &job_info.attributes, "Queued").await;
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
async fn get_stats(month: Option<String>, state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    let mut url = "https://print.aditya.stream/stats".to_string();
    if let Some(m) = month {
        if !m.is_empty() {
            url = format!("https://print.aditya.stream/stats?month={}", m);
        }
    }
    match state.http_client.get(&url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => Ok(text),
            Err(e) => Err(format!("Failed to read stats text: {}", e)),
        },
        Err(e) => Err(format!("Failed to fetch stats: {}", e)),
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
            })
        }
    };

    let http_client = Arc::new(reqwest::Client::new());

    let app_state = Arc::new(AppState {
        config,
        http_client,
        is_running: Arc::new(AtomicBool::new(false)),
        cancel_tx: Mutex::new(None),
    });

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_client,
            stop_client,
            get_client_status,
            get_jobs,
            reprint_job,
            minimize_window,
            maximize_window,
            close_window,
            get_stats
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
