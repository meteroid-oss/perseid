//! The logs of the `tracing` feature.
#![cfg(feature = "tracing")]
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode};
use petstore::api::{
    middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
    PetsListOptions, Petstore,
};
use tracing::{
    field::{Field, Visit},
    span, Event, Level, Metadata, Subscriber,
};

/// Answers every request with a 503 to retry at once.
struct Unavailable;

impl Middleware for Unavailable {
    fn handle<'a>(&'a self, _: Request, _: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>> {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("0"));
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Box::pin(async move { Ok(Response::buffered(status, headers, Bytes::new())) })
    }
}

/// Records each event as its level and `name=value` fields.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(Level, String)>>>);

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        if event.metadata().target().starts_with("petstore") {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.0));
        }
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

#[tokio::test]
async fn attempts_and_retries_are_logged_without_credentials() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());
    let petstore = Petstore::builder()
        .token("secret-token")
        .base_url("https://ada:pw@pets.example.com/v1")
        .max_retries(1)
        .middleware(Unavailable)
        .build()
        .unwrap();
    let error = petstore
        .pets()
        .list(PetsListOptions::new().limit(5))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));

    let events = recorder.0.lock().unwrap();
    let levels: Vec<_> = events.iter().map(|(level, _)| *level).collect();
    assert_eq!(levels, [Level::DEBUG, Level::INFO, Level::DEBUG]);
    let url = "url=https://pets.example.com/v1/pets ";
    for (_, fields) in events.iter() {
        assert!(
            fields.contains("method=GET ") && fields.contains(url),
            "{fields}"
        );
        assert!(!fields.contains("secret") && !fields.contains("pw") && !fields.contains("limit"));
    }
    assert!(events[0].1.contains("status=503 ") && events[0].1.contains("retries=0 "));
    assert!(events[1].1.contains("retry=1 ") && events[1].1.contains("delay_ms=0 "));
    assert!(events[2].1.contains("retries=1 ") && events[2].1.contains("elapsed_ms="));
}
