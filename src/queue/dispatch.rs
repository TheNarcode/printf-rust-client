use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::config::get_cups_creds;
use crate::printer::manager::PrinterManager;
use crate::queue::messages::{ack_cf_queue_messages, parse_message_body, pull_cf_queue_messages};
use crate::state::{AppState, current_timestamp};
use crate::types::{CfLeaseId, Config, JobInfo, PrintAttributes};

// ─── Job status helpers ────────────────────────────────────────────────────────

/// Updates (or inserts) a job's status in the shared job store.
///
/// Preserves any existing `lease_id` and `ipp_job_id` already recorded for the job.
pub async fn update_job_status(
    job_store: &Arc<Mutex<HashMap<String, JobInfo>>>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
) {
    let mut store = job_store.lock().await;
    let (existing_lease, existing_ipp_job_id) = store
        .get(&attributes.file_id)
        .map(|i| (i.lease_id.clone(), i.ipp_job_id))
        .unwrap_or((None, None));

    store.insert(
        attributes.file_id.clone(),
        JobInfo {
            file_id: attributes.file_id.clone(),
            order_id,
            attributes: attributes.clone(),
            status: status.to_string(),
            updated_at: current_timestamp(),
            lease_id: existing_lease,
            ipp_job_id: existing_ipp_job_id,
        },
    );

    log::info!("Job {} → {}", attributes.file_id, status);
}

/// Like `update_job_status` but also sets (or updates) the CF Queue lease ID.
pub async fn update_job_status_with_lease(
    job_store: &Arc<Mutex<HashMap<String, JobInfo>>>,
    attributes: &PrintAttributes,
    order_id: Option<String>,
    status: &str,
    lease_id: Option<String>,
) {
    let mut store = job_store.lock().await;
    let (existing_lease, existing_ipp_job_id) = store
        .get(&attributes.file_id)
        .map(|i| (i.lease_id.clone(), i.ipp_job_id))
        .unwrap_or((None, None));

    store.insert(
        attributes.file_id.clone(),
        JobInfo {
            file_id: attributes.file_id.clone(),
            order_id,
            attributes: attributes.clone(),
            status: status.to_string(),
            updated_at: current_timestamp(),
            lease_id: lease_id.or(existing_lease),
            ipp_job_id: existing_ipp_job_id,
        },
    );

    log::info!("Job {} → {} (lease updated)", attributes.file_id, status);
}

// ─── Job dispatch ──────────────────────────────────────────────────────────────

/// Dispatches a batch of `PrintAttributes` to the appropriate printers.
///
/// Returns `true` if every job in the batch completed successfully, `false` if any failed.
/// This result is used to decide whether to ACK or retry the originating CF Queue message.
pub async fn dispatch_job_batch(
    attributes_list: Vec<PrintAttributes>,
    pm: Arc<Mutex<Option<PrinterManager>>>,
    config: Arc<Config>,
    http_client: Arc<reqwest::Client>,
    job_store: Arc<Mutex<HashMap<String, JobInfo>>>,
) -> bool {
    let has_color = attributes_list
        .iter()
        .any(|a| a.color == crate::types::ColorMode::Color);
    let has_mono = attributes_list
        .iter()
        .any(|a| a.color == crate::types::ColorMode::Monochrome);

    // Select printers and resolve their configured media sources in one lock acquisition
    let (color_printer, mono_printer, color_media_source, mono_media_source) = {
        let mut pm_guard = pm.lock().await;
        match pm_guard.as_mut() {
            Some(pm_ref) => pm_ref.get_printers_for_order(has_color, has_mono),
            None => (None, None, None, None),
        }
    };

    let order_id: Option<String> = attributes_list.first().and_then(|a| a.order.clone());
    let mut all_succeeded = true;

    for mut attributes in attributes_list {
        let is_color = attributes.color == crate::types::ColorMode::Color;

        // Resolve which printer to use for this job
        let printer = if let Some(ref target) = attributes.target_printer.clone() {
            // A specific printer was requested (requeue/reprint path)
            let pm_lock = pm.lock().await;
            match pm_lock
                .as_ref()
                .and_then(|pm| {
                    pm.get_printers()
                        .into_iter()
                        .find(|p| p.name == *target || p.uri == *target)
                })
            {
                Some(found) => found,
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
            // Normal path — pick from the round-robin selection
            match attributes.color {
                crate::types::ColorMode::Color => match &color_printer {
                    Some(p) => p.clone(),
                    None => {
                        log::error!(
                            "No active color printer available for job {}",
                            attributes.file_id
                        );
                        update_job_status(&job_store, &attributes, order_id.clone(), "Failed")
                            .await;
                        all_succeeded = false;
                        continue;
                    }
                },
                crate::types::ColorMode::Monochrome => match &mono_printer {
                    Some(p) => p.clone(),
                    None => {
                        log::error!(
                            "No active monochrome printer available for job {}",
                            attributes.file_id
                        );
                        update_job_status(&job_store, &attributes, order_id.clone(), "Failed")
                            .await;
                        all_succeeded = false;
                        continue;
                    }
                },
            }
        };

        let media_source = if is_color {
            color_media_source.clone()
        } else {
            mono_media_source.clone()
        };

        attributes.target_printer = Some(printer.name.clone());
        update_job_status(&job_store, &attributes, order_id.clone(), "Processing").await;
        log::info!(
            "Dispatching job {} → printer {} ({})",
            attributes.file_id, printer.name, printer.uri
        );

        let result = match printer.uri.parse() {
            Ok(uri) => {
                crate::printer::client::print_job(
                    uri,
                    printer.name.clone(),
                    attributes.clone(),
                    media_source,
                    Arc::clone(&config),
                    Arc::clone(&http_client),
                    Arc::clone(&job_store),
                )
                .await
            }
            Err(e) => Err(format!(
                "Invalid printer URI '{}': {}",
                printer.uri, e
            )
            .into()),
        };

        match result {
            Ok(()) => {
                log::info!("Job {} completed successfully", attributes.file_id);
                update_job_status(&job_store, &attributes, order_id.clone(), "Completed").await;
            }
            Err(e) => {
                all_succeeded = false;
                let err_str = e.to_string();
                log::error!("Job {} failed: {}", attributes.file_id, err_str);
                let new_status = if err_str.contains("PendingTimeout") {
                    "Stuck"
                } else {
                    "Failed"
                };
                update_job_status(&job_store, &attributes, order_id.clone(), new_status).await;
            }
        }
    }

    all_succeeded
}

// ─── Client lifecycle ──────────────────────────────────────────────────────────

/// Starts the Cloudflare Queue polling loop in a background `tokio` task.
///
/// On start:
/// 1. Initialises (or re-initialises) the printer manager via IPP, preserving paused state.
/// 2. Enters a `tokio::select!` loop polling the queue every 2 seconds.
/// 3. Applies exponential backoff (doubling up to 60 s) on repeated API errors.
/// 4. Exits cleanly when the cancel channel fires or `is_running` is set to `false`.
pub async fn start_client(state: Arc<AppState>) -> Result<String, String> {
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
    let pm = Arc::clone(&state.printer_manager);
    let state_init = Arc::clone(&state);

    log::info!("Starting printf background client loop");

    tokio::spawn(async move {
        // ── Printer manager initialisation ──────────────────────────────────
        let creds = get_cups_creds(&state_init.config);
        match crate::printer::client::get_ipp_printers(creds).await {
            Ok(printers) => {
                let mut pm_lock = pm.lock().await;
                if pm_lock.is_none() {
                    *pm_lock = Some(PrinterManager::new(printers));
                } else {
                    // Re-initialise but preserve paused state across restarts
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
                log::info!("Printer manager initialised");
            }
            Err(e) => {
                log::warn!(
                    "Failed to enumerate IPP printers: {} — starting with empty printer manager",
                    e
                );
                let mut pm_lock = pm.lock().await;
                if pm_lock.is_none() {
                    *pm_lock = Some(PrinterManager::new(Vec::new()));
                }
            }
        }

        let cf_account_id = config.cf_account_id.clone();
        let cf_queue_id = config.cf_queue_id.clone();
        let cf_token = config
            .cf_api_token
            .clone()
            .or_else(|| config.printf_key.clone());

        let mut backoff = Duration::from_secs(2);

        // ── Poll loop ────────────────────────────────────────────────────────
        loop {
            if !is_running_flag.load(Ordering::SeqCst) {
                log::info!("Stop flag set — exiting poll loop");
                break;
            }

            if cf_account_id.is_none() || cf_queue_id.is_none() || cf_token.is_none() {
                log::warn!(
                    "CF Queue configuration incomplete (cf_account_id / cf_queue_id / token) — \
                     waiting 5 s before retry"
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = &mut rx => {
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
                    log::info!("Received cancel signal — stopping client");
                    is_running_flag.store(false, Ordering::SeqCst);
                    break;
                }
                pull_res = pull_cf_queue_messages(&http_client, account_id, queue_id, token) => {
                    match pull_res {
                        Ok(messages) => {
                            backoff = Duration::from_secs(2); // Reset backoff on success

                            if messages.is_empty() {
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                                    _ = &mut rx => {
                                        is_running_flag.store(false, Ordering::SeqCst);
                                        break;
                                    }
                                }
                                continue;
                            }

                            log::info!("Pulled {} message(s) from CF Queue", messages.len());

                            for msg in messages {
                                let http_client = Arc::clone(&http_client);
                                let account_id = account_id.clone();
                                let queue_id  = queue_id.clone();
                                let token     = token.clone();
                                let pm        = Arc::clone(&pm);
                                let config    = Arc::clone(&config);
                                let job_store = Arc::clone(&job_store);

                                tokio::spawn(async move {
                                    match parse_message_body(&msg.body) {
                                        Ok(attributes_list) => {
                                            // ── Deduplication guard ─────────────────────────────
                                            // If a job with the same file_id is already queued or
                                            // processing (e.g. due to a duplicate CF Queue delivery),
                                            // skip re-dispatching it to prevent double prints.
                                            let mut to_dispatch = Vec::new();
                                            {
                                                let store = job_store.lock().await;
                                                for attr in &attributes_list {
                                                    if let Some(existing) = store.get(&attr.file_id) {
                                                        let s = existing.status.to_lowercase();
                                                        if s == "queued" || s == "processing" {
                                                            log::warn!(
                                                                "Skipping duplicate message for \
                                                                 file {} (currently {})",
                                                                attr.file_id, existing.status
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                    to_dispatch.push(attr.clone());
                                                }
                                            }

                                            if to_dispatch.is_empty() {
                                                // All items were duplicates — ACK to clean up the queue
                                                log::info!(
                                                    "All items in message {} were duplicates; ACKing",
                                                    msg.id
                                                );
                                                let acks = vec![CfLeaseId {
                                                    lease_id: msg.lease_id,
                                                    delay_seconds: None,
                                                }];
                                                if let Err(e) = ack_cf_queue_messages(
                                                    &http_client, &account_id, &queue_id, &token,
                                                    acks, vec![],
                                                ).await {
                                                    log::error!(
                                                        "Failed to ACK deduplicated message {}: {}",
                                                        msg.id, e
                                                    );
                                                }
                                                return;
                                            }

                                            // Mark all jobs as Queued with their lease ID
                                            for attr in &to_dispatch {
                                                update_job_status_with_lease(
                                                    &job_store,
                                                    attr,
                                                    attr.order.clone(),
                                                    "Queued",
                                                    Some(msg.lease_id.clone()),
                                                )
                                                .await;
                                            }

                                            let success = dispatch_job_batch(
                                                to_dispatch,
                                                pm,
                                                config,
                                                Arc::clone(&http_client),
                                                job_store,
                                            )
                                            .await;

                                            let (acks, retries) = if success {
                                                log::info!(
                                                    "All jobs in message {} completed; ACKing",
                                                    msg.id
                                                );
                                                (
                                                    vec![CfLeaseId {
                                                        lease_id: msg.lease_id,
                                                        delay_seconds: None,
                                                    }],
                                                    vec![],
                                                )
                                            } else {
                                                log::warn!(
                                                    "One or more jobs in message {} failed; \
                                                     retrying in 60 s",
                                                    msg.id
                                                );
                                                (
                                                    vec![],
                                                    vec![CfLeaseId {
                                                        lease_id: msg.lease_id,
                                                        delay_seconds: Some(60),
                                                    }],
                                                )
                                            };

                                            if let Err(e) = ack_cf_queue_messages(
                                                &http_client, &account_id, &queue_id, &token,
                                                acks, retries,
                                            )
                                            .await
                                            {
                                                log::error!(
                                                    "Failed to ACK/retry message {}: {}",
                                                    msg.id, e
                                                );
                                            }
                                        }
                                        Err(err) => {
                                            log::error!(
                                                "Could not parse body of message {}: {}",
                                                msg.id, err
                                            );
                                            // ACK unparseable messages so they don't loop forever
                                            let acks = vec![CfLeaseId {
                                                lease_id: msg.lease_id,
                                                delay_seconds: None,
                                            }];
                                            let _ = ack_cf_queue_messages(
                                                &http_client, &account_id, &queue_id, &token,
                                                acks, vec![],
                                            )
                                            .await;
                                        }
                                    }
                                });
                            }
                        }
                        Err(err) => {
                            log::error!(
                                "CF Queue pull error: {} — retrying in {:?}",
                                err, backoff
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => {}
                                _ = &mut rx => {
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

        log::info!("printf client loop exited");
        is_running_flag.store(false, Ordering::SeqCst);
    });

    Ok("Client started".to_string())
}

/// Signals the background polling loop to stop gracefully.
pub async fn stop_client(state: Arc<AppState>) -> Result<String, String> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Ok("Client is not running".to_string());
    }

    state.is_running.store(false, Ordering::SeqCst);
    if let Some(tx) = state.cancel_tx.lock().await.take() {
        let _ = tx.send(());
    }

    log::info!("Stop signal sent to client loop");
    Ok("Client stopped".to_string())
}

/// Returns all tracked jobs from the job store, sorted by most-recently-updated first.
///
/// # Important
/// Jobs are returned regardless of whether the client is currently running.
/// The job store persists for the application lifetime and is always safe to read.
/// This allows operators to inspect completed, failed, or stuck jobs after stopping the client.
pub async fn get_jobs(state: Arc<AppState>) -> Vec<JobInfo> {
    let store = state.job_store.lock().await;
    let mut jobs: Vec<JobInfo> = store
        .values()
        .filter(|j| j.status.to_lowercase() != "completed")
        .cloned()
        .collect();
    drop(store);
    jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    jobs
}

// ─── Job re-dispatch ───────────────────────────────────────────────────────────

/// Re-dispatches an existing job to its last-known (or round-robin selected) printer.
///
/// Used by the "Reprint" button on stuck/failed jobs in the UI.
pub async fn reprint_job(file_id: String, state: Arc<AppState>) -> Result<(), String> {
    let job_info = {
        let store = state.job_store.lock().await;
        store
            .get(&file_id)
            .cloned()
            .ok_or_else(|| format!("Job {} not found in job store", file_id))?
    };

    update_job_status(&state.job_store, &job_info.attributes, job_info.order_id.clone(), "Queued")
        .await;

    let success = dispatch_job_batch(
        vec![job_info.attributes.clone()],
        Arc::clone(&state.printer_manager),
        Arc::clone(&state.config),
        Arc::clone(&state.http_client),
        Arc::clone(&state.job_store),
    )
    .await;

    if success {
        log::info!("Reprint of job {} succeeded", file_id);
        // ACK the original queue message to prevent redelivery
        if let Some(ref lease_id) = job_info.lease_id {
            if let (Some(account_id), Some(queue_id), Some(token)) = (
                state.config.cf_account_id.clone(),
                state.config.cf_queue_id.clone(),
                state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone()),
            ) {
                let acks = vec![CfLeaseId {
                    lease_id: lease_id.clone(),
                    delay_seconds: None,
                }];
                if let Err(e) = ack_cf_queue_messages(
                    &state.http_client, &account_id, &queue_id, &token, acks, vec![],
                )
                .await
                {
                    log::error!("Failed to ACK reprint job {}: {}", file_id, e);
                }
            }
        }
    }

    Ok(())
}

/// Re-queues a stuck job to a different (operator-selected) printer.
///
/// Attempts to cancel the old IPP job first as a best-effort resource cleanup.
/// Cancellation failure is logged but never blocks the requeue — the old job
/// may already be finished or the printer may be unreachable.
pub async fn requeue_to_printer(
    file_id: String,
    new_printer_name: String,
    state: Arc<AppState>,
) -> Result<(), String> {
    let mut job_info = {
        let store = state.job_store.lock().await;
        store
            .get(&file_id)
            .cloned()
            .ok_or_else(|| format!("Job {} not found in job store", file_id))?
    };

    // ── Best-effort: cancel the old CUPS job to free printer resources ──────
    if let (Some(old_job_id), Some(ref old_printer)) = (
        job_info.ipp_job_id,
        job_info.attributes.target_printer.clone(),
    ) {
        let creds = get_cups_creds(&state.config);
        match crate::printer::client::cancel_ipp_job(old_printer, old_job_id, creds).await {
            Ok(()) => {
                log::info!(
                    "Cancelled old CUPS job {} on {} before requeue",
                    old_job_id, old_printer
                );
            }
            Err(e) => {
                // Not fatal — the job may already be completed or the printer offline.
                // We proceed with the requeue regardless.
                log::warn!(
                    "Could not cancel old CUPS job {} on {}: {} — proceeding with requeue",
                    old_job_id, old_printer, e
                );
            }
        }
    }

    job_info.attributes.target_printer = Some(new_printer_name.clone());
    update_job_status(&state.job_store, &job_info.attributes, job_info.order_id.clone(), "Queued")
        .await;

    let success = dispatch_job_batch(
        vec![job_info.attributes.clone()],
        Arc::clone(&state.printer_manager),
        Arc::clone(&state.config),
        Arc::clone(&state.http_client),
        Arc::clone(&state.job_store),
    )
    .await;

    if success {
        log::info!("Requeue of job {} to {} succeeded", file_id, new_printer_name);
        if let Some(ref lease_id) = job_info.lease_id {
            if let (Some(account_id), Some(queue_id), Some(token)) = (
                state.config.cf_account_id.clone(),
                state.config.cf_queue_id.clone(),
                state.config.cf_api_token.clone().or_else(|| state.config.printf_key.clone()),
            ) {
                let acks = vec![CfLeaseId {
                    lease_id: lease_id.clone(),
                    delay_seconds: None,
                }];
                if let Err(e) = ack_cf_queue_messages(
                    &state.http_client, &account_id, &queue_id, &token, acks, vec![],
                )
                .await
                {
                    log::error!("Failed to ACK requeued job {}: {}", file_id, e);
                }
            }
        }
    }

    Ok(())
}
