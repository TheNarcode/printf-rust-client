use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use crate::printer::manager::PrinterManager;
use crate::types::{Config, JobInfo};

/// Central shared state. Wrapped in `Arc` and distributed to all async tasks and
/// Dioxus components via context injection at launch time.
pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: Arc<reqwest::Client>,
    /// Atomic flag — the single source of truth for whether the queue polling loop is active.
    pub is_running: Arc<AtomicBool>,
    /// One-shot channel sender used to signal the polling loop to stop gracefully.
    pub cancel_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The printer manager — `None` until the first `start_client` call.
    pub printer_manager: Arc<Mutex<Option<PrinterManager>>>,
    /// In-memory store of all tracked print jobs keyed by `file_id`.
    pub job_store: Arc<Mutex<HashMap<String, JobInfo>>>,
    /// Path to the on-disk JSON snapshot of `job_store`.
    /// The store is atomically written here on every status transition so that
    /// in-progress jobs survive an app restart and CF Queue re-deliveries cannot
    /// cause double-prints.
    pub job_store_path: std::path::PathBuf,
}

/// Returns the current Unix time in whole seconds.
#[inline]
pub fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Returns the current Unix timestamp as a decimal string (used as `updated_at` in `JobInfo`).
#[inline]
pub fn current_timestamp() -> String {
    current_timestamp_secs().to_string()
}
