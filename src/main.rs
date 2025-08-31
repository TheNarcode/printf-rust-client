use std::sync::Arc;

use crate::client::got_event;
use crate::ipp::PrinterManager;
use eventsource_client as es;
use eventsource_client::Client;
use futures::TryStreamExt;
use tokio::sync::Mutex;

pub mod client;
pub mod ipp;
pub mod types;

const URL: &str = "https://archlinux.234892.xyz/event/sse";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = es::ClientBuilder::for_url(URL)?.build();
    let pm = Arc::new(Mutex::new(PrinterManager::new("config.json")));

    client
        .stream()
        .try_for_each(|event| {
            let pm = pm.clone();
            async move {
                let mut pm = pm.lock().await;
                got_event(event, &mut pm).await;
                Ok(())
            }
        })
        .await?;

    Ok(())
}
