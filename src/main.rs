use std::fs::{self, File};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ipp::{PrinterManager, print_job};
use crate::types::{ColorMode, Config, PrintAttributes};
use ::ipp::prelude::Uri;
use eventsource_client::{self as es};
use eventsource_client::{Client, SSE};
use ftail::Ftail;
use futures::TryStreamExt;
use log::LevelFilter;

pub mod ipp;
pub mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let logs_dir = dirs::data_local_dir().unwrap().join("printf").join("logs");
    fs::create_dir_all(&logs_dir)?;

    Ftail::new()
        .daily_file(&logs_dir, LevelFilter::Info)
        .init()?;

    log::info!("printf client started");

    let config = read_config()?;

    let client = es::ClientBuilder::for_url(&config.event_url)?
        .reconnect(
            es::ReconnectOptions::reconnect(true)
                .retry_initial(false)
                .delay(Duration::from_secs(1))
                .backoff_factor(2)
                .delay_max(Duration::from_secs(60))
                .build(),
        )
        .build();

    let pm = Arc::new(Mutex::new(PrinterManager::new(
        config.color_printers.len(),
        config.monochrome_printers.len(),
    )));

    log::info!("printer manager initialized");

    client
        .stream()
        .try_for_each(|event| async {
            let pm = pm.clone();

            if let SSE::Event(e) = event {
                if let "update" = e.event_type.as_str() {
                    log::info!("got new print command");

                    let attributes: PrintAttributes = serde_json::from_str(&e.data).unwrap();
                    let mut pm_guard = pm.lock().unwrap();

                    let printer_uri = match &attributes.color {
                        &ColorMode::Color => {
                            let printer = pm_guard.get_next_printer(&attributes.color);
                            config.color_printers[printer].uri.parse::<Uri>().unwrap()
                        }
                        &ColorMode::Monochrome => {
                            let printer = pm_guard.get_next_printer(&attributes.color);
                            config.color_printers[printer].uri.parse::<Uri>().unwrap()
                        }
                    };

                    log::info!("using printer {} for print", printer_uri);

                    print_job(printer_uri, attributes).await.unwrap();

                    log::info!("print job successful");
                }
            }

            Ok(())
        })
        .await?;

    Ok(())
}

pub fn read_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_dir = dirs::config_local_dir()
        .unwrap()
        .join("printf")
        .join("config.json");

    let file = File::open(&config_dir)?;
    Ok(serde_json::from_reader(file)?)
}
