//! Runs against the SDK generated from tests/fixtures/torture.yaml (see tests/sdk/run.sh):
//! request building, retries and error handling, with scripted attempts instead of a network.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode};
use torture::{
    api::{
        middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
        Torture,
    },
    error::{ApiErrorKind, Error},
    models::{Kind, ThingCreate},
};

const THING: &str = r#"{"id":"i","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,"nullable_required":null,"tags":[],"metadata":{},"attrs":{}}"#;
const NO_DELAY: (&str, &str) = ("retry-after-ms", "0");

/// What one attempt receives.
#[derive(Clone)]
enum Step {
    Reply(u16, Vec<(&'static str, &'static str)>, &'static str),
    /// The connection fails before any response.
    Fail,
    /// No response ever arrives.
    Hang,
}

fn reply(status: u16, body: &'static str) -> Step {
    Step::Reply(status, Vec::new(), body)
}

/// Plays one step per attempt (the last one repeats) and records every attempt it receives.
#[derive(Clone)]
struct Script {
    steps: Arc<Vec<Step>>,
    seen: Arc<Mutex<Vec<(String, HeaderMap)>>>,
}

impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: Arc::new(steps),
            seen: Arc::default(),
        }
    }

    fn client(&self, max_retries: u32) -> Torture {
        self.builder(max_retries).build().unwrap()
    }

    fn builder(&self, max_retries: u32) -> torture::api::TortureBuilder {
        Torture::builder()
            .token("token")
            .max_retries(max_retries)
            .middleware(self.clone())
    }

    fn attempts(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// `METHOD url` of an attempt.
    fn target(&self, attempt: usize) -> String {
        self.seen.lock().unwrap()[attempt].0.clone()
    }

    fn headers(&self, attempt: usize) -> HeaderMap {
        self.seen.lock().unwrap()[attempt].1.clone()
    }
}

impl Middleware for Script {
    fn handle<'a>(
        &'a self,
        request: Request,
        _: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        let attempt = {
            let mut seen = self.seen.lock().unwrap();
            seen.push((
                format!("{} {}", request.method(), request.uri()),
                request.headers().clone(),
            ));
            seen.len() - 1
        };
        let step = self.steps[attempt.min(self.steps.len() - 1)].clone();
        Box::pin(async move {
            match step {
                Step::Reply(status, headers, body) => {
                    let mut map = HeaderMap::new();
                    for (name, value) in headers {
                        map.insert(name, HeaderValue::from_static(value));
                    }
                    let status = StatusCode::from_u16(status).unwrap();
                    Ok(Response::buffered(status, map, Bytes::from_static(body.as_bytes())))
                }
                Step::Fail => Err(BoxError::from("connection reset by peer")),
                Step::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(BoxError::from("woke up"))
                }
            }
        })
    }
}

/// Counts the attempts it sees: registered first, it wraps every attempt of the retry loop.
#[derive(Clone, Default)]
struct Counter(Arc<Mutex<usize>>);

impl Middleware for Counter {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        *self.0.lock().unwrap() += 1;
        next.run(request)
    }
}

#[tokio::test]
async fn path_parameters_are_percent_encoded() {
    let cases = [
        ("plain", "plain"),
        ("sp ace", "sp%20ace"),
        ("sl/ash", "sl%2Fash"),
        ("q?mark=1", "q%3Fmark=1"),
        ("per%cent", "per%25cent"),
        ("lit%25eral", "lit%2525eral"),
        ("ha#sh", "ha%23sh"),
        ("a+b", "a+b"),
        ("h\u{e9}llo w\u{f6}rld \u{2713}", "h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93"),
        ("\u{1f389}", "%F0%9F%8E%89"),
    ];
    for (value, encoded) in cases {
        let script = Script::new(vec![reply(200, THING)]);
        script.client(0).things().retrieve(value).await.unwrap();
        assert_eq!(script.target(0), format!("GET https://torture.example.com/things/{encoded}"));
    }
}

#[tokio::test]
async fn dot_segments_never_reach_the_server_unescaped() {
    for dots in ["..", "."] {
        let script = Script::new(vec![reply(200, THING)]);
        let result = script.client(0).things().retrieve(dots).await;
        if matches!(result, Err(Error::Request(_))) {
            assert_eq!(script.attempts(), 0, "a rejected path is not sent");
            continue;
        }
        let target = script.target(0);
        let path = target.rsplit_once("/things/").unwrap().1;
        assert_ne!(path, dots, "`{dots}` was sent as a dot segment: {target}");
        assert!(!path.contains("/.."), "{target}");
    }
}

#[tokio::test]
async fn the_base_url_path_prefix_is_kept() {
    for base in ["https://torture.example.com/api/v2", "https://torture.example.com/api/v2/"] {
        let script = Script::new(vec![reply(200, THING)]);
        let client = script.builder(0).base_url(base).build().unwrap();
        client.things().retrieve("t1").await.unwrap();
        assert_eq!(script.target(0), "GET https://torture.example.com/api/v2/things/t1");
    }
}

#[tokio::test]
async fn error_bodies_need_not_be_json() {
    let cases = [
        (500, "text/plain", "upstream exploded", ApiErrorKind::InternalServer),
        (502, "text/html", "<html><h1>Bad Gateway</h1></html>", ApiErrorKind::InternalServer),
        (404, "application/json", "", ApiErrorKind::NotFound),
        (400, "application/json", r#"{"truncated": "#, ApiErrorKind::BadRequest),
        (418, "text/plain", "teapot", ApiErrorKind::Other),
    ];
    for (status, content_type, body, kind) in cases {
        let step = Step::Reply(status, vec![("content-type", content_type), ("x-request-id", "req_7")], body);
        let script = Script::new(vec![step]);
        let error = script.client(0).things().retrieve("t").await.unwrap_err();
        assert_eq!(error.status(), Some(StatusCode::from_u16(status).unwrap()));
        assert_eq!(error.kind(), Some(kind));
        let api = error.api().unwrap();
        assert_eq!(api.text(), body);
        assert_eq!(api.payload(), None, "{body:?} is not JSON");
        assert!(api.json::<serde_json::Value>().is_err());
        assert_eq!(api.request_id(), Some("req_7"));
        let message = error.to_string();
        assert!(message.contains(&status.to_string()) && message.contains(body), "{message}");
        assert_eq!(script.attempts(), 1);
    }

    let script = Script::new(vec![Step::Reply(404, vec![("request-id", "alt_1")], "")]);
    let error = script.client(0).things().retrieve("t").await.unwrap_err();
    assert_eq!(error.api().unwrap().request_id(), Some("alt_1"));
    let script = Script::new(vec![reply(404, "")]);
    let error = script.client(0).things().retrieve("t").await.unwrap_err();
    assert_eq!(error.api().unwrap().request_id(), None);
}

#[tokio::test]
async fn timeouts_are_retried() {
    let script = Script::new(vec![Step::Hang, reply(200, THING)]);
    let client = script
        .builder(1)
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    assert_eq!(client.things().retrieve("t").await.unwrap().name, "n");
    assert_eq!(script.attempts(), 2);

    let script = Script::new(vec![Step::Hang]);
    let client = script
        .builder(1)
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let error = client.things().retrieve("t").await.unwrap_err();
    assert!(error.is_timeout(), "{error:?}");
    assert_eq!(script.attempts(), 2);
}

#[tokio::test]
async fn connection_failures_are_retried() {
    let script = Script::new(vec![Step::Fail, reply(200, THING)]);
    assert_eq!(script.client(1).things().retrieve("t").await.unwrap().id, "i");
    assert_eq!(script.attempts(), 2);

    let script = Script::new(vec![Step::Fail]);
    let error = script.client(1).things().retrieve("t").await.unwrap_err();
    assert!(error.is_connection(), "{error:?}");
    assert!(error.to_string().contains("connection reset by peer"), "{error}");
    assert_eq!(script.attempts(), 2);
}

#[tokio::test]
async fn middleware_runs_once_per_attempt() {
    let script = Script::new(vec![
        Step::Reply(503, vec![NO_DELAY], "busy"),
        Step::Reply(429, vec![NO_DELAY], "slow down"),
        reply(200, THING),
    ]);
    let counter = Counter::default();
    let client = Torture::builder()
        .token("token")
        .middleware(counter.clone())
        .middleware(script.clone())
        .build()
        .unwrap();
    client.things().retrieve("t").await.unwrap();
    assert_eq!(script.attempts(), 3);
    assert_eq!(*counter.0.lock().unwrap(), 3, "the middleware sits inside the retry loop");
    assert_eq!(script.headers(2)["torture-retry-count"], "2");
}

#[tokio::test]
async fn automatic_idempotency_keys_survive_retries() {
    let script = Script::new(vec![Step::Reply(503, vec![NO_DELAY], "busy"), reply(200, THING)]);
    let client = script.client(2);
    let create = ThingCreate::new(Kind::Alpha, "n");
    client.things().create(create.clone(), None).await.unwrap();
    assert_eq!(script.attempts(), 2, "a POST with a key is retried");
    let (first, second) = (script.headers(0), script.headers(1));
    let key = first["idempotency-key"].to_str().unwrap().to_owned();
    assert!(key.starts_with("auto_"), "{key}");
    assert_eq!(second["idempotency-key"], key.as_str());

    // Another request draws another key.
    client.things().create(create, None).await.unwrap();
    let other = script.headers(2)["idempotency-key"].to_str().unwrap().to_owned();
    assert_ne!(other, key);
}

#[tokio::test]
async fn dropping_a_call_cancels_its_retries() {
    let slow = Step::Reply(503, vec![("retry-after-ms", "800")], "busy");
    let script = Script::new(vec![slow, reply(200, THING)]);
    let client = script.client(2);
    let started = Instant::now();
    let cancelled = tokio::time::timeout(Duration::from_millis(150), client.things().retrieve("t")).await;
    assert!(cancelled.is_err(), "{cancelled:?}");
    assert!(started.elapsed() < Duration::from_millis(700));
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(script.attempts(), 1, "nothing is sent once the call is dropped");

    // The client stays usable.
    assert_eq!(client.things().retrieve("t").await.unwrap().name, "n");
    assert_eq!(script.attempts(), 2);

    let script = Script::new(vec![Step::Hang]);
    let client = script.client(2);
    let cancelled = tokio::time::timeout(Duration::from_millis(50), client.things().retrieve("t")).await;
    assert!(cancelled.is_err());
    assert_eq!(script.attempts(), 1);
}

#[tokio::test]
async fn unknown_response_properties_are_tolerated() {
    let body = r#"{"id":"i","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,"nullable_required":null,"tags":[],"metadata":{},"attrs":{},"future":{"a":[1,{"b":null}]},"flag":true}"#;
    let script = Script::new(vec![reply(200, body)]);
    let thing = script.client(0).things().retrieve("t").await.unwrap();
    assert_eq!(thing.extra["future"], serde_json::json!({"a": [1, {"b": null}]}));
    assert_eq!(thing.extra["flag"], true);
}

#[tokio::test]
async fn a_success_body_that_does_not_decode_is_a_decode_error() {
    for body in [r#"{"id": 1}"#, "", "not json", "null", r#"{"id":"i"}"#] {
        let script = Script::new(vec![reply(200, body)]);
        let error = script.client(2).things().retrieve("t").await.unwrap_err();
        assert!(matches!(error, Error::Decode(_)), "{body:?}: {error:?}");
        assert!(error.api().is_none() && error.status().is_none());
        assert_eq!(script.attempts(), 1, "a decode error is not retried");
    }
}

const EVENTS: (&str, &str) = ("content-type", "text/event-stream");

#[tokio::test]
async fn errors_sent_in_a_stream_are_api_errors() {
    let cases = [
        "event: error\ndata: {\"error\":{\"message\":\"overloaded\"}}\n\n",
        "data: {\"error\":{\"message\":\"overloaded\",\"type\":\"server_error\"}}\n\n",
        // An item of the model, with an `error` the model does not declare.
        "data: {\"text\":\"x\",\"error\":{\"message\":\"overloaded\"}}\n\n",
    ];
    for error in cases {
        let body = format!("data: {{\"text\":\"a\"}}\n\n{error}data: {{\"text\":\"b\"}}\n\n");
        let step = Step::Reply(200, vec![EVENTS, ("x-request-id", "req_s")], body.leak());
        let script = Script::new(vec![step]);
        let mut stream = script.client(0).chats().create_stream(None).await.unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().text, "a");
        let error = stream.next().await.unwrap().unwrap_err();
        let api = error.api().unwrap_or_else(|| panic!("{error:?}"));
        assert_eq!(api.status, StatusCode::OK);
        assert_eq!(api.request_id(), Some("req_s"));
        assert_eq!(api.message().as_deref(), Some("overloaded"));
        assert_eq!(error.to_string(), "API error (200 OK): overloaded");
        assert!(stream.next().await.is_none(), "the stream ends at the error");
    }

    let script = Script::new(vec![Step::Reply(200, vec![EVENTS], "event: error\ndata: overloaded\n\n")]);
    let mut stream = script.client(0).chats().create_stream(None).await.unwrap();
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.api().unwrap_or_else(|| panic!("{error:?}")).text(), "overloaded");
}

#[tokio::test]
async fn keepalives_that_are_not_items_are_skipped() {
    let body = ": comment\n\nevent: ping\ndata: {}\n\nevent: keepalive\ndata: {\"type\":\"keepalive\"}\n\n\
        event: ping\ndata: alive\n\ndata: {\"text\":\"a\"}\n\nevent: ping\n\nevent: ping\ndata: {\"text\":\"b\"}\n\ndata: [DONE]\n\n";
    let script = Script::new(vec![Step::Reply(200, vec![EVENTS], body)]);
    let mut stream = script.client(0).chats().create_stream(None).await.unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().text, "a");
    assert_eq!(stream.next().await.unwrap().unwrap().text, "b", "a keepalive that is an item");
    assert!(stream.next().await.is_none());

    let body = "data: {\"text\":\"a\"}\n\nevent: ping\ndata: {}\n\n";
    let script = Script::new(vec![Step::Reply(200, vec![EVENTS], body)]);
    let mut stream = script.client(0).chats().create_stream(None).await.unwrap();
    stream.next().await.unwrap().unwrap();
    assert!(stream.next().await.is_none());
    assert_eq!(stream.last_event().unwrap().data, "{\"text\":\"a\"}", "of the last item");

    let script = Script::new(vec![Step::Reply(200, vec![EVENTS], "data: {\"other\":1}\n\n")]);
    let mut stream = script.client(0).chats().create_stream(None).await.unwrap();
    let error = stream.next().await.unwrap().unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
}

#[tokio::test]
async fn error_messages_show_the_message_of_the_body() {
    let cases = [
        (r#"{"error":{"message":"No such thing","code":"missing"}}"#, "No such thing"),
        (r#"{"message":"Thing not found"}"#, "Thing not found"),
        (r#"{"detail":"Not found."}"#, "Not found."),
        (r#"{"error":{"code":404}}"#, r#"{"error":{"code":404}}"#),
    ];
    for (body, shown) in cases {
        let script = Script::new(vec![reply(404, body)]);
        let error = script.client(0).things().retrieve("t").await.unwrap_err();
        assert_eq!(error.to_string(), format!("API error (404 Not Found): {shown}"));
        assert_eq!(error.api().unwrap().text(), body, "the raw body stays available");
    }
}
