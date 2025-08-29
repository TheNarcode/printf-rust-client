use eventsource_client::{Client, Error, SSE};
use futures::{Stream, TryStreamExt};

pub fn event_listener(client: impl Client) -> impl Stream<Item = Result<(), ()>> {
    client.stream().map_ok(got_event).map_err(got_error)
}

fn got_event(event: SSE) {
    match event {
        SSE::Event(e) => match e.event_type.as_str() {
            _ => {} // handle event_types
        },
        _ => {} // handle other events
    };
}

fn got_error(error: Error) {
    println!("error {}", error); // handle other errors
}
