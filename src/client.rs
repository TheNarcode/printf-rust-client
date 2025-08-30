use crate::{
    ipp::{PrinterManager, print},
    types::PrintAttributes,
};
use eventsource_client::SSE;

pub async fn got_event(event: SSE) {
    if let SSE::Event(e) = event {
        if let "update" = e.event_type.as_str() {
            let mut pm = PrinterManager::new("config.json");
            let json_string: String = serde_json::from_str(&e.data).unwrap();
            let attributes: PrintAttributes = serde_json::from_str(&json_string).unwrap();
            let printer = pm.get_next_printer(&attributes.color).unwrap();
            print(printer, attributes).await;
        }
    }
}
