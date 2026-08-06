#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use dioxus::desktop::{Config as DesktopConfig, WindowBuilder};
use ftail::Ftail;
use log::LevelFilter;
use tokio::sync::Mutex;
use crate::config::{get_config_path, read_config};
use crate::constants::BASE_URL;
use crate::state::AppState;
use crate::types::Config;
use crate::ui::App;

pub mod api;
pub mod config;
pub mod constants;
pub mod printer;
pub mod queue;
pub mod state;
pub mod types;
pub mod ui;

fn main() {
    let logs_dir = dirs::data_local_dir()
        .expect("Cannot determine local data dir")
        .join("printf")
        .join("logs");

    let config_path = get_config_path().expect("Cannot determine config path");

    fs::create_dir_all(&logs_dir).expect("Failed to create logs directory");
    fs::create_dir_all(config_path.parent().unwrap())
        .expect("Failed to create config directory");

    Ftail::new()
        .console(LevelFilter::Info)
        .daily_file(&logs_dir, LevelFilter::Info)
        .init()
        .expect("Failed to initialise logging");

    log::info!("printf dioxus client starting (v{})", env!("CARGO_PKG_VERSION"));

    let config = match read_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log::warn!("Failed to read config ({}); using built-in defaults", e);
            Arc::new(Config {
                s3_base_url:   "http://localhost:8000/".to_string(),
                webhook_url:   None,
                printf_key:    None,
                base_url:      BASE_URL.to_string(),
                cf_account_id: None,
                cf_queue_id:   None,
                cf_api_token:  None,
                cups_username: None,
                cups_password: None,
            })
        }
    };

    let http_client = Arc::new(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client"),
    );

    let job_store_path = dirs::data_local_dir()
        .expect("Cannot determine local data dir")
        .join("printf")
        .join("jobs.json");

    if let Some(parent) = job_store_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let initial_jobs = crate::queue::dispatch::load_job_store(&job_store_path);

    let app_state = Arc::new(AppState {
        config,
        http_client,
        is_running:      Arc::new(AtomicBool::new(false)),
        cancel_tx:       Mutex::new(None),
        printer_manager: Arc::new(Mutex::new(None)),
        job_store:       Arc::new(Mutex::new(initial_jobs)),
        job_store_path,
    });

    let icon_bytes = include_bytes!("../icons/icon.png");
    let window_icon = match image::load_from_memory(icon_bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
        }
        Err(e) => {
            log::warn!("Failed to load app icon: {}", e);
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