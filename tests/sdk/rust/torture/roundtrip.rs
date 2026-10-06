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
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{json, Value};
use torture::{
    api::{
        middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
        HttpClient, RequestBody, RequestOptions, ThingsCreateOptions, ThingsListOptions,
        Torture,
    },
    error::{ApiErrorKind, Error},
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
    let mut newer = thing();
    newer["surprise"] = json!({"nested": [1]});
    assert_eq!(round_trip::<Thing>(newer.clone()), newer);
    let parsed: Thing = serde_json::from_value(newer).unwrap();
    assert_eq!(parsed.extra["surprise"], json!({"nested": [1]}));
    let circle = json!({"type": "circle", "radius": 1.5, "color": "red"});
    let shape: Shape = serde_json::from_value(circle.clone()).unwrap();
    assert!(matches!(&shape, Shape::Circle(c) if c.extra.len() == 1 && c.extra["color"] == "red"));
    assert_eq!(serde_json::to_value(&shape).unwrap(), circle);
    let tree = json!({"value": "root", "children": [{"value": "c", "children": []}],
        "next": {"value": "n", "children": []}});
    assert_eq!(round_trip::<TreeNode>(tree.clone()), tree);
    let composed = json!({"id": "b1", "extra": "e", "sibling_prop": "s"});
    assert_eq!(round_trip::<Composed>(composed.clone()), composed);
}

#[test]
fn all_of_parts_keep_unknown_properties_once() {
    let mut located: Located = serde_json::from_str(r#"{"id":"1","line1":"l","zip":"z"}"#).unwrap();
    located.id = "new".into();
    located.line1 = "edited".into();
    assert_eq!(serde_json::to_string(&located).unwrap(), r#"{"id":"new","line1":"edited","zip":"z"}"#);

    let text = r#"{"id":"b","extra":"e","sibling_prop":"s","more":1}"#;
    let composed: Composed = serde_json::from_str(text).unwrap();
    assert_eq!(composed.extra_properties["more"], 1);
    assert_eq!(serde_json::to_string(&composed).unwrap(), text);
}

#[test]
fn members_named_like_the_unknown_property_map_round_trip() {
    let input = json!({
        "type": "t", "class": "c", "extra": "e", "extra_fields": "f", "properties": {"k": "v"},
        "additional_properties": "a", "any_properties": "p", "$dollar": "d", "with space": "w",
        "surprise": [1],
    });
    let reserved: Reserved = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(reserved.extra.as_deref(), Some("e"));
    assert_eq!(reserved.properties.as_ref().unwrap()["k"], "v");
    assert_eq!(reserved.extra_properties.len(), 1);
    assert_eq!(round_trip::<Reserved>(input.clone()), input);
}

#[test]
fn dates_are_chrono_types() {
    let thing: Thing = serde_json::from_value(thing()).unwrap();
    assert_eq!(thing.created_at.timestamp_subsec_nanos(), 123_456_789);
    assert_eq!(thing.birthday.unwrap().to_string(), "2024-02-29");
}

#[test]
fn required_nullable_fields_are_sent_as_null() {
    let thing = Thing::new(Default::default(), 1, Default::default(), "i", Kind::Alpha, Default::default(), "n", vec![]);
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
        Torture::builder()
            .token("token")
            .middleware(self.clone())
            .build()
            .unwrap()
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
    let options = ThingsCreateOptions::new().idempotency_key("mine");
    let create = ThingCreate::new(Kind::Alpha, "n");
    origin.client().things().create(create.clone(), options).await.unwrap();
    origin.client().things().create(create, None).await.unwrap();

    let requests = origin.requests();
    let keys: Vec<_> = requests[0].1.get_all("idempotency-key").iter().collect();
    assert_eq!(keys, ["mine"]);
    assert!(requests[1].1["idempotency-key"].to_str().unwrap().starts_with("auto_"));
}

#[tokio::test]
async fn header_and_date_query_params_are_encoded() {
    let origin = Origin::replying(vec![(200, vec![], r#"{"data":[]}"#)]);
    let since = "2024-01-02T03:04:05Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    let options = ThingsListOptions::new("req")
        .since(since)
        .day("2024-02-29".parse::<chrono::NaiveDate>().unwrap())
        .kinds(vec![Kind::Beta2, Kind::Unknown("new".into())])
        .csv_ids(vec!["a".into(), "b".into()]);
    origin.client().things().list(options).await.unwrap();

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
        (503, vec![("retry-after-ms", "0")], "busy"),
        (429, vec![("retry-after", "0")], "slow down"),
    ]);
    let thing = origin.client().things().retrieve("t/1").await.unwrap();
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
        .update("t", ThingPatch::new())
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
        .create(ThingCreate::new(Kind::Alpha, "n"), None)
        .await
        .unwrap_err();
    let Error::Api(api) = error else { panic!("{error:?}") };
    let problem: ValidationError = api.json().unwrap();
    assert_eq!(problem.message, "bad");

    let origin = Origin { delay: Some(Duration::from_secs(5)), ..Origin::default() };
    let slow = Torture::builder()
        .token("t")
        .timeout(Duration::from_millis(20))
        .max_retries(0)
        .middleware(origin)
        .build()
        .unwrap();
    let error = slow.tree().retrieve().await.unwrap_err();
    assert!(error.is_timeout() && !error.is_connection(), "{error:?}");

    let origin = Origin::replying(vec![(200, vec![], "not json")]);
    let error = origin.client().tree().retrieve().await.unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
    assert!(std::error::Error::source(&error).is_some());
}

/// Counts requests, then hands them to the wrapped client.
struct Counting(Arc<dyn HttpClient>, Arc<AtomicUsize>);

impl HttpClient for Counting {
    fn send(
        &self,
        request: http::Request<RequestBody>,
    ) -> BoxFuture<'_, Result<http::Response<hyper::body::Incoming>, BoxError>> {
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
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client = Torture::builder().token("t").base_url(&url).connector(connector).build().unwrap();
    let thing = client.things().retrieve("i").await.unwrap();
    assert_eq!(thing.name, "n");

    let count = Arc::new(AtomicUsize::new(0));
    let inner = torture::api::http_client(hyper_util::client::legacy::connect::HttpConnector::new());
    let client = Torture::builder()
        .token("t")
        .base_url(url)
        .http_client(Arc::new(Counting(inner, count.clone())))
        .build()
        .unwrap();
    client.things().retrieve("i").await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn primitive_or_object_unions_are_enums() {
    let holder: UnionHolder = serde_json::from_value(json!({
        "shape": {"type": "circle", "radius": 1.0}, "shapes": [],
        "inline_union": ["a", "b"], "str_or_int": 7,
    }))
    .unwrap();
    assert_eq!(holder.inline_union.as_ref().and_then(|u| u.as_array_of_strings()), Some(&vec!["a".to_owned(), "b".to_owned()]));
    assert_eq!(holder.str_or_int, Some(StringOrInt::Integer(7)));
    for input in [json!("text"), json!(""), json!(3), json!(true), json!(null)] {
        assert_eq!(round_trip::<StringOrInt>(input.clone()), input);
    }
    assert_eq!(serde_json::from_value::<StringOrInt>(json!(true)).unwrap(), StringOrInt::Unknown(json!(true)));
    assert_eq!(StringOrInt::from("x").as_string(), Some("x"));
    assert_eq!(serde_json::to_value(UnionHolderInlineUnion::from("s")).unwrap(), json!("s"));
}

#[test]
fn union_variants_default_their_tag() {
    assert_eq!(Circle::new(2.0).r#type, "circle");
    assert_eq!(Cat::new(true).pet_type, "Cat");
    let shape = Shape::Square(Square::new(3.0));
    assert_eq!(serde_json::to_value(&shape).unwrap(), json!({"type": "square", "side": 3.0}));
}

#[tokio::test]
async fn request_options_apply_to_one_call() {
    let origin = Origin::replying(vec![(503, vec![], "busy"), (503, vec![], "busy")]);
    let options = RequestOptions::new()
        .header("authorization", "Bearer other")
        .header("x-trace", "t")
        .idempotency_key("k1")
        .max_retries(0);
    let error = origin
        .client()
        .things()
        .with_options(options)
        .create(ThingCreate::new(Kind::Alpha, "n"), None)
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
    let requests = origin.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1["authorization"], "Bearer other");
    assert_eq!(requests[0].1["x-trace"], "t");
    assert_eq!(requests[0].1["idempotency-key"], "k1");

    let slow = Origin { delay: Some(Duration::from_secs(5)), ..Origin::default() };
    let options = RequestOptions::new().timeout(Duration::from_millis(20)).max_retries(0);
    let error = slow.client().tree().with_options(options).retrieve().await.unwrap_err();
    assert!(matches!(error, Error::Timeout), "{error:?}");

    let invalid = RequestOptions::new().header("bad header", "v");
    let error = Origin::default().client().tree().with_options(invalid).retrieve().await.unwrap_err();
    assert!(matches!(error, Error::Request(_)), "{error:?}");
}

#[tokio::test]
async fn api_errors_expose_their_payload_and_request_id() {
    let origin = Origin::replying(vec![(404, vec![("x-request-id", "req_1")], r#"{"title":"gone"}"#)]);
    let error = origin.client().tree().retrieve().await.unwrap_err();
    let api = error.api().unwrap();
    assert_eq!(api.kind(), ApiErrorKind::NotFound);
    assert_eq!(api.request_id(), Some("req_1"));
    let payload: torture::api::ErrorBody = api.payload().unwrap();
    assert_eq!(payload["title"], "gone");
}

#[test]
fn unions_of_objects_pick_their_variant_and_keep_unknown_shapes() {
    let account = |value: Value| -> ObjectUnionsAccount { serde_json::from_value(value).unwrap() };
    let active = json!({"id": "a1", "object": "account", "email": "e"});
    let deleted = json!({"deleted": true, "id": "a2", "object": "account"});
    let unknown = json!({"object": "account_v2", "id": "a3"});
    assert_eq!(account(json!("a0")).id(), Some("a0"));
    assert_eq!(account(active.clone()).as_account().map(|a| a.id.as_str()), Some("a1"));
    assert_eq!(account(deleted.clone()).as_deleted_account().map(|a| a.id.as_str()), Some("a2"));
    assert_eq!(account(deleted.clone()).id(), Some("a2"));
    assert_eq!(account(unknown.clone()), ObjectUnionsAccount::Unknown(unknown.clone()));
    for input in [json!("a0"), active, deleted.clone(), unknown] {
        assert_eq!(round_trip::<ObjectUnionsAccount>(input.clone()), input);
    }
    let as_active: Account = account(deleted).decode_as().unwrap();
    assert_eq!(as_active.id, "a2");

    let unions: ObjectUnions = serde_json::from_value(json!({
        "source": {"file_id": "f"},
        "sources": [{"url": "u", "detail": "d"}, {"file_id": "f"}, {"path": "p"}],
        "document": {"title": "t", "author": "a"},
        "loose": {"title": "t"},
    }))
    .unwrap();
    assert!(unions.source.as_ref().and_then(|s| s.as_file_source()).is_some());
    let sources = unions.sources.as_ref().unwrap();
    assert!(sources[0].as_url_source().is_some() && sources[1].as_file_source().is_some());
    assert_eq!(sources[2], ObjectUnionsSources::Unknown(json!({"path": "p"})));
    assert!(unions.document.as_ref().and_then(|d| d.as_article()).is_some());
    // Best match is the default for unions no property tells apart, even without
    // `x-perseid-union`: the tie goes to the first variant.
    assert!(unions.loose.as_ref().and_then(|d| d.as_draft()).is_some_and(|d| d.title == "t"));
    let draft: ObjectUnionsDocument = serde_json::from_value(json!({"title": "t"})).unwrap();
    assert!(draft.as_draft().is_some(), "ties go to the first variant");
    let untitled: ObjectUnionsDocument = serde_json::from_value(json!({"body": "b"})).unwrap();
    assert_eq!(untitled, ObjectUnionsDocument::Unknown(json!({"body": "b"})));
    let text = json!({"source": {"url": "u"}, "document": {"title": "t", "body": "b"}, "sources": []});
    assert_eq!(round_trip::<ObjectUnions>(text.clone()), text);
}

#[tokio::test]
async fn raw_responses_and_client_headers() {
    let origin = Origin::replying(vec![(200, vec![("x-request-id", "req_2")], THING)]);
    let client = Torture::builder()
        .token("token")
        .header("x-client", "c")
        .header("x-trace", "client")
        .middleware(origin.clone())
        .build()
        .unwrap();
    let options = RequestOptions::new().header("x-trace", "call");
    let response = client.things().with_options(options).retrieve("i").with_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.request_id(), Some("req_2"));
    assert_eq!(response.into_data().name, "n");
    let headers = &origin.requests()[0].1;
    assert_eq!(headers["x-client"], "c");
    assert_eq!(headers.get_all("x-trace").iter().collect::<Vec<_>>(), ["call"]);
}

#[tokio::test]
async fn header_params_win_over_client_headers() {
    let origin = Origin::replying(vec![(200, vec![], r#"{"data":[]}"#)]);
    let client = Torture::builder()
        .token("token")
        .header("x-required", "client")
        .header("authorization", "Bearer client")
        .middleware(origin.clone())
        .build()
        .unwrap();
    client.things().list(ThingsListOptions::new("req")).await.unwrap();
    let headers = &origin.requests()[0].1;
    assert_eq!(headers.get_all("x-required").iter().collect::<Vec<_>>(), ["req"]);
    assert_eq!(headers["authorization"], "Bearer client");
}

#[tokio::test]
async fn throttled_non_idempotent_requests_are_not_retried() {
    let origin = Origin::replying(vec![(429, vec![("retry-after-ms", "0")], "slow down")]);
    let error = origin
        .client()
        .things()
        .update("t", ThingPatch::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::RateLimited));
    assert_eq!(origin.requests().len(), 1);
}

#[tokio::test]
async fn bodiless_success_responses_are_none() {
    let origin = Origin::replying(vec![(200, vec![], r#"{"id":"w","name":"n"}"#), (202, vec![], "")]);
    let widgets = origin.client().widgets();
    assert_eq!(widgets.update("w", WidgetUpdate::new()).await.unwrap(), None);
    let updated: Option<Widget> = widgets.update("w", WidgetUpdate::new()).await.unwrap();
    assert_eq!(updated.map(|w| w.name), Some("n".to_owned()));
}

#[test]
fn clients_default_to_the_spec_server() {
    let client = Torture::builder().build().unwrap();
    assert!(format!("{client:?}").contains("https://torture.example.com"), "{client:?}");
}
