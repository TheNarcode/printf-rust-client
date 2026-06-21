use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ipp::{PrinterManager, get_ipp_printers, print_job};
use crate::types::{Config, PrintAttributes};
use ftail::Ftail;
use log::LevelFilter;
use redis::AsyncCommands;

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

    let _config = read_config()?;

    let printers = get_ipp_printers().await?;
    let pm = Arc::new(Mutex::new(PrinterManager::new(printers)));

    log::info!("printer manager initialized");

    let redis_url = "rediss://default:gQAAAAAAAgIbAAIgcDFlMTUzZTAyM2M5NDk0MjQ1ODA4NGQ5NjgwOWI2Mzk4YQ@aware-leopard-131611.upstash.io:6379";
    let redis_client = redis::Client::open(redis_url)?;
    loop {
        let mut con = match redis_client.get_multiplexed_async_connection().await {
            Ok(c) => {
                log::info!("connected to redis queue");
                c
            }
            Err(e) => {
                log::error!("failed to connect to redis: {}", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
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
                            let mut pm_guard = pm.lock().unwrap();
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

                        tokio::spawn(async move {
                            log::info!("using printer {} for print", printer.uri);

                            match printer.uri.parse() {
                                Ok(uri) => match print_job(uri, attributes).await {
                                    Ok(_) => log::info!("print job successful"),
                                    Err(e) => log::error!("print job failed: {}", e),
                                },
                                Err(e) => log::error!("failed to parse printer URI: {}", e),
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

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub fn read_config() -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let config_dir = get_config_path()?;
    let file = File::open(&config_dir)?;
    Ok(serde_json::from_reader(file)?)
}

pub fn get_config_path() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let config_dir = dirs::config_local_dir()
        .unwrap()
        .join("printf")
        .join("config.json");

    Ok(config_dir)
}


