use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{Config, PrintAttributes};
use ftail::Ftail;
use log::LevelFilter;
use redis::AsyncCommands;
use tokio::sync::Mutex;

pub mod ipp;
pub mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let logs_dir = dirs::data_local_dir().unwrap().join("printf").join("logs");
    let config_path = get_config_path()?;

    fs::create_dir_all(&logs_dir)?;
    fs::create_dir_all(config_path.parent().unwrap())?;

    Ftail::new()
        .console(LevelFilter::Info)
        .daily_file(&logs_dir, LevelFilter::Info)
        .init()?;

    log::info!("printf client started");

    let config = Arc::new(read_config()?);
    let http_client = Arc::new(reqwest::Client::new());

    let printers = get_ipp_printers().await?;
    let pm = Arc::new(Mutex::new(PrinterManager::new(printers)));

    log::info!("printer manager initialized");

    let redis_client = redis::Client::open(config.redis_url.as_str())?;

    let mut reconnect_delay = Duration::from_secs(5);
    let mut first_connect = true;

    loop {
        let mut con = match redis_client.get_multiplexed_async_connection().await {
            Ok(c) => {
                if first_connect {
                    log::info!("connected to redis");
                } else {
                    log::info!("reconnected to redis");
                }
                first_connect = false;
                reconnect_delay = Duration::from_secs(5);
                c
            }
            Err(e) => {
                log::error!("failed to connect to redis: {}", e);
                tokio::time::sleep(reconnect_delay).await;
                reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(60));
                continue;
            }
        };

        loop {
            match con.brpop::<_, Option<(String, String)>>("printf_queue", 0.0).await {
                Ok(Some((_key, data))) => {
                    log::info!("got new print command from queue");

                    let attributes_list: Vec<PrintAttributes> = match serde_json::from_str(&data) {
                        Ok(list) => list,
                        Err(err) => {
                            log::error!("failed to parse print attributes: {}", err);
                            continue;
                        }
                    };

                    for attributes in attributes_list {
                        let printer = {
                            let mut pm_guard = pm.lock().await;
                            match pm_guard.get_printer(&attributes.color) {
                                Some(p) => p,
                                None => {
                                    log::error!(
                                        "no printer found for color mode: {:?}",
                                        attributes.color
                                    );
                                    continue;
                                }
                            }
                        };

                        let config = Arc::clone(&config);
                        let http_client = Arc::clone(&http_client);
                        let redis_client = redis_client.clone();

                        tokio::spawn(async move {
                            log::info!("using printer {} for print", printer.uri);

                            let failed = match printer.uri.parse() {
                                Ok(uri) => match print_job(uri, attributes.clone(), config, http_client).await {
                                    Ok(_) => { log::info!("print job successful"); false }
                                    Err(e) => { log::error!("print job failed: {}", e); true }
                                },
                                Err(e) => { log::error!("failed to parse printer URI: {}", e); true }
                            };

                            if failed {
                                match serde_json::to_string(&[&attributes]) {
                                    Ok(payload) => {
                                        match redis_client.get_multiplexed_async_connection().await {
                                            Ok(mut con) => {
                                                match con.lpush::<_, _, ()>("printf_queue", payload).await {
                                                    Ok(_) => log::info!("re-queued failed job for file: {}", attributes.file_id),
                                                    Err(e) => log::error!("failed to re-queue job: {}", e),
                                                }
                                            }
                                            Err(e) => log::error!("failed to connect to redis for re-queue: {}", e),
                                        }
                                    }
                                    Err(e) => log::error!("failed to serialize job for re-queue: {}", e),
                                }
                            }
                        });
                    }
                }
                Ok(None) => {
                    log::warn!("BRPOP returned None unexpectedly");
                }
                Err(e) => {
                    log::error!("redis connection lost: {}", e);
                    break;
                }
            }
        }

        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(60));
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
