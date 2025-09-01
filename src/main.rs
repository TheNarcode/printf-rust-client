use std::fs::File;
use std::sync::{Arc, Mutex};

use crate::ipp::{PrinterManager, print_job};
use crate::types::{ColorMode, Config, PrintAttributes};
use ::ipp::prelude::Uri;
use eventsource_client::{self as es};
use eventsource_client::{Client, SSE};
use futures::TryStreamExt;

pub mod ipp;
pub mod types;

// handle all errors (for now doing .unwrap())
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = read_config("config.json");
    let client = es::ClientBuilder::for_url(&config.event_url)?.build();

    let pm = Arc::new(Mutex::new(PrinterManager::new(
        config.color_printers.len(),
        config.monochrome_printers.len(),
    )));

    client
        .stream()
        .try_for_each(|event| async {
            let pm = pm.clone();

            if let SSE::Event(e) = event {
                if let "update" = e.event_type.as_str() {
                    let json_string: String = serde_json::from_str(&e.data).unwrap();
                    let attributes: PrintAttributes = serde_json::from_str(&json_string).unwrap();
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

                    print_job(printer_uri, attributes).await;
                }
            }

            Ok(())
        })
        .await?;

    Ok(())
}

pub fn read_config(file_name: &str) -> Config {
    let file = File::open(file_name).unwrap();
    serde_json::from_reader(file).unwrap()
}
