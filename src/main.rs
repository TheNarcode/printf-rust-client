#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::ipp::prelude::*;
use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{
    ApiOrder, CfAckRequest, CfLeaseId, CfQueueMessage, CfQueuePullRequest, CfQueuePullResponse,
    Config, JobInfo, PrintAttributes, Printer,
};
use dioxus::desktop::{Config as DesktopConfig, WindowBuilder, use_window};
use dioxus::prelude::*;
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

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_timestamp() -> String {
    current_timestamp_secs().to_string()
}

async fn update_job_status(
    job_store: &Arc<Mutex<HashMap<String, JobInfo>>>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
) {
    let mut store = job_store.lock().await;
    let (existing_lease, existing_ipp_job_id) = store.get(&attributes.file_id)
        .map(|info| (info.lease_id.clone(), info.ipp_job_id))
        .unwrap_or((None, None));

    let job_info = JobInfo {
        file_id: attributes.file_id.clone(),
        order_id,
        attributes: attributes.clone(),
        status: status.to_string(),
        updated_at: current_timestamp(),
        lease_id: existing_lease,
        ipp_job_id: existing_ipp_job_id,
    };

    store.insert(attributes.file_id.clone(), job_info);

    log::info!(
        "updated job status for {} to {}",
        attributes.file_id,
        status
    );
}

async fn update_job_status_with_lease(
    job_store: &Arc<Mutex<HashMap<String, JobInfo>>>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
    lease_id: Option<String>,
) {
    let mut store = job_store.lock().await;
    let (existing_lease, existing_ipp_job_id) = store.get(&attributes.file_id)
        .map(|info| (info.lease_id.clone(), info.ipp_job_id))
        .unwrap_or((None, None));
    let final_lease = lease_id.or(existing_lease);

    let job_info = JobInfo {
        file_id: attributes.file_id.clone(),
        order_id,
        attributes: attributes.clone(),
        status: status.to_string(),
        updated_at: current_timestamp(),
        lease_id: final_lease,
        ipp_job_id: existing_ipp_job_id,
    };

    store.insert(attributes.file_id.clone(), job_info);

    log::info!(
        "updated job status for {} to {}",
        attributes.file_id,
        status
    );
}

fn parse_message_body_str(s: &str) -> Result<Vec<PrintAttributes>, String> {
    if let Ok(order) = serde_json::from_str::<ApiOrder>(s) {
        if !order.files.is_empty() {
            return Ok(order.to_print_attributes_list());
        }
    }
    if let Ok(orders) = serde_json::from_str::<Vec<ApiOrder>>(s) {
        let mut list = Vec::new();
        for order in orders {
            list.extend(order.to_print_attributes_list());
        }
        if !list.is_empty() {
            return Ok(list);
        }
    }
    if let Ok(list) = serde_json::from_str::<Vec<PrintAttributes>>(s) {
        return Ok(list);
    }
    if let Ok(single) = serde_json::from_str::<PrintAttributes>(s) {
        return Ok(vec![single]);
    }
    Err(format!("Could not parse body string: {}", s))
}

fn parse_message_body(body: &serde_json::Value) -> Result<Vec<PrintAttributes>, String> {
    match body {
        serde_json::Value::String(s) => {
            if let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
                if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                    if let Ok(list) = parse_message_body_str(&decoded_str) {
                        return Ok(list);
                    }
                }
            }
            parse_message_body_str(s)
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if let Ok(order) = serde_json::from_value::<ApiOrder>(body.clone()) {
                if !order.files.is_empty() {
                    return Ok(order.to_print_attributes_list());
                }
            }
            if let Ok(orders) = serde_json::from_value::<Vec<ApiOrder>>(body.clone()) {
                let mut list = Vec::new();
                for order in orders {
                    list.extend(order.to_print_attributes_list());
                }
                if !list.is_empty() {
                    return Ok(list);
                }
            }
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
        visibility_timeout_ms: 180000,
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
        if let Some(ref mut pm_ref) = *pm_guard {
            pm_ref.get_printers_for_order(has_color, has_mono)
        } else {
            (None, None, None, None)
        }
    };

    let order_id: Option<String> = attributes_list.first().and_then(|a| a.order.clone());
    let mut all_succeeded = true;

    for mut attributes in attributes_list {
        let is_color = attributes.color == crate::types::ColorMode::Color;
        let printer = if let Some(target) = &attributes.target_printer {
            let pm_lock = pm.lock().await;
            if let Some(ref pm_ref) = *pm_lock {
                if let Some(found) = pm_ref.get_printers().iter().find(|p| p.name == *target || p.uri == *target) {
                    found.clone()
                } else {
                    crate::types::Printer {
                        uri: if target.starts_with("http") || target.starts_with("ipp") { target.clone() } else { format!("ipp://localhost:631/printers/{}", target) },
                        name: target.clone(),
                        color_mode: attributes.color.clone(),
                        paused: false,
                        properties: None,
                    }
                }
            } else {
                crate::types::Printer {
                    uri: if target.starts_with("http") || target.starts_with("ipp") { target.clone() } else { format!("ipp://localhost:631/printers/{}", target) },
                    name: target.clone(),
                    color_mode: attributes.color.clone(),
                    paused: false,
                    properties: None,
                }
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
            Ok(uri) => match print_job(uri, printer.name.clone(), attributes.clone(), media_source, config_cloned, http_client_cloned, Arc::clone(&job_store)).await {
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

async fn start_client(state: Arc<AppState>) -> Result<String, String> {
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
    let state_init = Arc::clone(&state);

    tokio::spawn(async move {
        let creds = get_cups_creds(&state_init.config);
        match get_ipp_printers(creds).await {
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
                log::warn!("failed to get ipp printers: {}; initializing empty manager", e);
                let mut pm_lock = pm.lock().await;
                if pm_lock.is_none() {
                    *pm_lock = Some(PrinterManager::new(Vec::new()));
                }
            }
        }

        let cf_account_id = config.cf_account_id.clone();
        let cf_queue_id = config.cf_queue_id.clone();
        let cf_token = config.cf_api_token.clone().or_else(|| config.printf_key.clone());

        let mut backoff = Duration::from_secs(2);

        loop {
            if !is_running_flag.load(Ordering::SeqCst) {
                log::info!("client stop requested in Cloudflare Queue loop");
                break;
            }

            if cf_account_id.is_none() || cf_queue_id.is_none() || cf_token.is_none() {
                log::warn!("Cloudflare Queue configuration missing (cf_account_id, cf_queue_id, or cf_api_token/printf_key)");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = &mut rx => {
                        log::info!("client loop cancelled while waiting for config");
                        is_running_flag.store(false, Ordering::SeqCst);
                        break;
                    }
                }
                continue;
            }

            let account_id = cf_account_id.as_ref().unwrap();
            let queue_id = cf_queue_id.as_ref().unwrap();
            let token = cf_token.as_ref().unwrap();

            tokio::select! {
                _ = &mut rx => {
                    log::info!("client loop cancelled via channel");
                    is_running_flag.store(false, Ordering::SeqCst);
                    break;
                }
                pull_res = pull_cf_queue_messages(&http_client, account_id, queue_id, token) => {
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

                            for msg in messages {
                                let http_client = Arc::clone(&http_client);
                                let account_id = account_id.clone();
                                let queue_id = queue_id.clone();
                                let token = token.clone();
                                let pm = Arc::clone(&pm);
                                let config = Arc::clone(&config);
                                let job_store = Arc::clone(&job_store);

                                tokio::spawn(async move {
                                    match parse_message_body(&msg.body) {
                                        Ok(attributes_list) => {
                                            for attr in &attributes_list {
                                                update_job_status_with_lease(
                                                    &job_store,
                                                    attr,
                                                    attr.order.clone(),
                                                    "Queued",
                                                    Some(msg.lease_id.clone()),
                                                ).await;
                                            }

                                            let success = dispatch_job_batch(
                                                attributes_list,
                                                pm,
                                                config,
                                                Arc::clone(&http_client),
                                                job_store,
                                            ).await;

                                            let (acks, retries) = if success {
                                                log::info!("printing completed successfully for message (id: {}), sending ACK", msg.id);
                                                (vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: None }], vec![])
                                            } else {
                                                log::warn!("printing failed or stuck for message (id: {}), scheduling retry", msg.id);
                                                (vec![], vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: Some(60) }])
                                            };

                                            if let Err(e) = ack_cf_queue_messages(&http_client, &account_id, &queue_id, &token, acks, retries).await {
                                                log::error!("failed to ack/retry message (id: {}): {}", msg.id, e);
                                            }
                                        }
                                        Err(err) => {
                                            log::error!("failed to parse message body (id: {}): {}", msg.id, err);
                                            let _ = ack_cf_queue_messages(&http_client, &account_id, &queue_id, &token, vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: None }], vec![]).await;
                                        }
                                    }
                                });
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

async fn stop_client(state: Arc<AppState>) -> Result<String, String> {
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

async fn get_jobs(state: Arc<AppState>) -> Vec<JobInfo> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Vec::new();
    }

    let mut jobs: Vec<JobInfo> = Vec::new();
    {
        let store = state.job_store.lock().await;
        for info in store.values() {
            jobs.push(info.clone());
        }
    }
    jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    jobs
}

async fn get_completed_orders(state: Arc<AppState>) -> Result<Vec<ApiOrder>, String> {
    let url = format!("{}/client/orders", BASE_URL);
    let mut req = state.http_client.get(&url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("Failed to fetch orders: {}", e))?;
    let orders = resp.json::<Vec<ApiOrder>>().await.map_err(|e| format!("Failed to parse orders: {}", e))?;

    let filtered: Vec<ApiOrder> = orders
        .into_iter()
        .filter(|o| o.status.unwrap_or(0) != 3 && (o.status.unwrap_or(0) == 1 || o.paid.unwrap_or(false)))
        .collect();

    Ok(filtered)
}

fn get_cups_creds(config: &Config) -> Option<(&str, &str)> {
    match (&config.cups_username, &config.cups_password) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Some((u.as_str(), p.as_str())),
        _ => None,
    }
}

pub async fn fetch_printer_properties_from_cups(name: &str, state: Arc<AppState>) -> (crate::types::PrinterProperties, crate::types::ColorMode) {
    let creds = get_cups_creds(&state.config);
    crate::ipp::fetch_printer_properties_via_ipp(name, creds).await
}

async fn get_printer_list(state: Arc<AppState>) -> Result<Vec<Printer>, String> {
    let creds = get_cups_creds(&state.config);
    let list = crate::ipp::get_ipp_printers(creds).await.map_err(|e| e.to_string())?;
    let mut updated_list = Vec::new();

    let paused_map: HashMap<String, bool> = {
        let pm_lock = state.printer_manager.lock().await;
        if let Some(ref pm) = *pm_lock {
            pm.get_printers().into_iter().map(|p| (p.name.clone(), p.paused)).collect()
        } else {
            HashMap::new()
        }
    };

    for mut p in list {
        let (props, color) = fetch_printer_properties_from_cups(&p.name, state.clone()).await;
        p.properties = Some(props);
        p.color_mode = color;
        if let Some(&is_paused) = paused_map.get(&p.name) {
            p.paused = is_paused;
        }
        updated_list.push(p);
    }

    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        for p in &updated_list {
            if let Some(props) = &p.properties {
                pm.set_printer_properties(&p.name, props.clone(), p.color_mode.clone());
            }
        }
    } else {
        *pm_lock = Some(PrinterManager::new(updated_list.clone()));
    }

    Ok(updated_list)
}

async fn pause_printer(uri: String, state: Arc<AppState>) -> Result<(), String> {
    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        pm.set_printer_paused(&uri, true);
        log::info!("Paused printer: {}", uri);
        Ok(())
    } else {
        Err("Printer manager not initialized".to_string())
    }
}

async fn unpause_printer(uri: String, state: Arc<AppState>) -> Result<(), String> {
    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        pm.set_printer_paused(&uri, false);
        log::info!("Unpaused printer: {}", uri);
        Ok(())
    } else {
        Err("Printer manager not initialized".to_string())
    }
}

async fn add_appsocket_printer(
    name: String,
    ip: String,
    port: u16,
    color_mode: crate::types::ColorMode,
    state: Arc<AppState>,
) -> Result<String, String> {
    let clean_name = name.trim().replace(' ', "_");
    if clean_name.is_empty() || ip.trim().is_empty() {
        return Err("Printer name and IP address are required".to_string());
    }

    let creds = get_cups_creds(&state.config);
    crate::ipp::add_appsocket_printer_via_ipp(&clean_name, &ip, port, color_mode, creds).await?;
    log::info!("Successfully added AppSocket printer {} via IPP HTTP", clean_name);
    Ok(format!("AppSocket printer {} added successfully", clean_name))
}

async fn save_printer_properties(
    name: String,
    props: crate::types::PrinterProperties,
    color_mode: crate::types::ColorMode,
    state: Arc<AppState>,
) -> Result<(), String> {
    let creds = get_cups_creds(&state.config);
    let _ = crate::ipp::save_printer_properties_via_ipp(&name, &props, &color_mode, creds).await;

    let mut pm_lock = state.printer_manager.lock().await;
    if let Some(ref mut pm) = *pm_lock {
        pm.set_printer_properties(&name, props.clone(), color_mode);
    }

    log::info!("Saved printer properties for {}: {:?}", name, props);
    Ok(())
}

async fn reprint_job(file_id: String, state: Arc<AppState>) -> Result<(), String> {
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

        let success = dispatch_job_batch(
            vec![job_info.attributes],
            Arc::clone(&state.printer_manager),
            Arc::clone(&state.config),
            Arc::clone(&state.http_client),
            Arc::clone(&state.job_store),
        ).await;

        if success {
            log::info!("re-queued job {} for reprint succeeded", file_id);
            if let Some(ref lease_id) = job_info.lease_id {
                let cf_account_id = state.config.cf_account_id.clone();
                let cf_queue_id = state.config.cf_queue_id.clone();
                let cf_token = state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone());

                if let (Some(account_id), Some(queue_id), Some(token)) = (cf_account_id, cf_queue_id, cf_token) {
                    let acks = vec![CfLeaseId { lease_id: lease_id.clone(), delay_seconds: None }];
                    let _ = ack_cf_queue_messages(&state.http_client, &account_id, &queue_id, &token, acks, vec![]).await;
                }
            }
        }

        Ok(())
    } else {
        Err(format!("Job {} not found in job store", file_id))
    }
}

async fn requeue_to_printer(
    file_id: String,
    printer_uri: String,
    state: Arc<AppState>,
) -> Result<(), String> {
    let mut target_job: Option<JobInfo> = None;
    {
        let store = state.job_store.lock().await;
        if let Some(info) = store.get(&file_id) {
            target_job = Some(info.clone());
        }
    }

    if let Some(mut job_info) = target_job {
        if let Some(old_job_id) = job_info.ipp_job_id {
            if let Some(ref old_printer_name) = job_info.attributes.target_printer {
                let old_path = if old_printer_name.starts_with('/') {
                    old_printer_name.clone()
                } else if old_printer_name.starts_with("http") || old_printer_name.starts_with("ipp") {
                    if let Ok(u) = old_printer_name.parse::<Uri>() {
                        u.path().to_string()
                    } else {
                        format!("/printers/{}", old_printer_name)
                    }
                } else {
                    format!("/printers/{}", old_printer_name)
                };

                let creds = get_cups_creds(&state.config);
                let old_uri_str = crate::ipp::format_ipp_uri(&old_path, creds);
                if let Ok(parsed_uri) = old_uri_str.parse::<Uri>() {
                    log::info!("Canceling old CUPS job {} on printer {} ({}) before requeue", old_job_id, old_printer_name, old_uri_str);
                    let cancel_op = IppOperationBuilder::cancel_job(parsed_uri.clone(), old_job_id).build();
                    let client = AsyncIppClient::new(parsed_uri);
                    let _ = client.send(cancel_op).await;
                }
            }
        }

        job_info.attributes.target_printer = Some(printer_uri);

        update_job_status(
            &state.job_store,
            &job_info.attributes,
            job_info.order_id.clone(),
            "Queued",
        ).await;

        let success = dispatch_job_batch(
            vec![job_info.attributes],
            Arc::clone(&state.printer_manager),
            Arc::clone(&state.config),
            Arc::clone(&state.http_client),
            Arc::clone(&state.job_store),
        ).await;

        if success {
            log::info!("re-queued stuck job {} to new printer succeeded", file_id);

            if let Some(ref lease_id) = job_info.lease_id {
                let cf_account_id = state.config.cf_account_id.clone();
                let cf_queue_id = state.config.cf_queue_id.clone();
                let cf_token = state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone());

                if let (Some(account_id), Some(queue_id), Some(token)) = (cf_account_id, cf_queue_id, cf_token) {
                    log::info!("sending ACK for requeued job {} (lease_id: {})", file_id, lease_id);
                    let acks = vec![CfLeaseId { lease_id: lease_id.clone(), delay_seconds: None }];
                    if let Err(e) = ack_cf_queue_messages(&state.http_client, &account_id, &queue_id, &token, acks, vec![]).await {
                        log::error!("failed to send ACK for requeued job (id: {}): {}", file_id, e);
                    } else {
                        log::info!("successfully ACKed requeued job {} to Cloudflare Queue", file_id);
                    }
                }
            }
        }

        Ok(())
    } else {
        Err(format!("Job {} not found in job store", file_id))
    }
}

async fn get_stats(month: Option<String>, state: Arc<AppState>) -> Result<serde_json::Value, String> {
    let mut url = format!("{}/client/stats", BASE_URL);
    if let Some(m) = month {
        if !m.is_empty() && m != "all" {
            url = format!("{}/client/stats?month={}", BASE_URL, m);
        }
    }

    let mut req = state.http_client.get(url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("Failed to fetch stats: {}", e))?;
    let json = resp.json::<serde_json::Value>().await.map_err(|e| format!("Failed to parse stats: {}", e))?;
    Ok(json)
}

async fn mark_order_collected(order_id: String, state: Arc<AppState>) -> Result<(), String> {
    let url = format!("{}/client/collect", BASE_URL);
    let payload = serde_json::json!({ "orderId": order_id });
    let mut req = state.http_client.post(url).json(&payload);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("x-printf-key", key.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("Failed to mark collected: {}", e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Failed with status: {}", resp.status()))
    }
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Jobs,
    Stats,
    Completed,
    Settings,
}

#[component]
fn App() -> Element {
    let window = use_window();
    let app_state = use_context::<Arc<AppState>>();
    let mut is_running = use_signal(|| app_state.is_running.load(Ordering::SeqCst));
    let mut active_tab = use_signal(|| Tab::Jobs);
    let mut jobs = use_signal(Vec::<JobInfo>::new);
    let mut printers = use_signal(Vec::<Printer>::new);
    let mut completed_orders = use_signal(Vec::<ApiOrder>::new);
    let mut completed_search = use_signal(String::new);
    let mut selected_month = use_signal(|| "current".to_string());
    let mut stats_json = use_signal(|| serde_json::Value::Null);
    let mut now_secs = use_signal(current_timestamp_secs);
    let mut selected_requeue_printers = use_signal(HashMap::<String, String>::new);

    // Add & Edit Printer Modal Signals
    let mut show_add_modal = use_signal(|| false);
    let mut new_name = use_signal(String::new);
    let mut new_ip = use_signal(String::new);
    let mut new_port = use_signal(|| "9100".to_string());
    let mut new_color = use_signal(|| crate::types::ColorMode::Color);
    let mut add_status_msg = use_signal(String::new);

    let mut editing_printer = use_signal(|| None::<Printer>);
    let mut edit_media = use_signal(|| "iso_a4_210x297mm".to_string());
    let mut edit_media_source = use_signal(|| "auto".to_string());
    let mut edit_orientation = use_signal(|| "portrait".to_string());
    let mut edit_print_quality = use_signal(|| "normal".to_string());
    let mut edit_sides = use_signal(|| "one-sided".to_string());
    let mut edit_color = use_signal(|| crate::types::ColorMode::Color);

    // Initial printers fetch
    let app_state_init = app_state.clone();
    use_future(move || {
        let state = app_state_init.clone();
        async move {
            if let Ok(list) = get_printer_list(state).await {
                printers.set(list);
            }
        }
    });

    // Main 1s timer for UI updates (clock & jobs polling)
    let app_state_timer = app_state.clone();
    use_future(move || {
        let app_state = app_state_timer.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                now_secs.set(current_timestamp_secs());
                let running = app_state.is_running.load(Ordering::SeqCst);
                if is_running() != running {
                    is_running.set(running);
                }
                let current_jobs = get_jobs(app_state.clone()).await;
                jobs.set(current_jobs);
            }
        }
    });

    // Tab change trigger data loading
    let app_state_tab = app_state.clone();
    use_effect(move || {
        let tab = active_tab();
        let month = selected_month();
        let app_state = app_state_tab.clone();
        spawn(async move {
            match tab {
                Tab::Jobs => {}
                Tab::Stats => {
                    if let Ok(data) = get_stats(Some(month), app_state.clone()).await {
                        stats_json.set(data);
                    }
                }
                Tab::Completed => {
                    if let Ok(orders) = get_completed_orders(app_state.clone()).await {
                        completed_orders.set(orders);
                    }
                }
                Tab::Settings => {
                    if let Ok(list) = get_printer_list(app_state.clone()).await {
                        printers.set(list);
                    }
                }
            }
        });
    });

    // Calculate month select options
    let month_options = use_memo(|| {
        let mut opts = Vec::new();
        opts.push(("current".to_string(), "Current Month".to_string()));
        let now = chrono::Local::now();
        for i in 1..=3 {
            let d = now - chrono::Months::new(i);
            let month_str = d.format("%Y-%m").to_string();
            let month_name = d.format("%B %Y").to_string();
            opts.push((month_str, month_name));
        }
        opts.push(("all".to_string(), "All Time".to_string()));
        opts
    });

    // Helper functions for stats
    let get_stat_count = |key: &str| -> f64 {
        stats_json().get(key).and_then(|v| v.get("pages")).and_then(|v| v.as_f64()).unwrap_or(0.0)
    };

    let count_1s_mono = get_stat_count("b/w single sided");
    let count_2s_mono = get_stat_count("b/w double sided");
    let count_1s_color = get_stat_count("color single sided");
    let count_2s_color = get_stat_count("color double sided");

    let price_1s_mono = count_1s_mono * 3.0;
    let price_2s_mono = count_2s_mono * 2.0;
    let price_1s_color = count_1s_color * 6.0;
    let price_2s_color = count_2s_color * 6.0;

    let net_1s_mono = price_1s_mono * 0.975;
    let net_2s_mono = price_2s_mono * 0.975;
    let net_1s_color = price_1s_color * 0.975;
    let net_2s_color = price_2s_color * 0.975;

    let total_count = count_1s_mono + count_2s_mono + count_1s_color + count_2s_color;
    let total_price = price_1s_mono + price_2s_mono + price_1s_color + price_2s_color;
    let total_net = net_1s_mono + net_2s_mono + net_1s_color + net_2s_color;
    let vendor_payable = 2.0 * (total_price - total_net);

    let jobs_count_text = format!("{} Job{}", jobs().len(), if jobs().len() == 1 { "" } else { "s" });

    let app_state_toggle = app_state.clone();
    let window_drag = window.clone();

    rsx! {
        style { {include_str!("../ui/styles.css")} }
        div { class: "app-container",
            // Header Bar
            header {
                class: "app-header",
                div {
                    class: "header-brand",
                    style: "cursor: move;",
                    onmousedown: move |_| {
                        window_drag.drag();
                    },
                    div { class: "brand-logo",
                        svg {
                            class: "bi bi-printer-fill",
                            view_box: "0 0 16 16",
                            width: "16",
                            height: "16",
                            fill: "currentColor",
                            path { d: "M5 1a2 2 0 0 0-2 2v1h10V3a2 2 0 0 0-2-2zm6 8H5a1 1 0 0 0-1 1v3a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1" }
                            path { d: "M0 7a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2h-1v-2a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v2H2a2 2 0 0 1-2-2zm2.5 1a.5.5 0 1 0 0-1 .5.5 0 0 0 0 1" }
                        }
                    }
                    h1 { "printf" }
                }
                div { class: "header-controls",
                    div {
                        class: if is_running() { "status-indicator status-running" } else { "status-indicator status-stopped" },
                        span { class: "status-dot" }
                        span { if is_running() { "Running" } else { "Stopped" } }
                    }
                    button {
                        class: if is_running() { "btn btn-danger btn-sm" } else { "btn btn-primary btn-sm" },
                        onclick: {
                            let state = app_state_toggle.clone();
                            move |_| {
                                log::info!("Toggle Start/Stop Client clicked");
                                let state = state.clone();
                                spawn(async move {
                                    if is_running() {
                                        if let Ok(msg) = stop_client(state.clone()).await {
                                            log::info!("{}", msg);
                                        }
                                    } else {
                                        if let Ok(msg) = start_client(state.clone()).await {
                                            log::info!("{}", msg);
                                        }
                                    }
                                    let running = state.is_running.load(Ordering::SeqCst);
                                    is_running.set(running);
                                });
                            }
                        },
                        if is_running() { "Stop Client" } else { "Start Client" }
                    }
                }
            }

            // Main Content Area
            main { class: "main-content",
                {match active_tab() {
                    Tab::Jobs => rsx! {
                        div { class: "page-view active",
                            section { class: "section-jobs",
                                div { class: "section-header",
                                    h2 { "Active Print Jobs" }
                                    span { class: "badge-count", "{jobs_count_text}" }
                                }
                                div { class: "jobs-list-container",
                                    div { class: "jobs-list",
                                        if jobs().is_empty() {
                                            div { class: "empty-state",
                                                p { "No active print jobs" }
                                                span { "Start the client to monitor incoming jobs." }
                                            }
                                        } else {
                                            for job in jobs() {
                                                {
                                                    let status = job.status.to_lowercase();
                                                    let is_stuck = status == "stuck" || status == "failed";
                                                    let updated_at_u64 = job.updated_at.parse::<u64>().unwrap_or(0);
                                                    let limit: Option<u64> = match status.as_str() {
                                                        "queued" => Some(30),
                                                        "processing" => Some(120),
                                                        _ => None,
                                                    };

                                                    let file_id = job.file_id.clone();
                                                    let f_id_select = file_id.clone();
                                                    let f_id_requeue = file_id.clone();
                                                    let f_id_reprint = file_id.clone();

                                                    let title_label = match &job.order_id {
                                                        Some(order) => format!("{} — {}", order, file_id),
                                                        None => file_id.clone(),
                                                    };

                                                    let a = &job.attributes;
                                                    let color_str = if a.color == crate::types::ColorMode::Color { "Color" } else { "B&W" };
                                                    let copies_num = a.copies.parse::<i32>().unwrap_or(1);
                                                    let num_up = a.number_up.parse::<i32>().unwrap_or(1);

                                                    let app_state_requeue = app_state.clone();
                                                    let app_state_reprint = app_state.clone();

                                                    rsx! {
                                                        div { class: "job-row-new", key: "{file_id}",
                                                            div { class: "job-row-header",
                                                                div { style: "display:flex;align-items:center;gap:0.5rem;min-width:0;flex:1",
                                                                    span { class: "job-status-dot dot-{status}" }
                                                                    span { class: "job-row-title", "{title_label}" }
                                                                }
                                                                div { class: "job-actions",
                                                                    if is_stuck {
                                                                        {
                                                                            let curr_val = selected_requeue_printers().get(&f_id_select).cloned().unwrap_or_default();
                                                                            rsx! {
                                                                                select {
                                                                                    class: "custom-select requeue-select",
                                                                                    style: "font-size:0.75rem;padding:0.3rem 1.5rem 0.3rem 0.6rem",
                                                                                    value: "{curr_val}",
                                                                                    onchange: {
                                                                                        let f_id = f_id_select.clone();
                                                                                        move |evt: Event<FormData>| {
                                                                                            let val = evt.value();
                                                                                            log::info!("Requeue printer selected for {}: {}", f_id, val);
                                                                                            let mut map = selected_requeue_printers();
                                                                                            map.insert(f_id.clone(), val);
                                                                                            selected_requeue_printers.set(map);
                                                                                        }
                                                                                    },
                                                                                    option { value: "", "Select Printer" }
                                                                                    for p in printers() {
                                                                                        option { value: "{p.name}", "{p.name}" }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                        button {
                                                                            class: "btn btn-primary btn-sm requeue-btn",
                                                                            onclick: {
                                                                                let state = app_state_requeue.clone();
                                                                                let f_id = f_id_requeue.clone();
                                                                                move |_| {
                                                                                    log::info!("Requeue button clicked for {}", f_id);
                                                                                    let f_id = f_id.clone();
                                                                                    let state = state.clone();
                                                                                    let target_p_name = selected_requeue_printers().get(&f_id).cloned().unwrap_or_default();
                                                                                    spawn(async move {
                                                                                        if !target_p_name.is_empty() {
                                                                                            let _ = requeue_to_printer(f_id, target_p_name, state).await;
                                                                                        }
                                                                                    });
                                                                                }
                                                                            },
                                                                            "Requeue"
                                                                        }
                                                                    }
                                                                    button {
                                                                        class: "btn-reprint reprint-btn",
                                                                        onclick: {
                                                                            let state = app_state_reprint.clone();
                                                                            let f_id = f_id_reprint.clone();
                                                                            move |_| {
                                                                                log::info!("Reprint button clicked for {}", f_id);
                                                                                let f_id = f_id.clone();
                                                                                let state = state.clone();
                                                                                spawn(async move {
                                                                                    let _ = reprint_job(f_id, state).await;
                                                                                });
                                                                            }
                                                                        },
                                                                        "Reprint"
                                                                    }
                                                                }
                                                            }

                                                            // Job Pills
                                                            div { class: "job-pills",
                                                                span { class: "pill", "{color_str}" }
                                                                if !a.sides.is_empty() {
                                                                    span { class: "pill", "{a.sides}" }
                                                                }
                                                                if copies_num > 1 {
                                                                    span { class: "pill", "×{copies_num}" }
                                                                }
                                                                if num_up > 1 {
                                                                    span { class: "pill", "{num_up}-up" }
                                                                }
                                                                if !a.paper_format.is_empty() {
                                                                    span { class: "pill", "{a.paper_format}" }
                                                                }
                                                                if !a.page_ranges.is_empty() {
                                                                    span { class: "pill", "pp {a.page_ranges}" }
                                                                }
                                                                if !a.orientation.is_empty() {
                                                                    span { class: "pill", "{a.orientation}" }
                                                                }
                                                                if !a.print_scaling.is_empty() {
                                                                    span { class: "pill", "{a.print_scaling}" }
                                                                }
                                                                if let Some(target) = &a.target_printer {
                                                                    span { class: "pill", "{target}" }
                                                                }
                                                            }

                                                            // Job Progress Timer
                                                            if let Some(lim) = limit {
                                                                {
                                                                    let elapsed = now_secs().saturating_sub(updated_at_u64);
                                                                    let display_elapsed = elapsed.min(lim);
                                                                    let percent = ((display_elapsed as f64 / lim as f64) * 100.0).min(100.0);
                                                                    let bg_color = if percent >= 80.0 {
                                                                        "hsl(0, 78%, 56%)"
                                                                    } else if percent >= 50.0 {
                                                                        "hsl(24, 90%, 55%)"
                                                                    } else {
                                                                        "hsl(142, 71%, 42%)"
                                                                    };

                                                                    rsx! {
                                                                        div { class: "job-timer",
                                                                            div { class: "job-timer-bar-wrap",
                                                                                div {
                                                                                    class: "job-timer-bar",
                                                                                    style: "width: {percent}%; background-color: {bg_color};"
                                                                                }
                                                                            }
                                                                            span { class: "job-timer-label", "{display_elapsed}s / {lim}s" }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Tab::Stats => {
                        let app_state_stats_btn = app_state.clone();
                        let app_state_stats_select = app_state.clone();
                        rsx! {
                            div { class: "page-view active",
                                section { class: "section-jobs",
                                    div { class: "section-header",
                                        h2 { "Print Statistics & Revenue" }
                                        div { class: "stats-controls",
                                            button {
                                                class: "btn btn-primary btn-sm",
                                                title: "Refresh Statistics",
                                                onclick: {
                                                    let state = app_state_stats_btn.clone();
                                                    move |_| {
                                                        log::info!("Refresh Stats button clicked");
                                                        let state = state.clone();
                                                        let month = selected_month();
                                                        spawn(async move {
                                                            if let Ok(data) = get_stats(Some(month), state).await {
                                                                stats_json.set(data);
                                                            }
                                                        });
                                                    }
                                                },
                                                "Refresh Stats"
                                            }
                                            select {
                                                class: "custom-select",
                                                value: selected_month(),
                                                onchange: {
                                                    let state = app_state_stats_select.clone();
                                                    move |evt: Event<FormData>| {
                                                        let m = evt.value();
                                                        log::info!("Month select changed to {}", m);
                                                        selected_month.set(m.clone());
                                                        let state = state.clone();
                                                        spawn(async move {
                                                            if let Ok(data) = get_stats(Some(m), state).await {
                                                                stats_json.set(data);
                                                            }
                                                        });
                                                    }
                                                },
                                                for (val, label) in month_options() {
                                                    option { value: "{val}", "{label}" }
                                                }
                                            }
                                        }
                                    }
                                    div { class: "stats-container",
                                        div { class: "stat-header-row",
                                            div { class: "stat-col-header", "Category" }
                                            div { class: "stat-col-header text-right", "Pages Printed" }
                                            div { class: "stat-col-header text-right", "Gross (100%)" }
                                            div { class: "stat-col-header text-right", "Net Earning (97.5%)" }
                                        }
                                        div { class: "stat-rows-group",
                                            div { class: "stat-row",
                                                div { class: "stat-category-label",
                                                    span { class: "category-dot dot-mono-1" }
                                                    span { "1 Sided Monochrome" }
                                                }
                                                div { class: "stat-val text-right", "{count_1s_mono}" }
                                                div { class: "stat-val text-right", span { "₹{price_1s_mono:.2}" } }
                                                div { class: "stat-val text-right stat-highlight", span { "₹{net_1s_mono:.2}" } }
                                            }
                                            div { class: "stat-row",
                                                div { class: "stat-category-label",
                                                    span { class: "category-dot dot-mono-2" }
                                                    span { "2 Sided Monochrome" }
                                                }
                                                div { class: "stat-val text-right", "{count_2s_mono}" }
                                                div { class: "stat-val text-right", span { "₹{price_2s_mono:.2}" } }
                                                div { class: "stat-val text-right stat-highlight", span { "₹{net_2s_mono:.2}" } }
                                            }
                                            div { class: "stat-row",
                                                div { class: "stat-category-label",
                                                    span { class: "category-dot dot-color-1" }
                                                    span { "1 Sided Color" }
                                                }
                                                div { class: "stat-val text-right", "{count_1s_color}" }
                                                div { class: "stat-val text-right", span { "₹{price_1s_color:.2}" } }
                                                div { class: "stat-val text-right stat-highlight", span { "₹{net_1s_color:.2}" } }
                                            }
                                            div { class: "stat-row",
                                                div { class: "stat-category-label",
                                                    span { class: "category-dot dot-color-2" }
                                                    span { "2 Sided Color" }
                                                }
                                                div { class: "stat-val text-right", "{count_2s_color}" }
                                                div { class: "stat-val text-right", span { "₹{price_2s_color:.2}" } }
                                                div { class: "stat-val text-right stat-highlight", span { "₹{net_2s_color:.2}" } }
                                            }
                                        }
                                        div { class: "stat-total-row",
                                            div { class: "stat-total-label", "Total Revenue" }
                                            div { class: "stat-total-val text-right", "{total_count}" }
                                            div { class: "stat-total-val text-right", span { "₹{total_price:.2}" } }
                                            div { class: "stat-total-val text-right stat-total-highlight", span { "₹{total_net:.2}" } }
                                        }
                                    }

                                    // Vendor Payable Container
                                    div { class: "vendor-payable-container",
                                        div { class: "vendor-payable-info",
                                            div { class: "vendor-payable-icon",
                                                svg {
                                                    class: "bi bi-currency-dollar",
                                                    view_box: "0 0 16 16",
                                                    width: "16",
                                                    height: "16",
                                                    fill: "currentColor",
                                                    path { d: "M4 10.781c.148 1.667 1.513 2.85 3.591 3.003V15h1.043v-1.216c2.27-.179 3.678-1.438 3.678-3.3 0-1.59-.947-2.51-2.956-3.028l-.722-.187V3.467c1.122.11 1.879.714 2.07 1.616h1.47c-.166-1.6-1.54-2.748-3.54-2.875V1H7.591v1.233c-1.939.23-3.27 1.472-3.27 3.156 0 1.454.966 2.483 2.661 2.917l.61.162v4.031c-1.149-.17-1.94-.8-2.131-1.718zm3.391-3.836c-1.043-.263-1.6-.825-1.6-1.616 0-.944.704-1.641 1.8-1.828v3.495l-.2-.05zm1.591 1.872c1.287.323 1.852.859 1.852 1.769 0 1.097-.826 1.828-2.2 1.939V8.73z" }
                                                }
                                            }
                                            div { class: "vendor-payable-text",
                                                h3 { "Vendor Payable Amount" }
                                            }
                                        }
                                        div { class: "vendor-payable-amount", "₹{vendor_payable:.2}" }
                                    }
                                }
                            }
                        }
                    },
                    Tab::Completed => {
                        let app_state_completed_btn = app_state.clone();
                        rsx! {
                            div { class: "page-view active",
                                section { class: "section-jobs",
                                    div { class: "completed-orders-section",
                                        div { class: "section-header", style: "margin-bottom: 1rem;",
                                            h2 { "Completed Orders" }
                                            div { style: "display: flex; gap: 0.5rem; align-items: center;",
                                                input {
                                                    r#type: "text",
                                                    placeholder: "Search Order ID...",
                                                    style: "padding: 0.35rem 0.75rem; border: 1px solid var(--border); border-radius: 0.375rem; font-size: 0.875rem; outline: none; width: 200px;",
                                                    value: completed_search(),
                                                    oninput: move |evt: Event<FormData>| completed_search.set(evt.value())
                                                }
                                                button {
                                                    class: "btn btn-primary btn-sm",
                                                    title: "Refresh Completed Orders",
                                                    onclick: {
                                                        let state = app_state_completed_btn.clone();
                                                        move |_| {
                                                            log::info!("Refresh Completed Orders clicked");
                                                            let state = state.clone();
                                                            spawn(async move {
                                                                if let Ok(orders) = get_completed_orders(state).await {
                                                                    completed_orders.set(orders);
                                                                }
                                                            });
                                                        }
                                                    },
                                                    "Refresh"
                                                }
                                            }
                                        }
                                        div { class: "jobs-list-container",
                                            div { class: "jobs-list",
                                                {
                                                    let search_term = completed_search().to_lowercase();
                                                    let filtered: Vec<_> = completed_orders()
                                                        .into_iter()
                                                        .filter(|order| search_term.is_empty() || order.id.to_lowercase().contains(&search_term))
                                                        .collect();

                                                    if filtered.is_empty() {
                                                        rsx! {
                                                            div { class: "empty-state",
                                                                p { if completed_orders().is_empty() { "No completed orders found." } else { "No orders match your search." } }
                                                            }
                                                        }
                                                    } else {
                                                        rsx! {
                                                            for order in filtered {
                                                                {
                                                                    let order_id = order.id.clone();
                                                                    let is_collected = order.status == Some(3);
                                                                    let app_state_mark = app_state.clone();
                                                                    rsx! {
                                                                        div { class: "completed-order-row", key: "{order_id}",
                                                                            div { class: "job-info",
                                                                                div { class: "job-id", "Order #{order_id}" }
                                                                                div { class: "job-meta",
                                                                                    if is_collected { "Collected" } else { "Ready for pickup" }
                                                                                }
                                                                            }
                                                                            div { class: "job-actions",
                                                                                if !is_collected {
                                                                                    button {
                                                                                        class: "btn btn-primary btn-sm",
                                                                                        onclick: {
                                                                                            let state = app_state_mark.clone();
                                                                                            let o_id = order_id.clone();
                                                                                            move |_| {
                                                                                                log::info!("Mark Collected clicked for {}", o_id);
                                                                                                let state = state.clone();
                                                                                                let o_id = o_id.clone();
                                                                                                spawn(async move {
                                                                                                    if mark_order_collected(o_id, state.clone()).await.is_ok() {
                                                                                                        if let Ok(orders) = get_completed_orders(state).await {
                                                                                                            completed_orders.set(orders);
                                                                                                        }
                                                                                                    }
                                                                                                });
                                                                                            }
                                                                                        },
                                                                                        "Mark Collected"
                                                                                    }
                                                                                } else {
                                                                                    span { style: "color: #10b981; font-size: 0.85rem; font-weight: 600;", "✓ Collected" }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Tab::Settings => {
                        let app_state_settings_btn = app_state.clone();
                        rsx! {
                            div { class: "page-view active",
                                section { class: "section-jobs",
                                    div { class: "section-header",
                                        h2 { "Printer Settings" }
                                        div { style: "display: flex; gap: 0.5rem; align-items: center;",
                                            button {
                                                class: "btn btn-primary btn-sm",
                                                onclick: move |_| {
                                                    log::info!("Add New Printer button clicked");
                                                    add_status_msg.set(String::new());
                                                    show_add_modal.set(true);
                                                },
                                                "+ Add New Printer"
                                            }
                                            button {
                                                class: "btn btn-primary btn-sm",
                                                onclick: {
                                                    let state = app_state_settings_btn.clone();
                                                    move |_| {
                                                        log::info!("Refresh Printer Settings clicked");
                                                        let state = state.clone();
                                                        spawn(async move {
                                                            if let Ok(list) = get_printer_list(state).await {
                                                                printers.set(list);
                                                            }
                                                        });
                                                    }
                                                },
                                                "Refresh"
                                            }
                                        }
                                    }
                                    div { class: "jobs-list-container",
                                        div { class: "jobs-list",
                                            if printers().is_empty() {
                                                div { class: "empty-state", p { "No printers found" } }
                                            } else {
                                                for p in printers() {
                                                    {
                                                        let is_paused = p.paused;
                                                        let uri = p.uri.clone();
                                                        let color_label = if p.color_mode == crate::types::ColorMode::Color { "Color" } else { "Monochrome" };
                                                        let app_state_toggle = app_state.clone();
                                                        let printer_obj = p.clone();

                                                        rsx! {
                                                            div { class: "printer-card", key: "{uri}",
                                                                div { class: "printer-card-info",
                                                                    div { class: "printer-card-meta",
                                                                        div { class: "printer-card-name", "{p.name}" }
                                                                        span { class: "pill", "{color_label}" }
                                                                        if is_paused {
                                                                            span { class: "pill", style: "background:#fff3cd;border-color:#ffc107;color:#856404", "Paused" }
                                                                        } else {
                                                                            span { class: "pill", style: "background:#d1fae5;border-color:#6ee7b7;color:#065f46", "Active" }
                                                                        }
                                                                    }
                                                                }
                                                                div { style: "display: flex; gap: 0.5rem; align-items: center;",
                                                                    button {
                                                                        class: "btn-reprint",
                                                                        style: "font-size: 0.75rem; padding: 0.35rem 0.75rem;",
                                                                        onclick: {
                                                                            let printer_obj = printer_obj.clone();
                                                                            let state_props = app_state.clone();
                                                                            move |_| {
                                                                                log::info!("Edit Printer Properties clicked for {}", printer_obj.name);
                                                                                let name = printer_obj.name.clone();
                                                                                let mut printer_to_edit = printer_obj.clone();
                                                                                let state = state_props.clone();
                                                                                spawn(async move {
                                                                                    let (fetched_props, fetched_color) = fetch_printer_properties_from_cups(&name, state).await;
                                                                                    edit_media.set(fetched_props.media.clone());
                                                                                    edit_media_source.set(fetched_props.media_source.clone());
                                                                                    edit_orientation.set(fetched_props.orientation.clone());
                                                                                    edit_print_quality.set(fetched_props.print_quality.clone());
                                                                                    edit_sides.set(fetched_props.sides.clone());
                                                                                    edit_color.set(fetched_color.clone());

                                                                                    printer_to_edit.properties = Some(fetched_props);
                                                                                    printer_to_edit.color_mode = fetched_color;
                                                                                    editing_printer.set(Some(printer_to_edit));
                                                                                });
                                                                            }
                                                                        },
                                                                        "Properties"
                                                                    }
                                                                    button {
                                                                        class: if is_paused { "printer-toggle-btn paused" } else { "printer-toggle-btn active" },
                                                                        onclick: {
                                                                            let state = app_state_toggle.clone();
                                                                            let target_uri = uri.clone();
                                                                            move |_| {
                                                                                log::info!("Toggle printer pause clicked for {}", target_uri);
                                                                                let state = state.clone();
                                                                                let target_uri = target_uri.clone();
                                                                                spawn(async move {
                                                                                    if is_paused {
                                                                                        let _ = unpause_printer(target_uri, state.clone()).await;
                                                                                    } else {
                                                                                        let _ = pause_printer(target_uri, state.clone()).await;
                                                                                    }
                                                                                    if let Ok(list) = get_printer_list(state).await {
                                                                                        printers.set(list);
                                                                                    }
                                                                                });
                                                                            }
                                                                        },
                                                                        if is_paused { "Resume" } else { "Pause" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }}
            }

            // Modal Dialog: Add New AppSocket Printer
            if show_add_modal() {
                div { class: "modal-backdrop",
                    onclick: move |_| show_add_modal.set(false),
                    div { class: "modal-content",
                        onclick: move |e| e.stop_propagation(),
                        div { class: "modal-header",
                            h3 { "Add New AppSocket Printer" }
                            button {
                                class: "modal-close-btn",
                                onclick: move |_| show_add_modal.set(false),
                                "×"
                            }
                        }
                        div { class: "modal-body",
                            if !add_status_msg().is_empty() {
                                div { style: "color: var(--destructive); font-size: 0.8rem; font-weight: 500;", "{add_status_msg}" }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Printer Name (CUPS Identifier)" }
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "e.g. office_jet_9100",
                                    value: new_name(),
                                    oninput: move |e: Event<FormData>| new_name.set(e.value())
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Printer IP Address / Host" }
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "e.g. 192.168.1.100",
                                    value: new_ip(),
                                    oninput: move |e: Event<FormData>| new_ip.set(e.value())
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "AppSocket Port (Default: 9100)" }
                                input {
                                    class: "form-input",
                                    r#type: "number",
                                    placeholder: "9100",
                                    value: new_port(),
                                    oninput: move |e: Event<FormData>| new_port.set(e.value())
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Color Capability" }
                                select {
                                    class: "custom-select",
                                    value: if new_color() == crate::types::ColorMode::Color { "color" } else { "monochrome" },
                                    onchange: move |e: Event<FormData>| {
                                        if e.value() == "color" {
                                            new_color.set(crate::types::ColorMode::Color);
                                        } else {
                                            new_color.set(crate::types::ColorMode::Monochrome);
                                        }
                                    },
                                    option { value: "color", "Color Printer" }
                                    option { value: "monochrome", "Monochrome (B&W) Printer" }
                                }
                            }
                        }
                        div { class: "modal-footer",
                            button {
                                class: "btn-reprint",
                                onclick: move |_| show_add_modal.set(false),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: {
                                    let state = app_state.clone();
                                    move |_| {
                                        let name = new_name();
                                        let ip = new_ip();
                                        let port = new_port().parse::<u16>().unwrap_or(9100);
                                        let color = new_color();
                                        let state = state.clone();

                                        spawn(async move {
                                            match add_appsocket_printer(name, ip, port, color, state.clone()).await {
                                                Ok(_) => {
                                                    show_add_modal.set(false);
                                                    new_name.set(String::new());
                                                    new_ip.set(String::new());
                                                    new_port.set("9100".to_string());
                                                    if let Ok(list) = get_printer_list(state).await {
                                                        printers.set(list);
                                                    }
                                                }
                                                Err(err) => {
                                                    add_status_msg.set(err);
                                                }
                                            }
                                        });
                                    }
                                },
                                "Add Printer"
                            }
                        }
                    }
                }
            }

            // Modal Dialog: Edit Printer Properties
            if let Some(target_p) = editing_printer() {
                div { class: "modal-backdrop",
                    onclick: move |_| editing_printer.set(None),
                    div { class: "modal-content",
                        onclick: move |e| e.stop_propagation(),
                        div { class: "modal-header",
                            h3 { "Printer Properties — {target_p.name}" }
                            button {
                                class: "modal-close-btn",
                                onclick: move |_| editing_printer.set(None),
                                "×"
                            }
                        }
                        div { class: "modal-body",
                            div { class: "form-group",
                                label { class: "form-label", "Default Paper Size (Media)" }
                                select {
                                    class: "custom-select",
                                    value: edit_media(),
                                    onchange: move |e: Event<FormData>| edit_media.set(e.value()),
                                    option { value: "iso_a4_210x297mm", "A4 (210 x 297 mm)" }
                                    option { value: "na_letter_8.5x11in", "US Letter (8.5 x 11 in)" }
                                    option { value: "iso_a3_297x420mm", "A3 (297 x 420 mm)" }
                                    option { value: "na_legal_8.5x14in", "US Legal (8.5 x 14 in)" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Default Input Tray / Media Source" }
                                select {
                                    class: "custom-select",
                                    value: edit_media_source(),
                                    onchange: move |e: Event<FormData>| edit_media_source.set(e.value()),
                                    option { value: "auto", "Auto Select" }
                                    option { value: "main", "Main Tray" }
                                    option { value: "top", "Top Tray" }
                                    option { value: "bottom", "Bottom Tray" }
                                    option { value: "tray-1", "Tray 1" }
                                    option { value: "tray-2", "Tray 2" }
                                    option { value: "manual", "Manual Feed" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Default Orientation" }
                                select {
                                    class: "custom-select",
                                    value: edit_orientation(),
                                    onchange: move |e: Event<FormData>| edit_orientation.set(e.value()),
                                    option { value: "portrait", "Portrait" }
                                    option { value: "landscape", "Landscape" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Default Duplex / Sides" }
                                select {
                                    class: "custom-select",
                                    value: edit_sides(),
                                    onchange: move |e: Event<FormData>| edit_sides.set(e.value()),
                                    option { value: "one-sided", "Single-Sided (Simplex)" }
                                    option { value: "two-sided-long-edge", "Two-Sided (Long Edge)" }
                                    option { value: "two-sided-short-edge", "Two-Sided (Short Edge)" }
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Color Mode" }
                                select {
                                    class: "custom-select",
                                    value: if edit_color() == crate::types::ColorMode::Color { "color" } else { "monochrome" },
                                    onchange: move |e: Event<FormData>| {
                                        if e.value() == "color" {
                                            edit_color.set(crate::types::ColorMode::Color);
                                        } else {
                                            edit_color.set(crate::types::ColorMode::Monochrome);
                                        }
                                    },
                                    option { value: "color", "Color" }
                                    option { value: "monochrome", "Monochrome (B&W)" }
                                }
                            }
                        }
                        div { class: "modal-footer",
                            button {
                                class: "btn-reprint",
                                onclick: move |_| editing_printer.set(None),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: {
                                    let state = app_state.clone();
                                    let target_name = target_p.name.clone();
                                    move |_| {
                                        let props = crate::types::PrinterProperties {
                                            media: edit_media(),
                                            media_source: edit_media_source(),
                                            orientation: edit_orientation(),
                                            print_quality: edit_print_quality(),
                                            sides: edit_sides(),
                                        };
                                        let color = edit_color();
                                        let target_name = target_name.clone();
                                        let state = state.clone();

                                        spawn(async move {
                                            let _ = save_printer_properties(target_name, props, color, state.clone()).await;
                                            editing_printer.set(None);
                                            if let Ok(list) = get_printer_list(state).await {
                                                printers.set(list);
                                            }
                                        });
                                    }
                                },
                                "Save Properties"
                            }
                        }
                    }
                }
            }

            // Bottom Navigation Footer Bar
            footer { class: "app-footer",
                div { class: "bottom-nav",
                    button {
                        class: if active_tab() == Tab::Jobs { "nav-tab active" } else { "nav-tab" },
                        onclick: move |_| {
                            log::info!("Tab switch: Jobs clicked");
                            active_tab.set(Tab::Jobs);
                        },
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            path { d: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" }
                            polyline { points: "14 2 14 8 20 8" }
                            line { x1: "16", y1: "13", x2: "8", y2: "13" }
                            line { x1: "16", y1: "17", x2: "8", y2: "17" }
                            polyline { points: "10 9 9 9 8 9" }
                        }
                        "Active Jobs"
                    }
                    button {
                        class: if active_tab() == Tab::Stats { "nav-tab active" } else { "nav-tab" },
                        onclick: move |_| {
                            log::info!("Tab switch: Stats clicked");
                            active_tab.set(Tab::Stats);
                        },
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            line { x1: "18", y1: "20", x2: "18", y2: "10" }
                            line { x1: "12", y1: "20", x2: "12", y2: "4" }
                            line { x1: "6", y1: "20", x2: "6", y2: "14" }
                        }
                        "Statistics"
                    }
                    button {
                        class: if active_tab() == Tab::Completed { "nav-tab active" } else { "nav-tab" },
                        onclick: move |_| {
                            log::info!("Tab switch: Completed clicked");
                            active_tab.set(Tab::Completed);
                        },
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            polyline { points: "9 11 12 14 22 4" }
                            path { d: "M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" }
                        }
                        "Completed"
                    }
                    button {
                        class: if active_tab() == Tab::Settings { "nav-tab active" } else { "nav-tab" },
                        onclick: move |_| {
                            log::info!("Tab switch: Settings clicked");
                            active_tab.set(Tab::Settings);
                        },
                        svg {
                            width: "16",
                            height: "16",
                            view_box: "0 0 24 24",
                            fill: "none",
                            stroke: "currentColor",
                            stroke_width: "2",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            circle { cx: "12", cy: "12", r: "3" }
                            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
                        }
                        "Settings"
                    }
                }
            }
        }
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

    log::info!("printf dioxus client started");

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
                cups_username: None,
                cups_password: None,
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

    let icon_bytes = include_bytes!("../icons/icon.png");
    let window_icon = match image::load_from_memory(icon_bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
        }
        Err(e) => {
            log::warn!("Failed to load app icon from icons/icon.png: {}", e);
            None
        }
    };

    let mut window_builder = WindowBuilder::new()
        .with_title("printf")
        .with_decorations(true)
        .with_transparent(false)
        .with_maximized(true)
        .with_resizable(true);

    if let Some(icon) = window_icon {
        window_builder = window_builder.with_window_icon(Some(icon));
    }

    let desktop_config = DesktopConfig::new()
        .with_window(window_builder)
        .with_menu(None);

    dioxus::LaunchBuilder::desktop()
        .with_cfg(desktop_config)
        .with_context(app_state)
        .launch(App);
}
