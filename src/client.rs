use crate::{
    ipp::{PrinterManager, print},
    types::{self, PrintAttributes},
};
use eventsource_client::{Client, Error, SSE};
use futures::{Stream, TryStreamExt};

pub async fn event_listener(
    client: impl Client,
) -> impl Stream<Item = Result<impl Future<Output = ()>, ()>> {
    client.stream().map_ok(got_event).map_err(got_error)
}

async fn got_event(event: SSE) {
    println!("{:?}", event);
    match event {
        SSE::Event(e) => match e.event_type.as_str() {
            // "update" => {
            //     let mut pm = PrinterManager::new("config.json");
            //     let printer = pm.get_next_printer(types::ColorMode::Color).unwrap();
            //     let attributes = PrintAttributes {
            //         color: types::ColorMode::Color,
            //         copies: 1,
            //         file: "test/aditya.pdf".to_string(),
            //         orientation: 3,
            //         paper_format: "iso_a4_210x297mm".to_string(),
            //     };

            //     print(printer, attributes).await;
            // }
            _ => {} // handle event_types
        },
        _ => {} // handle other events
    };
}

fn got_error(error: Error) {
    println!("error {}", error); // handle other errors
}
