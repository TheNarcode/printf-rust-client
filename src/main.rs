use eventsource_client as es;
use futures::TryStreamExt;

use crate::{
    ipp::{PrinterManager, print},
    types::PrintAttributes,
};

pub mod client;
pub mod ipp;
pub mod types;

const URL: &str = "https://sse.234892.xyz";

// todo: reconnect logic
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = es::ClientBuilder::for_url(URL)?.build();
    let mut stream = client::event_listener(client);
    while let Ok(Some(_)) = stream.try_next().await {}
    Ok(())
}
