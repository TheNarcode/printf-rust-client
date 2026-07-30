#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{
    ApiOrder, CfAckRequest, CfLeaseId, CfQueueMessage, CfQueuePullRequest, CfQueuePullResponse,
    Config, JobInfo, PrintAttributes,
};
use ftail::Ftail;
use log::LevelFilter;
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
    pub job_store: Arc<Mutex<HashMap<String, JobInfo>>>,
}

fn current_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

async fn update_job_status(
    job_store: &Arc<Mutex<HashMap<String, JobInfo>>>,
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

    // Store in local in-memory state
    {
        let mut store = job_store.lock().await;
        store.insert(attributes.file_id.clone(), job_info);
    }

    log::info!(
        "updated job status for {} to {}",
        attributes.file_id,
        status
    );
}

fn parse_message_body(body: &serde_json::Value) -> Result<Vec<PrintAttributes>, String> {
    match body {
        serde_json::Value::String(s) => {
            // First attempt: base64 decode
            if let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
                if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                    if let Ok(list) = serde_json::from_str::<Vec<PrintAttributes>>(&decoded_str) {
                        return Ok(list);
                    }
                    if let Ok(single) = serde_json::from_str::<PrintAttributes>(&decoded_str) {
                        return Ok(vec![single]);
                    }
                }
            }
            // Second attempt: parse raw JSON string
            if let Ok(list) = serde_json::from_str::<Vec<PrintAttributes>>(s) {
                return Ok(list);
            }
            if let Ok(single) = serde_json::from_str::<PrintAttributes>(s) {
                return Ok(vec![single]);
            }
            Err(format!("Could not parse body string: {}", s))
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if let Ok(list) = serde_json::from_value::<Vec<PrintAttributes>>(body.clone()) {
                return Ok(list);
            }
            if let Ok(single) = serde_json::from_value::<PrintAttributes>(body.clone()) {
                return Ok(vec![single]);
            }
            Err(format!("Could not parse body JSON structure: {}", body))
        }
        _ => Err("Invalid body format in message".to_string()),
    }
}

async fn pull_cf_queue_messages(
    http_client: &reqwest::Client,
    account_id: &str,
    queue_id: &str,
    token: &str,
) -> Result<Vec<CfQueueMessage>, String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/queues/{}/messages/pull",
        account_id, queue_id
    );
    let payload = CfQueuePullRequest {
        visibility_timeout_ms: 30000,
        batch_size: 10,
    };

    let resp = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send pull request to Cloudflare Queue: {}", e))?;

    if !resp.status().is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        return Err(format!("Cloudflare Queue pull error ({})", err_text));
    }

    let pull_resp: CfQueuePullResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Cloudflare Queue pull response: {}", e))?;

    if !pull_resp.success {
        return Err(format!(
            "Cloudflare Queue pull reported failure: {:?}",
            pull_resp.errors
        ));
    }

    Ok(pull_resp.result.map(|r| r.messages).unwrap_or_default())
}

async fn ack_cf_queue_messages(
    http_client: &reqwest::Client,
    account_id: &str,
    queue_id: &str,
    token: &str,
    acks: Vec<CfLeaseId>,
    retries: Vec<CfLeaseId>,
) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/queues/{}/messages/ack",
        account_id, queue_id
    );
    let payload = CfAckRequest { acks, retries };

    let resp = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send ack request to Cloudflare Queue: {}", e))?;

    if !resp.status().is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        return Err(format!("Cloudflare Queue ack error ({})", err_text));
    }

    Ok(())
}

async fn dispatch_job_batch(
    attributes_list: Vec<PrintAttributes>,
    pm: Arc<Mutex<Option<PrinterManager>>>,
    config: Arc<Config>,
    http_client: Arc<reqwest::Client>,
    job_store: Arc<Mutex<HashMap<String, JobInfo>>>,
) -> bool {
    let has_color = attributes_list.iter().any(|a| a.color == crate::types::ColorMode::Color);
    let has_mono = attributes_list.iter().any(|a| a.color == crate::types::ColorMode::Monochrome);

    let (color_printer, mono_printer, color_media_source, mono_media_source) = {
        let mut pm_guard = pm.lock().await;
        let pm_ref = pm_guard.as_mut().unwrap();
        pm_ref.get_printers_for_order(has_color, has_mono)
    };

    let order_id: Option<String> = attributes_list.first().and_then(|a| a.order.clone());

    let mut all_succeeded = true;

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
                        update_job_status(&job_store, &attributes, order_id.clone(), "Failed").await;
                        all_succeeded = false;
                        continue;
                    }
                },
                crate::types::ColorMode::Monochrome => match &mono_printer {
                    Some(p) => p.clone(),
                    None => {
                        log::error!("no monochrome printer found for order");
                        update_job_status(&job_store, &attributes, order_id.clone(), "Failed").await;
                        all_succeeded = false;
                        continue;
                    }
                },
            }
        };

        let config_cloned = Arc::clone(&config);
        let http_client_cloned = Arc::clone(&http_client);
        let media_source = if is_color {
            color_media_source.clone()
        } else {
            mono_media_source.clone()
        };
        let order_id_cloned = order_id.clone();

        attributes.target_printer = Some(printer.name.clone());

        update_job_status(&job_store, &attributes, order_id_cloned.clone(), "Processing").await;

        log::info!("using printer {} ({}) for print", printer.name, printer.uri);

        let failed = match printer.uri.parse() {
            Ok(uri) => match print_job(uri, printer.name.clone(), attributes.clone(), media_source, config_cloned, http_client_cloned).await {
                Ok(_) => {
                    log::info!("print job successful");
                    None
                }
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
            all_succeeded = false;
            if err_msg.contains("PendingTimeout") {
                update_job_status(&job_store, &attributes, order_id_cloned.clone(), "Stuck").await;
            } else {
                update_job_status(&job_store, &attributes, order_id_cloned.clone(), "Failed").await;
            }
        } else {
            update_job_status(&job_store, &attributes, order_id_cloned.clone(), "Completed").await;
        }
    }

    all_succeeded
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
    let job_store = Arc::clone(&state.job_store);

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

        let cf_account_id = config.cf_account_id.clone();
        let cf_queue_id = config.cf_queue_id.clone();
        let cf_token = config.cf_api_token.clone().or_else(|| config.printf_key.clone());

        if cf_account_id.is_none() || cf_queue_id.is_none() || cf_token.is_none() {
            log::error!("Cloudflare Queue configuration missing (cf_account_id, cf_queue_id, or cf_api_token/printf_key)");
            is_running_flag.store(false, Ordering::SeqCst);
            return;
        }

        let account_id = cf_account_id.unwrap();
        let queue_id = cf_queue_id.unwrap();
        let token = cf_token.unwrap();

        log::info!("Starting Cloudflare Queue pull consumer for queue '{}'", queue_id);

        let mut backoff = Duration::from_secs(2);

        loop {
            if !is_running_flag.load(Ordering::SeqCst) {
                log::info!("client stop requested in Cloudflare Queue loop");
                break;
            }

            tokio::select! {
                _ = &mut rx => {
                    log::info!("client loop cancelled via channel");
                    is_running_flag.store(false, Ordering::SeqCst);
                    break;
                }
                pull_res = pull_cf_queue_messages(&http_client, &account_id, &queue_id, &token) => {
                    match pull_res {
                        Ok(messages) => {
                            backoff = Duration::from_secs(2);
                            if messages.is_empty() {
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                                    _ = &mut rx => {
                                        log::info!("client loop cancelled while waiting");
                                        is_running_flag.store(false, Ordering::SeqCst);
                                        break;
                                    }
                                }
                                continue;
                            }

                            log::info!("pulled {} message(s) from Cloudflare Queue", messages.len());

                            let mut acks = Vec::new();
                            let mut retries = Vec::new();

                            for msg in messages {
                                match parse_message_body(&msg.body) {
                                    Ok(attributes_list) => {
                                        let success = dispatch_job_batch(
                                            attributes_list,
                                            Arc::clone(&pm),
                                            Arc::clone(&config),
                                            Arc::clone(&http_client),
                                            Arc::clone(&job_store),
                                        ).await;

                                        if success {
                                            log::info!("printing completed successfully for message (id: {}), sending ACK", msg.id);
                                            acks.push(CfLeaseId {
                                                lease_id: msg.lease_id,
                                                delay_seconds: None,
                                            });
                                        } else {
                                            log::warn!("printing failed or stuck for message (id: {}), scheduling retry", msg.id);
                                            retries.push(CfLeaseId {
                                                lease_id: msg.lease_id,
                                                delay_seconds: Some(60),
                                            });
                                        }
                                    }
                                    Err(err) => {
                                        log::error!("failed to parse message body (id: {}): {}", msg.id, err);
                                        acks.push(CfLeaseId {
                                            lease_id: msg.lease_id,
                                            delay_seconds: None,
                                        });
                                    }
                                }
                            }

                            if !acks.is_empty() || !retries.is_empty() {
                                if let Err(e) = ack_cf_queue_messages(&http_client, &account_id, &queue_id, &token, acks, retries).await {
                                    log::error!("failed to ack/retry messages: {}", e);
                                }
                            }
                        }
                        Err(err) => {
                            log::error!("error pulling from Cloudflare Queue: {}", err);
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut rx => {
                                    log::info!("client loop cancelled during backoff");
                                    is_running_flag.store(false, Ordering::SeqCst);
                                    break;
                                }
                            }
                            backoff = (backoff * 2).min(Duration::from_secs(60));
                        }
                    }
                }
            }
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

    let mut jobs: Vec<JobInfo> = Vec::new();

    {
        let store = state.job_store.lock().await;
        for info in store.values() {
            jobs.push(info.clone());
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
    let mut target_job: Option<JobInfo> = None;

    {
        let store = state.job_store.lock().await;
        if let Some(info) = store.get(&file_id) {
            target_job = Some(info.clone());
        }
    }

    if let Some(job_info) = target_job {
        update_job_status(
            &state.job_store,
            &job_info.attributes,
            job_info.order_id.clone(),
            "Queued",
        ).await;

        dispatch_job_batch(
            vec![job_info.attributes],
            Arc::clone(&state.printer_manager),
            Arc::clone(&state.config),
            Arc::clone(&state.http_client),
            Arc::clone(&state.job_store),
        ).await;

        log::info!("re-queued job {} for reprint", file_id);
        Ok(())
    } else {
        Err(format!("Job {} not found in job store", file_id))
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
    let mut target_job: Option<JobInfo> = None;

    {
        let store = state.job_store.lock().await;
        if let Some(info) = store.get(&file_id) {
            target_job = Some(info.clone());
        }
    }

    if let Some(mut job_info) = target_job {
        job_info.attributes.target_printer = Some(printer_uri);

        update_job_status(
            &state.job_store,
            &job_info.attributes,
            job_info.order_id.clone(),
            "Queued",
        ).await;

        dispatch_job_batch(
            vec![job_info.attributes],
            Arc::clone(&state.printer_manager),
            Arc::clone(&state.config),
            Arc::clone(&state.http_client),
            Arc::clone(&state.job_store),
        ).await;

        log::info!("re-queued stuck job {} to new printer", file_id);
        Ok(())
    } else {
        Err(format!("Job {} not found in job store", file_id))
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
                s3_base_url: "http://localhost:8000/".to_string(),
                webhook_url: None,
                printf_key: None,
                base_url: BASE_URL.to_string(),
                cf_account_id: None,
                cf_queue_id: None,
                cf_api_token: None,
            })
        }
    };

    let http_client = Arc::new(reqwest::Client::new());
    let job_store = Arc::new(Mutex::new(HashMap::new()));

    let app_state = Arc::new(AppState {
        config,
        http_client,
        is_running: Arc::new(AtomicBool::new(false)),
        cancel_tx: Mutex::new(None),
        printer_manager: Arc::new(Mutex::new(None)),
        job_store,
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
