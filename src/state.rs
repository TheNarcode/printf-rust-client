use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use crate::printer::manager::PrinterManager;
use crate::types::{Config, JobInfo};

pub struct AppState {
    pub config: Arc<Config>,
    pub http_client: Arc<reqwest::Client>,
    pub is_running: Arc<AtomicBool>,
    pub cancel_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub printer_manager: Arc<Mutex<Option<PrinterManager>>>,
    pub job_store: Arc<Mutex<HashMap<String, JobInfo>>>,
    pub job_store_path: std::path::PathBuf,
}

#[inline]
pub fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[inline]
pub fn current_timestamp() -> String {
    current_timestamp_secs().to_string()
}