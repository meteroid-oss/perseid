//! Runs against the SDK generated from tests/fixtures/torture.yaml (see tests/sdk/run.sh).
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use bytes::Bytes;
use http1::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{json, Value};
use torture::{
    api::{
        middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
        HttpClient, RequestBody, ThingsCreateThingOptions, ThingsListThingsOptions, Torture,
        TortureOptions,
    },
    error::Error,
    models::*,
};

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(input: Value) -> Value {
    let value: T = serde_json::from_value(input).expect("deserializes");
    serde_json::to_value(value).expect("serializes")
}

fn thing() -> Value {
    json!({
        "id": "t1", "name": "n", "created_at": "2024-01-02T03:04:05.123456789Z", "kind": "beta-2",
        "count": 9007199254740993_i64, "nullable_required": null, "tags": ["a"],
        "metadata": {"k": "v"}, "attrs": {"x": [1, 2]}, "amount": "12.50",
        "birthday": "2024-02-29", "priority": 10, "nested_map": {"a": [{"line1": "l"}]},
    })
}

#[test]
fn models_round_trip_unchanged() {
    assert_eq!(round_trip::<Thing>(thing()), thing());
    let tree = json!({"value": "root", "children": [{"value": "c", "children": []}],
        "next": {"value": "n", "children": []}});
    assert_eq!(round_trip::<TreeNode>(tree.clone()), tree);
    let composed = json!({"id": "b1", "extra": "e", "sibling_prop": "s"});
    assert_eq!(round_trip::<Composed>(composed.clone()), composed);
}

#[test]
fn dates_are_chrono_types() {
    let thing: Thing = serde_json::from_value(thing()).unwrap();
    assert_eq!(thing.created_at.timestamp_subsec_nanos(), 123_456_789);
    assert_eq!(thing.birthday.unwrap().to_string(), "2024-02-29");
}

#[test]
fn required_nullable_fields_are_sent_as_null() {
    let thing = Thing::new(json!({}), 1, Default::default(), "i", Kind::Alpha, Default::default(), "n", vec![]);
    let value = serde_json::to_value(&thing).unwrap();
    assert_eq!(value["nullable_required"], Value::Null);
    assert!(value.get("nullable_optional").is_none());
}

#[test]
fn patch_bodies_tell_null_from_absent() {
    let mut patch = ThingPatch::new();
    patch.description = Some(None);
    patch.count = Some(Some(3));
    assert_eq!(serde_json::to_value(&patch).unwrap(), json!({"description": null, "count": 3}));
    for input in [json!({}), json!({"description": null}), json!({"description": "d"})] {
        assert_eq!(round_trip::<ThingPatch>(input.clone()), input);
    }
    let cleared: ThingPatch = serde_json::from_value(json!({"description": null})).unwrap();
    assert_eq!(cleared.description, Some(None));
}

#[test]
fn unions_keep_their_discriminator() {
    let circle = json!({"type": "circle", "radius": 1.5});
    let shape: Shape = serde_json::from_value(circle.clone()).unwrap();
    assert!(matches!(&shape, Shape::Circle(c) if c.radius == 1.5));
    assert_eq!(shape.tag(), Some("circle"));
    assert_eq!(serde_json::to_value(&shape).unwrap(), circle);

    let cat = json!({"pet_type": "Cat", "meow": true});
    assert_eq!(round_trip::<Pet>(cat.clone()), cat);

    let reopened = serde_json::from_value::<Activity>(json!({"kind": "reopened", "id": "a"}));
    assert!(matches!(reopened, Ok(Activity::Reopened(_))), "{reopened:?}");
}

#[test]
fn unknown_values_are_kept() {
    let triangle = json!({"type": "triangle", "a": 1});
    let shape: Shape = serde_json::from_value(triangle.clone()).unwrap();
    assert!(matches!(&shape, Shape::Unknown(_)));
    assert_eq!(shape.tag(), Some("triangle"));
    assert_eq!(serde_json::to_value(&shape).unwrap(), triangle);

    let mut input = thing();
    input["kind"] = json!("brand-new");
    input["priority"] = json!(99);
    let parsed: Thing = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(parsed.kind, Kind::Unknown("brand-new".into()));
    assert_eq!(parsed.priority, Some(Priority::Unknown(99)));
    assert_eq!(serde_json::to_value(&parsed).unwrap(), input);
    assert_eq!(Kind::from("beta-2"), Kind::Beta2);
    assert_eq!(Kind::Beta2.as_str(), "beta-2");
}

#[test]
fn recursive_types_are_boxed() {
    let mut node = TreeNode::new(vec![], "root");
    node.parent = Some(Box::new(TreeNode::new(vec![], "parent")));
    assert_eq!(node.parent.unwrap().value, "parent");
}

/// Answers every request itself with queued responses, recording what it received.
#[derive(Clone, Default)]
struct Origin {
    responses: Arc<Mutex<Vec<(u16, Vec<(&'static str, &'static str)>, &'static str)>>>,
    requests: Arc<Mutex<Vec<(String, HeaderMap)>>>,
    delay: Option<Duration>,
}

impl Origin {
    fn replying(responses: Vec<(u16, Vec<(&'static str, &'static str)>, &'static str)>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            ..Self::default()
        }
    }

    fn client(&self) -> Torture {
        let mut options = TortureOptions {
            retry_schedule: Some(vec![Duration::ZERO; 3]),
            ..Default::default()
        };
        options.middleware.push(self.clone());
        Torture::new("token", Some(options))
    }

    fn requests(&self) -> Vec<(String, HeaderMap)> {
        self.requests.lock().unwrap().clone()
    }
}

const THING: &str = r#"{"id":"i","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,"nullable_required":null,"tags":[],"metadata":{},"attrs":{}}"#;

impl Middleware for Origin {
    fn handle<'a>(&'a self, request: Request, _: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>> {
        let target = format!("{} {}", request.method(), request.uri());
        self.requests.lock().unwrap().push((target, request.headers().clone()));
        let next = self.responses.lock().unwrap().pop();
        Box::pin(async move {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            let (status, headers, body) = next.unwrap_or((200, vec![], THING));
            let mut map = HeaderMap::new();
            for (name, value) in headers {
                map.insert(name, HeaderValue::from_static(value));
            }
            let status = StatusCode::from_u16(status).unwrap();
            Ok(Response::buffered(status, map, Bytes::from_static(body.as_bytes())))
        })
    }
}

#[tokio::test]
async fn caller_idempotency_key_replaces_the_automatic_one() {
    let origin = Origin::default();
    let options = ThingsCreateThingOptions { idempotency_key: Some("mine".into()) };
    let create = ThingCreate::new(Kind::Alpha, "n");
    origin.client().things().create_thing(create.clone(), Some(options)).await.unwrap();
    origin.client().things().create_thing(create, None).await.unwrap();

    let requests = origin.requests();
    let keys: Vec<_> = requests[0].1.get_all("idempotency-key").iter().collect();
    assert_eq!(keys, ["mine"]);
    assert!(requests[1].1["idempotency-key"].to_str().unwrap().starts_with("auto_"));
}

#[tokio::test]
async fn header_and_date_query_params_are_encoded() {
    let origin = Origin::replying(vec![(200, vec![], r#"{"data":[]}"#)]);
    let since = "2024-01-02T03:04:05Z".parse().unwrap();
    let options = ThingsListThingsOptions {
        x_required: "req".into(),
        since: Some(since),
        day: Some("2024-02-29".parse().unwrap()),
        kinds: Some(vec![Kind::Beta2, Kind::Unknown("new".into())]),
        csv_ids: Some(vec!["a".into(), "b".into()]),
        ..Default::default()
    };
    origin.client().things().list_things(options).await.unwrap();

    let (target, headers) = &origin.requests()[0];
    assert_eq!(
        target,
        "GET https://torture.example.com/things?csv_ids=a%2Cb&kinds=beta-2&kinds=new\
         &since=2024-01-02T03%3A04%3A05Z&day=2024-02-29"
    );
    assert_eq!(headers["x-required"], "req");
    assert_eq!(headers["authorization"], "Bearer token");
}

#[tokio::test]
async fn throttled_requests_are_retried_after_the_advertised_delay() {
    let origin = Origin::replying(vec![
        (200, vec![], THING),
        (503, vec![], "busy"),
        (429, vec![("retry-after", "0")], "slow down"),
    ]);
    let thing = origin.client().things().get_thing("t/1").await.unwrap();
    assert_eq!(thing.id, "i");

    let requests = origin.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].0, "GET https://torture.example.com/things/t%2F1");
    assert_eq!(requests[2].1["torture-retry-count"], "2");
}

#[tokio::test]
async fn non_idempotent_requests_are_not_retried() {
    let origin = Origin::replying(vec![(503, vec![("x-request-id", "r1")], "busy")]);
    let error = origin
        .client()
        .things()
        .update_thing("t", ThingPatch::new())
        .await
        .unwrap_err();
    assert_eq!(origin.requests().len(), 1);
    let Error::Api(api) = &error else { panic!("{error:?}") };
    assert_eq!(api.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(api.headers["x-request-id"], "r1");
    assert_eq!(api.text(), "busy");
    assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
}

#[tokio::test]
async fn errors_are_typed() {
    let body = r#"{"message":"bad","fields":{"name":["required"]}}"#;
    let origin = Origin::replying(vec![(422, vec![], body)]);
    let error = origin
        .client()
        .things()
        .create_thing(ThingCreate::new(Kind::Alpha, "n"), None)
        .await
        .unwrap_err();
    let Error::Api(api) = error else { panic!("{error:?}") };
    let problem: ValidationError = api.json().unwrap();
    assert_eq!(problem.message, "bad");

    let origin = Origin { delay: Some(Duration::from_secs(5)), ..Origin::default() };
    let mut options = TortureOptions {
        timeout: Some(Duration::from_millis(20)),
        num_retries: Some(0),
        ..Default::default()
    };
    options.middleware.push(origin);
    let error = Torture::new("t", Some(options)).tree().get_tree().await.unwrap_err();
    assert!(matches!(error, Error::Timeout), "{error:?}");

    let origin = Origin::replying(vec![(200, vec![], "not json")]);
    let error = origin.client().tree().get_tree().await.unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
    assert!(std::error::Error::source(&error).is_some());
}

/// Counts requests, then hands them to the wrapped client.
struct Counting(Arc<dyn HttpClient>, Arc<AtomicUsize>);

impl HttpClient for Counting {
    fn send(
        &self,
        request: http1::Request<RequestBody>,
    ) -> BoxFuture<'_, Result<http1::Response<hyper::body::Incoming>, BoxError>> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.send(request)
    }
}

/// Serves `THING` over plain HTTP from a thread, once per connection.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{THING}",
                THING.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    url
}

#[tokio::test]
async fn http_client_and_connector_can_be_replaced() {
    let url = serve();
    let options = TortureOptions {
        server_url: Some(url.clone()),
        ..Default::default()
    }
    .with_connector(hyper_util::client::legacy::connect::HttpConnector::new());
    let thing = Torture::new("t", Some(options)).things().get_thing("i").await.unwrap();
    assert_eq!(thing.name, "n");

    let count = Arc::new(AtomicUsize::new(0));
    let inner = torture::api::http_client(hyper_util::client::legacy::connect::HttpConnector::new());
    let options = TortureOptions {
        server_url: Some(url),
        http_client: Some(Arc::new(Counting(inner, count.clone()))),
        ..Default::default()
    };
    Torture::new("t", Some(options)).things().get_thing("i").await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
