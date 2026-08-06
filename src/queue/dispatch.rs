use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use crate::printer::client::get_cups_creds;
use crate::printer::manager::PrinterManager;
use crate::queue::messages::{ack_cf_queue_messages, parse_message_body, pull_cf_queue_messages};
use crate::state::{AppState, current_timestamp};
use crate::types::{CfLeaseId, ColorMode, JobInfo, PrintAttributes};

pub async fn persist_job_store(store: &HashMap<String, JobInfo>, path: &std::path::Path) {
    let json = match serde_json::to_string(store) {
        Ok(j) => j,
        Err(e) => { log::error!("Failed to serialize job store: {}", e); return; }
    };
    let tmp = path.with_extension("tmp");
    if let Err(e) = tokio::fs::write(&tmp, json.as_bytes()).await {
        log::error!("Failed to write job store to {}: {}", tmp.display(), e);
        return;
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        log::error!("Failed to atomically commit job store: {}", e);
    }
}

pub fn load_job_store(path: &std::path::Path) -> HashMap<String, JobInfo> {
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(store) => { let s: HashMap<String, JobInfo> = store; log::info!("Loaded {} job(s) from disk", s.len()); s }
            Err(e) => { log::warn!("Failed to parse job store ({}); starting fresh", e); HashMap::new() }
        },
        Err(_) => HashMap::new(),
    }
}

pub async fn update_job_status(
    state: &Arc<AppState>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
) {
    let path = state.job_store_path.clone();
    let snapshot = {
        let mut store = state.job_store.lock().await;
        let (lease, ipp_id) = store.get(&attributes.file_id)
            .map(|i| (i.lease_id.clone(), i.ipp_job_id))
            .unwrap_or((None, None));
        store.insert(attributes.file_id.clone(), JobInfo {
            file_id: attributes.file_id.clone(),
            order_id,
            attributes: attributes.clone(),
            status: status.to_string(),
            updated_at: current_timestamp(),
            lease_id: lease,
            ipp_job_id: ipp_id,
        });
        log::info!("Job {} → {}", attributes.file_id, status);
        store.clone()
    };
    persist_job_store(&snapshot, &path).await;
}

pub async fn update_job_status_with_lease(
    state: &Arc<AppState>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
    lease_id: Option<String>,
) {
    let path = state.job_store_path.clone();
    let snapshot = {
        let mut store = state.job_store.lock().await;
        let (existing_lease, ipp_id) = store.get(&attributes.file_id)
            .map(|i| (i.lease_id.clone(), i.ipp_job_id))
            .unwrap_or((None, None));
        store.insert(attributes.file_id.clone(), JobInfo {
            file_id: attributes.file_id.clone(),
            order_id,
            attributes: attributes.clone(),
            status: status.to_string(),
            updated_at: current_timestamp(),
            lease_id: lease_id.or(existing_lease),
            ipp_job_id: ipp_id,
        });
        log::info!("Job {} → {} (lease updated)", attributes.file_id, status);
        store.clone()
    };
    persist_job_store(&snapshot, &path).await;
}

pub async fn dispatch_job_batch(
    attributes_list: Vec<PrintAttributes>,
    state: Arc<AppState>,
) -> bool {
    let has_color = attributes_list.iter().any(|a| a.color == ColorMode::Color);
    let has_mono  = attributes_list.iter().any(|a| a.color == ColorMode::Monochrome);
    let needs_auto_select = attributes_list.iter().any(|a| a.target_printer.is_none());
    let (color_printer, mono_printer, color_media, mono_media) = if needs_auto_select {
        let mut pm = state.printer_manager.lock().await;
        match pm.as_mut() {
            Some(m) => m.get_printers_for_order(has_color, has_mono),
            None => (None, None, None, None),
        }
    } else {
        (None, None, None, None)
    };

    let order_id: Option<String> = attributes_list.first().and_then(|a| a.order.clone());

    struct Task {
        attributes: PrintAttributes,
        printer_name: String,
        printer_uri: ipp::prelude::Uri,
        media_source: Option<String>,
        order_id: Option<String>,
    }

    let mut tasks: Vec<Task> = Vec::new();
    let mut all_ok = true;

    for mut attributes in attributes_list {
        let is_color = attributes.color == ColorMode::Color;

        let printer = if let Some(ref target) = attributes.target_printer.clone() {
            let pm_lock = state.printer_manager.lock().await;
            match pm_lock.as_ref().and_then(|m| {
                m.get_printers().into_iter().find(|p| &p.name == target || &p.uri == target)
            }) {
                Some(p) => p,
                None => crate::types::Printer {
                    uri: if target.starts_with("http") || target.starts_with("ipp") {
                        target.clone()
                    } else {
                        format!("ipp://localhost:631/printers/{}", target)
                    },
                    name: target.clone(),
                    color_mode: attributes.color.clone(),
                    paused: false,
                    properties: None,
                },
            }
        } else {
            let chosen = if is_color { color_printer.clone() } else { mono_printer.clone() };
            match chosen {
                Some(p) => p,
                None => {
                    let mode = if is_color { "color" } else { "monochrome" };
                    log::error!("No active {} printer for job {}", mode, attributes.file_id);
                    update_job_status(&state, &attributes, order_id.clone(), "Failed").await;
                    all_ok = false;
                    continue;
                }
            }
        };

        let media_source = if is_color { color_media.clone() } else { mono_media.clone() };
        attributes.target_printer = Some(printer.name.clone());

        let uri = match printer.uri.parse::<ipp::prelude::Uri>() {
            Ok(u) => u,
            Err(e) => {
                log::error!("Invalid printer URI '{}': {}", printer.uri, e);
                update_job_status(&state, &attributes, order_id.clone(), "Failed").await;
                all_ok = false;
                continue;
            }
        };

        update_job_status(&state, &attributes, order_id.clone(), "Processing").await;
        log::info!("Dispatching job {} → {} ({})", attributes.file_id, printer.name, printer.uri);

        tasks.push(Task { attributes, printer_name: printer.name, printer_uri: uri, media_source, order_id: order_id.clone() });
    }

    let handles: Vec<_> = tasks.into_iter().map(|task| {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let result = crate::printer::client::print_job(
                task.printer_uri, task.printer_name, task.attributes.clone(), task.media_source, Arc::clone(&state),
            ).await;
            (task.attributes, task.order_id, result)
        })
    }).collect();

    for join_result in futures::future::join_all(handles).await {
        match join_result {
            Ok((attrs, oid, Ok(()))) => {
                log::info!("Job {} completed successfully", attrs.file_id);
                update_job_status(&state, &attrs, oid, "Completed").await;
            }
            Ok((attrs, oid, Err(e))) => {
                all_ok = false;
                let msg = e.to_string();
                log::error!("Job {} failed: {}", attrs.file_id, msg);
                let s = if msg.contains("PendingTimeout") { "Stuck" } else { "Failed" };
                update_job_status(&state, &attrs, oid, s).await;
            }
            Err(e) => {
                all_ok = false;
                log::error!("Print task panicked: {}", e);
            }
        }
    }

    all_ok
}

pub async fn start_client(state: Arc<AppState>) -> Result<String, String> {
    if state.is_running.load(Ordering::SeqCst) {
        return Ok("Client is already running".to_string());
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    *state.cancel_tx.lock().await = Some(tx);
    state.is_running.store(true, Ordering::SeqCst);
    log::info!("Starting printf background client");

    tokio::spawn(async move {
        let creds = get_cups_creds(&state.config);
        match crate::printer::client::get_ipp_printers(creds).await {
            Ok(printers) => {
                let mut pm_lock = state.printer_manager.lock().await;
                if pm_lock.is_none() {
                    *pm_lock = Some(PrinterManager::new(printers));
                } else {
                    let existing = pm_lock.take().unwrap();
                    let paused: Vec<_> = existing.get_printers().iter().filter(|p| p.paused).map(|p| p.uri.clone()).collect();
                    let mut new_pm = PrinterManager::new(printers);
                    for uri in &paused { new_pm.set_printer_paused(uri, true); }
                    *pm_lock = Some(new_pm);
                }
                log::info!("Printer manager initialised");
            }
            Err(e) => {
                log::warn!("Failed to enumerate printers: {} — starting with empty manager", e);
                let mut pm_lock = state.printer_manager.lock().await;
                if pm_lock.is_none() { *pm_lock = Some(PrinterManager::new(Vec::new())); }
            }
        }

        let cf_account_id = state.config.cf_account_id.clone();
        let cf_queue_id   = state.config.cf_queue_id.clone();
        let cf_token = state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone());
        let is_running = Arc::clone(&state.is_running);
        let mut backoff = Duration::from_secs(2);

        loop {
            if !is_running.load(Ordering::SeqCst) { break; }

            if cf_account_id.is_none() || cf_queue_id.is_none() || cf_token.is_none() {
                log::warn!("CF Queue config incomplete — waiting 5s");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = &mut rx => { is_running.store(false, Ordering::SeqCst); break; }
                }
                continue;
            }

            let account_id = cf_account_id.as_deref().unwrap();
            let queue_id   = cf_queue_id.as_deref().unwrap();
            let token      = cf_token.as_deref().unwrap();

            tokio::select! {
                _ = &mut rx => { log::info!("Cancel signal received"); is_running.store(false, Ordering::SeqCst); break; }
                pull_res = pull_cf_queue_messages(&state.http_client, account_id, queue_id, token) => {

                    if !is_running.load(Ordering::SeqCst) { break; }
                    match pull_res {
                        Ok(messages) => {
                            backoff = Duration::from_secs(2);
                            if messages.is_empty() {
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                                    _ = &mut rx => { is_running.store(false, Ordering::SeqCst); break; }
                                }
                                if !is_running.load(Ordering::SeqCst) { break; }
                                continue;
                            }
                            log::info!("Pulled {} message(s) from CF Queue", messages.len());

                            for msg in messages {
                                let state_msg = Arc::clone(&state);
                                tokio::spawn(async move {
                                    let acc   = state_msg.config.cf_account_id.clone().unwrap_or_default();
                                    let qid   = state_msg.config.cf_queue_id.clone().unwrap_or_default();
                                    let tok   = state_msg.config.cf_api_token.clone()
                                        .or_else(|| state_msg.config.printf_key.clone())
                                        .unwrap_or_default();

                                    match parse_message_body(&msg.body) {
                                        Ok(attributes_list) => {
                                            let mut to_dispatch = Vec::new();
                                            {
                                                let store = state_msg.job_store.lock().await;
                                                for attr in &attributes_list {
                                                    if let Some(existing) = store.get(&attr.file_id) {
                                                        let s = existing.status.to_lowercase();
                                                        if s == "queued" || s == "processing" {
                                                            log::warn!("Skipping duplicate for {} ({})", attr.file_id, existing.status);
                                                            continue;
                                                        }
                                                    }
                                                    to_dispatch.push(attr.clone());
                                                }
                                            }

                                            if to_dispatch.is_empty() {
                                                log::info!("All items in message {} were duplicates; ACKing", msg.id);
                                                let acks = vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: None }];
                                                let _ = ack_cf_queue_messages(&state_msg.http_client, &acc, &qid, &tok, acks, vec![]).await;
                                                return;
                                            }

                                            for attr in &to_dispatch {
                                                update_job_status_with_lease(&state_msg, attr, attr.order.clone(), "Queued", Some(msg.lease_id.clone())).await;
                                            }

                                            let success = dispatch_job_batch(to_dispatch, Arc::clone(&state_msg)).await;

                                            let (acks, retries) = if success {
                                                log::info!("Message {} succeeded; ACKing", msg.id);
                                                (vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: None }], vec![])
                                            } else {
                                                log::warn!("Message {} had failures; retrying in 60s", msg.id);
                                                (vec![], vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: Some(60) }])
                                            };

                                            if let Err(e) = ack_cf_queue_messages(&state_msg.http_client, &acc, &qid, &tok, acks, retries).await {
                                                log::error!("Failed to ACK/retry message {}: {}", msg.id, e);
                                            }
                                        }
                                        Err(err) => {
                                            log::error!("Cannot parse message {}: {}", msg.id, err);
                                            let acks = vec![CfLeaseId { lease_id: msg.lease_id, delay_seconds: None }];
                                            let _ = ack_cf_queue_messages(&state_msg.http_client, &acc, &qid, &tok, acks, vec![]).await;
                                        }
                                    }
                                });
                            }
                        }
                        Err(err) => {
                            log::error!("CF Queue pull error: {} — retrying in {:?}", err, backoff);
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut rx => { is_running.store(false, Ordering::SeqCst); break; }
                            }
                            if !is_running.load(Ordering::SeqCst) { break; }
                            backoff = (backoff * 2).min(Duration::from_secs(60));
                        }
                    }
                }
            }
        }

        log::info!("printf client loop exited");
        is_running.store(false, Ordering::SeqCst);
    });

    Ok("Client started".to_string())
}

pub async fn stop_client(state: Arc<AppState>) -> Result<String, String> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Ok("Client is not running".to_string());
    }
    state.is_running.store(false, Ordering::SeqCst);
    if let Some(tx) = state.cancel_tx.lock().await.take() { let _ = tx.send(()); }
    log::info!("Stop signal sent");
    Ok("Client stopped".to_string())
}

pub async fn get_jobs(state: Arc<AppState>) -> Vec<JobInfo> {
    let now = crate::state::current_timestamp_secs();
    let grace_secs: u64 = 60;
    let store = state.job_store.lock().await;
    let mut jobs: Vec<JobInfo> = store.values()
        .filter(|j| {
            let s = j.status.to_lowercase();
            if s != "completed" {
                return true;
            }
            let updated = j.updated_at.parse::<u64>().unwrap_or(0);
            now.saturating_sub(updated) < grace_secs
        })
        .cloned()
        .collect();
    drop(store);
    jobs.sort_by(|a, b| {
        let ta = b.updated_at.parse::<u64>().unwrap_or(0);
        let tb = a.updated_at.parse::<u64>().unwrap_or(0);
        ta.cmp(&tb)
    });
    jobs
}

pub async fn get_completed_jobs_today(state: Arc<AppState>) -> Vec<JobInfo> {
    let now = crate::state::current_timestamp_secs();
    let today_start: u64 = (now / 86_400) * 86_400;
    let store = state.job_store.lock().await;
    let mut jobs: Vec<JobInfo> = store.values()
        .filter(|j| {
            if j.status.to_lowercase() != "completed" { return false; }
            let updated = j.updated_at.parse::<u64>().unwrap_or(0);
            updated >= today_start
        })
        .cloned()
        .collect();
    drop(store);

    jobs.sort_by(|a, b| {
        b.updated_at.parse::<u64>().unwrap_or(0)
            .cmp(&a.updated_at.parse::<u64>().unwrap_or(0))
    });
    jobs
}

pub async fn reprint_job(file_id: String, state: Arc<AppState>) -> Result<(), String> {
    let job_info = {
        let store = state.job_store.lock().await;
        store.get(&file_id).cloned().ok_or_else(|| format!("Job {} not found", file_id))?
    };

    let mut attributes = job_info.attributes.clone();
    attributes.target_printer = None;
    update_job_status(&state, &attributes, job_info.order_id.clone(), "Queued").await;
    let success = dispatch_job_batch(vec![attributes], Arc::clone(&state)).await;
    ack_lease_if_present(&job_info.lease_id, &state).await;

    if !success { log::warn!("Reprint of {} failed — operator action required", file_id); }
    Ok(())
}

pub async fn requeue_to_printer(
    file_id: String,
    new_printer_name: String,
    state: Arc<AppState>,
) -> Result<(), String> {
    let mut job_info = {
        let store = state.job_store.lock().await;
        store.get(&file_id).cloned().ok_or_else(|| format!("Job {} not found", file_id))?
    };

    if let (Some(old_job_id), Some(ref old_printer)) = (job_info.ipp_job_id, job_info.attributes.target_printer.clone()) {
        let creds = get_cups_creds(&state.config);
        if let Err(e) = crate::printer::client::cancel_ipp_job(old_printer, old_job_id, creds).await {
            log::warn!("Could not cancel old CUPS job {} on {}: {}", old_job_id, old_printer, e);
        }
    }

    job_info.attributes.target_printer = Some(new_printer_name.clone());
    update_job_status(&state, &job_info.attributes, job_info.order_id.clone(), "Queued").await;
    let success = dispatch_job_batch(vec![job_info.attributes.clone()], Arc::clone(&state)).await;

    ack_lease_if_present(&job_info.lease_id, &state).await;

    if !success { log::warn!("Requeue of {} to {} failed — operator action required", file_id, new_printer_name); }
    Ok(())
}

async fn ack_lease_if_present(lease_id: &Option<String>, state: &Arc<AppState>) {
    let Some(lid) = lease_id else { return; };
    let (acc, qid, tok) = (
        state.config.cf_account_id.clone(),
        state.config.cf_queue_id.clone(),
        state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone()),
    );
    if let (Some(acc), Some(qid), Some(tok)) = (acc, qid, tok) {
        let acks = vec![CfLeaseId { lease_id: lid.clone(), delay_seconds: None }];
        if let Err(e) = ack_cf_queue_messages(&state.http_client, &acc, &qid, &tok, acks, vec![]).await {
            log::error!("Failed to ACK lease {}: {}", lid, e);
        }
    }
}