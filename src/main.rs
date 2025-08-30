use crate::client::got_event;
use eventsource_client as es;
use eventsource_client::Client;
use futures::TryStreamExt;

pub mod client;
pub mod ipp;
pub mod types;

const URL: &str = "https://archlinux.234892.xyz/event/sse";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = es::ClientBuilder::for_url(URL)?.build();

    client
        .stream()
        .try_for_each(|event| async {
            got_event(event).await;
            Ok(())
        })
        .await?;

    Ok(())
}
