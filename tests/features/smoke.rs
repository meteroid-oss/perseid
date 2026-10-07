use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, Utc};
use features::api::{
    middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
    ContentRetrieveScenariosEnumOptions, CookiesRetrieveScenariosCookieOptions,
    EncodingListScenariosHeadersOptions, EncodingRetrieveScenariosDatetimeOptions,
    EncodingRetrieveScenariosMultiOptions, EncodingRetrieveScenariosQueryOptions, Features,
    FeaturesBuilder, Page, Paginator, RequestOptions, RetriesRetrieveScenariosFlakyOptions,
    RetriesRetrieveScenariosRateLimitedOptions, RetriesRetrieveScenariosUnavailableOptions,
    RetriesScenariosIdempotentOptions, SseEvent, StreamingRetrieveEventsStreamOptions,
    StreamingUploadFileBody, TokenProvider, Upload, WidgetsListEventsOptions,
    WireBetaSearchOptions, WireSearchOptions,
};
use features::error::{ApiErrorKind, Error};
use features::models::{
    BigBox, Blob, Charge, ChargeItemsItem, ChargeShipping, ChargeShippingAddress,
    CompletionRequest, DateBox, Filter, FilterAmount, Health, Item, ItemPatch, NullBag,
    PaintKind, PaintKindsItem, Payment, SearchRange, Widget, WidgetList,
};
use futures::{StreamExt, TryStreamExt};
use http::{HeaderMap, StatusCode};

fn url() -> String {
    std::env::var("FEATURES_URL").unwrap()
}

/// A builder pointing at the mock server that carries no credentials.
fn builder() -> FeaturesBuilder {
    Features::builder().base_url(url())
}

fn client(token: &str) -> Features {
    builder().token(token).build().unwrap()
}

async fn ids<P, T>(items: Paginator<P, T>, id: fn(T) -> String) -> Vec<String>
where
    P: Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    items.collect().await.unwrap().into_iter().map(id).collect()
}

#[tokio::test]
async fn smoke() {
    let tok = client("tok");
    assert_eq!(tok.account().check_health().await.unwrap().status, "||");
    assert_eq!(tok.account().retrieve_machine().await.unwrap().status, "Bearer tok||");
    let widgets = tok.widgets();
    assert_eq!(ids(widgets.list_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let options = WidgetsListEventsOptions::new("created");
    let events = widgets.list_events_iter("w1", options);
    assert_eq!(ids(events, |e| e.id).await, ["e1", "e2", "e3"]);
    let mut gadgets = tok.gadgets().list_iter(None);
    let mut gadget_ids = Vec::new();
    while let Some(gadget) = gadgets.next().await {
        gadget_ids.push(gadget.unwrap().id);
    }
    assert_eq!(gadget_ids, ["g1", "g2", "g3"]);
    assert_eq!(ids(tok.records().list_iter(None), |r| r.id).await, ["r1", "r2", "r3"]);
    let streamed: Vec<String> =
        widgets.list_iter(None).map_ok(|w| w.id).try_collect().await.unwrap();
    assert_eq!(streamed, ["w1", "w2", "w3"]);
    let first = tok.gadgets().list_iter(None).take(1).collect::<Vec<_>>().await;
    assert_eq!(first.len(), 1);
    let spawned = tokio::spawn(tok.widgets().list_iter(None).collect());
    assert_eq!(spawned.await.unwrap().unwrap().len(), 3);
    let other = RequestOptions::new().header("authorization", "Bearer other");
    let status = tok.account().with_options(other).retrieve_machine().await.unwrap().status;
    assert_eq!(status, "Bearer other||");

    let basic = builder().basic_auth("u", "p").build().unwrap();
    assert_eq!(basic.account().create_session().await.unwrap().status, "Basic dTpw||");
    let provider = TokenProvider::new(|| async { Ok("fresh".to_owned()) });
    let provided = builder().token_provider(provider).build().unwrap();
    assert_eq!(provided.account().retrieve_machine().await.unwrap().status, "Bearer fresh||");
    let keyed = builder().api_key("api_key", "k").build().unwrap();
    assert_eq!(ids(keyed.widgets().list_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let anonymous = client("");
    let error = anonymous.widgets().list_iter(None).next().await.unwrap().unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::Unauthorized));
    assert_eq!(error.api().unwrap().payload().unwrap().error, "unauthorized");
}

#[tokio::test]
async fn parity() {
    let client = client("tok");
    let widget = &client.widgets().list(None).await.unwrap().data[0];
    assert_eq!(widget.extra["color"], "red");
    assert_eq!(serde_json::to_value(widget).unwrap()["color"], "red");

    let response = client.widgets().list(None).with_response().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.request_id(), Some("req_mock"));
    assert_eq!(response.data().data.len(), 2);

    let page = client.widgets().list_iter(None).first_page().await.unwrap();
    let names = |page: &Page<WidgetList, Widget>| {
        page.items().iter().map(|w| w.id.clone()).collect::<Vec<_>>()
    };
    assert_eq!(names(&page), ["w1", "w2"]);
    assert_eq!(page.next_cursor.as_deref(), Some("c2"));
    assert!(page.has_next_page());
    let last = page.next_page().await.unwrap().unwrap();
    assert_eq!(names(&last), ["w3"]);
    assert!(!last.has_next_page());
    assert!(last.next_page().await.unwrap().is_none());
    let pages = client.gadgets().list_iter(None).pages();
    assert_eq!(pages.map_ok(|page| page.into_items().len()).try_collect::<Vec<_>>().await.unwrap(), [2, 1]);

    let streaming = client.streaming();
    let mut chunks = streaming.create_completion_stream(CompletionRequest::new("ab")).await.unwrap();
    let mut deltas = Vec::new();
    while let Some(chunk) = chunks.next().await {
        deltas.push(chunk.unwrap().delta);
    }
    assert_eq!(deltas, ["a", "b"]);
    assert_eq!(chunks.last_event().unwrap().event, "message");
    let completion = streaming.create_completion(CompletionRequest::new("ab")).await.unwrap();
    assert_eq!(completion.text, "AB");

    let mut widget = Widget::new("w9", "nine");
    widget.extra.insert("color".into(), "blue".into());
    assert_eq!(serde_json::to_value(&widget).unwrap()["color"], "blue");
}

/// The one test reading the environment: every other client sets its base URL and token.
#[tokio::test]
async fn environment() {
    let url = url();
    std::env::set_var("FEATURES_BASE_URL", &url);
    std::env::set_var("FEATURES_API_KEY", "from-env");
    let from_env = Features::from_env().unwrap();
    assert_eq!(from_env.account().retrieve_machine().await.unwrap().status, "Bearer from-env||");
    let explicit = Features::builder().token("explicit").base_url(&url).build().unwrap();
    assert_eq!(explicit.account().retrieve_machine().await.unwrap().status, "Bearer explicit||");
    let tokenless = Features::builder().base_url(url).build().unwrap();
    let unauthorized = tokenless.account().retrieve_machine().await.unwrap_err();
    assert_eq!(unauthorized.status(), Some(StatusCode::UNAUTHORIZED), "the builder ignores FEATURES_API_KEY");
    std::env::set_var("FEATURES_BASE_URL", "http://127.0.0.1:9");
    let unreachable = FeaturesBuilder::from_env().token("t").max_retries(0).build().unwrap();
    assert!(unreachable.account().check_health().await.unwrap_err().is_connection());
    let invalid = Features::builder().base_url("features.example.com").build().unwrap_err();
    assert!(invalid.to_string().contains("`features.example.com` is not an absolute"), "{invalid}");
}

#[tokio::test]
async fn streaming() {
    let client = client("tok");
    let streaming = client.streaming();
    let options = StreamingRetrieveEventsStreamOptions::new().topic("news");
    let mut stream = streaming.retrieve_events_stream(Some(options)).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    assert_eq!(
        events,
        [
            SseEvent::new("news").with_event("greeting").with_id("1"),
            SseEvent::new("line1\nline2").with_id("1"),
            SseEvent::new(r#"{"n": 3}"#).with_id("3").with_retry(Duration::from_millis(1500)),
        ]
    );
    let body = StreamingUploadFileBody {
        file: Upload::bytes("hello").with_filename("a.txt").with_content_type("text/plain"),
        name: "doc".into(),
        count: Some(2),
        meta: Some(Health::new("ok")),
        tags: Some(vec!["a".into(), "b".into()]),
    };
    assert_eq!(
        streaming.upload_file(body).await.unwrap().status,
        r#"count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc;tags=::a;tags=::b"#
    );
    let raw = streaming.upload_content("f1", Upload::bytes("raw")).await.unwrap();
    assert_eq!(raw.status, "application/octet-stream:raw");
}

#[tokio::test]
async fn wire() {
    let client = client("tok");
    let wire = client.wire();
    let search = WireSearchOptions::new()
        .filter(Filter { status: Some("open".into()), amount: Some(FilterAmount { gte: Some(5), ..Default::default() }), ..Default::default() })
        .expand(vec!["a".into(), "b".into()])
        .metadata([("k".to_owned(), "v".to_owned())])
        .ids(vec!["x".to_owned(), "y".to_owned()])
        .tags(vec!["t1".into(), "t2".into()])
        .range(SearchRange { gte: Some(1), lt: Some(9), ..Default::default() });
    assert_eq!(
        wire.search(Some(search)).await.unwrap().status,
        "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y\
         &metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2"
    );
    let charge = Charge {
        capture: Some(true),
        metadata: Some([("order".to_owned(), "7".to_owned())].into()),
        items: Some(vec![
            ChargeItemsItem { quantity: Some(2), ..ChargeItemsItem::new("p1") },
            ChargeItemsItem::new("p2"),
        ]),
        expand: Some(vec!["customer".into()]),
        statuses: Some(vec!["a".into(), "b".into()]),
        codes: Some(vec!["c1".into(), "c2".into()]),
        shipping: Some(ChargeShipping {
            address: Some(ChargeShippingAddress {
                line1: Some("1 Main".into()),
                city: Some("Paris".into()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Charge::new(100)
    };
    assert_eq!(
        wire.create_charge(Some(charge)).await.unwrap().status,
        "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer\
         &items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7\
         &shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b"
    );
    assert_eq!(wire.create_charge(None).await.unwrap().status, "|");
    let beta = WireBetaSearchOptions::new().limit(2).features(vec!["x".to_owned(), "y".to_owned()]);
    assert_eq!(
        wire.beta_search(Some(beta.clone())).await.unwrap().status,
        "beta=true&limit=2|features=x,y"
    );
    let overridden = client.wire().with_options(RequestOptions::new().header("features", "z"));
    assert_eq!(
        overridden.beta_search(Some(beta)).await.unwrap().status,
        "beta=true&limit=2|features=z"
    );
    let image = wire.update_image("42", Upload::bytes("png")).await.unwrap();
    assert_eq!(image.status, "42:image/png:png");
}

// ---- scenarios: tests/features/SCENARIOS.md ---------------------------------------------------

static SCENARIO_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh `X-Scenario-Id`, so every test case owns its server-side state.
fn scenario(name: &str) -> String {
    format!("rust-{name}-{}", SCENARIO_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// What the server saw for a scenario id: `GET /__server/attempts/<id>`, plain HTTP, no SDK.
fn server_state(id: &str) -> (u64, Vec<String>) {
    let url = url();
    let host = url.trim_start_matches("http://");
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(
        stream,
        "GET /__server/attempts/{id} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    let state: serde_json::Value = serde_json::from_str(body).unwrap();
    let keys = state["keys"].as_array().unwrap();
    (
        state["attempts"].as_u64().unwrap(),
        keys.iter().map(|key| key.as_str().unwrap().to_owned()).collect(),
    )
}

/// Records every attempt (URL and headers, credentials included) the client makes.
#[derive(Clone, Default)]
struct Tap(Arc<Mutex<Vec<(String, HeaderMap)>>>);

impl Tap {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn headers(&self, attempt: usize) -> HeaderMap {
        self.0.lock().unwrap()[attempt].1.clone()
    }
}

impl Middleware for Tap {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        let target = format!("{} {}", request.method(), request.uri());
        self.0.lock().unwrap().push((target, request.headers().clone()));
        next.run(request)
    }
}

fn tapped(builder: FeaturesBuilder) -> (Features, Tap) {
    let tap = Tap::default();
    (builder.middleware(tap.clone()).build().unwrap(), tap)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Vec<u8> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | BASE64.iter().position(|b| *b == c).expect("a base64 digit") as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-02T03:04:05.250Z").unwrap().with_timezone(&Utc)
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 1, 2).unwrap()
}

#[tokio::test]
async fn items_methods_and_empty_responses() {
    let items = client("tok").items();
    let item = items.retrieve("i1").await.unwrap();
    assert_eq!((item.id.as_str(), item.name.as_str(), item.note.as_deref()), ("i1", "first", Some("hi")));

    // `note` is unset, so the body is exactly `{"name":"renamed"}` and not `"note":null`.
    let patch = ItemPatch { name: Some("renamed".into()), ..ItemPatch::default() };
    let item = items.update("i1", patch).await.unwrap();
    assert_eq!(item.name, "renamed");
    assert_eq!(item.note, None);

    let (): () = items.delete("i1").await.unwrap();
}

#[tokio::test]
async fn content_is_decoded_by_its_type() {
    let content = client("tok").content();
    assert_eq!(content.retrieve_scenarios_nullable_body().await.unwrap(), None::<Item>);
    assert_eq!(content.retrieve_scenarios_text().await.unwrap(), "hello text\n");
    assert_eq!(content.retrieve_scenarios_csv().await.unwrap(), "id,name\n1,alpha\n2,\"be,ta\"\n");

    let blob = content.download_blob().await.unwrap();
    assert_eq!(blob.len(), 256);
    assert_eq!(&blob[..], (0..=255u8).collect::<Vec<_>>().as_slice());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for _ in 0..4 {
        png.extend_from_slice(&[0x00, 0x01, 0xfe, 0xff]);
    }
    assert_eq!(&content.download_image().await.unwrap()[..], png.as_slice());
}

#[tokio::test]
async fn malformed_and_empty_bodies_raise_decode_errors() {
    let content = client("tok").content();
    let error = content.retrieve_scenarios_malformed().await.unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
    assert!(std::error::Error::source(&error).is_some());
    let error = content.retrieve_scenarios_empty_body().await.unwrap_err();
    assert!(matches!(error, Error::Decode(_)), "{error:?}");
    // A 2xx response never becomes an API error, and the decode error is not retried.
    let (client, tap) = tapped(builder().token("tok").max_retries(2));
    assert!(client.content().retrieve_scenarios_malformed().await.unwrap_err().api().is_none());
    assert_eq!(tap.count(), 1);
}

#[tokio::test]
async fn unknown_fields_and_nulls_are_kept() {
    let content = client("tok").content();
    let health = content.list_scenarios_extra_fields().await.unwrap();
    assert_eq!(health.status, "ok");
    assert_eq!(health.extra["extra"], 1);
    assert_eq!(health.extra["nested"], serde_json::json!({"a": [1, 2, {"b": null}]}));
    assert_eq!(health.extra["list"], serde_json::json!([1, "x"]));

    let nulls = content.list_scenarios_nulls().await.unwrap();
    assert_eq!(nulls.name, None);
    assert_eq!(nulls.tags, [Some("a".to_owned()), None, Some("b".to_owned())]);
    assert_eq!(nulls.counts, HashMap::from([("x".to_owned(), Some(1)), ("y".to_owned(), None)]));
    assert_eq!(nulls.note, None);

    // `name` is an explicit null, `note` is unset: the server checks the exact body.
    let sent = NullBag {
        name: None,
        note: None,
        ..NullBag::new(
            HashMap::from([("x".to_owned(), None), ("y".to_owned(), Some(2))]),
            vec![Some("a".to_owned()), None],
        )
    };
    assert_eq!(content.create_scenarios_null(sent.clone()).await.unwrap(), sent);
}

#[tokio::test]
async fn maps_and_enums() {
    let content = client("tok").content();
    let bag = content.retrieve_scenarios_bag().await.unwrap();
    assert_eq!(bag.id, "b1");
    assert_eq!(bag.extra, HashMap::from([("a".to_owned(), 1), ("b".to_owned(), 2)]));
    let labels = content.list_scenarios_labels().await.unwrap();
    assert_eq!(labels, HashMap::from([("k".to_owned(), "v".to_owned()), ("z".to_owned(), "y".to_owned())]));

    let known = content.retrieve_scenarios_enum(ContentRetrieveScenariosEnumOptions::new("known")).await.unwrap();
    assert_eq!(known.kind, PaintKind::Red);
    assert_eq!(known.kinds, Some(vec![PaintKindsItem::Green, PaintKindsItem::Blue]));
    let unknown = content.retrieve_scenarios_enum(ContentRetrieveScenariosEnumOptions::new("unknown")).await.unwrap();
    assert_eq!(unknown.kind, PaintKind::Unknown("magenta".into()));
    assert_eq!(unknown.kind.as_str(), "magenta");
    let kinds = unknown.kinds.unwrap();
    assert_eq!(kinds, [PaintKindsItem::Red, PaintKindsItem::Unknown("magenta".into())]);
}

#[tokio::test]
async fn integers_bytes_and_dates_round_trip() {
    let encoding = client("tok").encoding();
    let echoed = encoding.scenarios_bigint(BigBox::new(i64::MIN, 9_007_199_254_740_993)).await.unwrap();
    assert_eq!((echoed.value, echoed.min), (9_007_199_254_740_993, i64::MIN));

    // Bytes are base64 text in the model.
    let bytes = b"hello\xfb\xff\xfe";
    assert_eq!(base64_encode(bytes), "aGVsbG/7//4=");
    let echoed = encoding.create_scenarios_byte(Blob::new(base64_encode(bytes))).await.unwrap();
    assert_eq!(base64_decode(&echoed.data), bytes);
    let fetched = encoding.list_scenarios_bytes().await.unwrap();
    assert_eq!(base64_decode(&fetched.data), bytes);

    let options = EncodingRetrieveScenariosDatetimeOptions::new(instant(), day());
    let read = encoding.retrieve_scenarios_datetime(options).await.unwrap();
    assert_eq!((read.at, read.day), (instant(), day()));
    let read = encoding.scenarios_datetime(DateBox::new(instant(), day())).await.unwrap();
    assert_eq!((read.at, read.day), (instant(), day()));
}

#[tokio::test]
async fn path_segments_are_escaped() {
    let encoding = client("tok").encoding();
    let values = [
        "plain",
        "sp ace",
        "sl/ash",
        "q?mark",
        "per%cent",
        "ha#sh",
        "lit%25eral",
        "a+b",
        "héllo wörld ✓",
    ];
    for value in values {
        let health = encoding.retrieve_scenario_path(value).await.unwrap();
        assert_eq!(health.status, value);
    }
}

#[tokio::test]
async fn query_values_are_escaped_and_lists_repeat() {
    let encoding = client("tok").encoding();
    for value in ["plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓"] {
        let options = EncodingRetrieveScenariosQueryOptions::new(value);
        assert_eq!(encoding.retrieve_scenarios_query(options).await.unwrap().status, value);
    }
    let ids = vec!["b".to_owned(), "a".to_owned(), "c".to_owned()];
    let options = EncodingRetrieveScenariosMultiOptions::new(ids, true);
    assert_eq!(encoding.retrieve_scenarios_multi(options).await.unwrap().status, "b,a,c");
}

#[tokio::test]
async fn header_parameters_are_sent_only_when_set() {
    let encoding = client("tok").encoding();
    let options = EncodingListScenariosHeadersOptions::new("acme");
    assert_eq!(encoding.list_scenarios_headers(options).await.unwrap().status, "acme|");
    let options = EncodingListScenariosHeadersOptions::new("acme").x_trace_id("t1");
    assert_eq!(encoding.list_scenarios_headers(options).await.unwrap().status, "acme|t1");
}

#[tokio::test]
async fn cookies_are_sent_as_parameters_and_as_credentials() {
    let (client, tap) = tapped(builder().token("tok"));
    let options = CookiesRetrieveScenariosCookieOptions::new("abc123");
    assert_eq!(client.cookies().retrieve_scenarios_cookie(options).await.unwrap().status, "abc123");
    // The operation takes no credentials: none is sent although the client has a token.
    let headers = tap.headers(0);
    assert_eq!(headers["cookie"], "session_id=abc123");
    assert!(!headers.contains_key("authorization"));

    let (keyed, tap) = tapped(builder().api_key("api_key_cookie", "ck1"));
    assert_eq!(keyed.cookies().retrieve_scenarios_cookie_auth().await.unwrap().status, "ck1");
    let headers = tap.headers(0);
    assert_eq!(headers["cookie"], "auth_token=ck1");
    assert!(!headers.contains_key("authorization") && !headers.contains_key("x-api-key"));

    let error = client_without_credentials().cookies().retrieve_scenarios_cookie_auth().await.unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::Unauthorized), "{error:?}");
    assert_eq!(error.api().unwrap().payload().unwrap().error, "unauthorized");
}

const OAUTH_SECRET: &str = "p@ss word";

fn oauth_client(id: &str) -> FeaturesBuilder {
    builder().client_credentials(id, OAUTH_SECRET).max_retries(0)
}

#[tokio::test]
async fn oauth_tokens_are_fetched_once_and_cached() {
    let client = oauth_client("rust-oauth").build().unwrap();
    let status = || async { client.account().retrieve_machine().await.unwrap().status };
    assert_eq!(status().await, "Bearer at-rust-oauth-1||");
    assert_eq!(status().await, "Bearer at-rust-oauth-1||");
    assert_eq!(server_state("rust-oauth").0, 1);

    let in_body = oauth_client("rust-oauth-body").client_credentials_in_body().build().unwrap();
    assert_eq!(
        in_body.account().retrieve_machine().await.unwrap().status,
        "Bearer at-rust-oauth-body-1||"
    );

    let concurrent = oauth_client("rust-oauth-concurrent").build().unwrap();
    let (first, second) =
        tokio::join!(concurrent.account().retrieve_machine(), concurrent.account().retrieve_machine());
    assert_eq!(first.unwrap().status, "Bearer at-rust-oauth-concurrent-1||");
    assert_eq!(second.unwrap().status, "Bearer at-rust-oauth-concurrent-1||");
    assert_eq!(server_state("rust-oauth-concurrent").0, 1);

    // A token wins over the client credentials.
    let with_token = oauth_client("rust-oauth").token("tok").build().unwrap();
    assert_eq!(with_token.account().retrieve_machine().await.unwrap().status, "Bearer tok||");
}

#[tokio::test]
async fn a_rejected_oauth_token_is_replaced_once() {
    let client = oauth_client("rust-oauth-revoked").build().unwrap();
    assert_eq!(
        client.account().retrieve_machine().await.unwrap().status,
        "Bearer at-rust-oauth-revoked-2||"
    );
    assert_eq!(server_state("rust-oauth-revoked").0, 2);

    let invalid = builder().client_credentials("rust-oauth-bad", "wrong").max_retries(0).build().unwrap();
    let error = invalid.account().retrieve_machine().await.unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::Unauthorized), "{error:?}");
    assert_eq!(error.api().unwrap().payload().unwrap().error, "invalid_client");
}

fn client_without_credentials() -> Features {
    builder().build().unwrap()
}

#[tokio::test]
async fn unauthenticated_calls_send_no_credentials_and_raise_the_auth_error() {
    let (client, tap) = tapped(builder());
    let error = client.widgets().list(None).await.unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::Unauthorized));
    assert_eq!(error.status(), Some(StatusCode::UNAUTHORIZED));
    assert_eq!(error.api().unwrap().payload().unwrap().error, "unauthorized");
    assert_eq!(tap.count(), 1, "a 401 is not retried");
    let headers = tap.headers(0);
    assert!(!headers.contains_key("authorization") && !headers.contains_key("x-api-key"));
}

#[tokio::test]
async fn flaky_service_is_retried_after_the_advertised_delay() {
    let id = scenario("flaky");
    let client = builder().max_retries(2).build().unwrap();
    let started = Instant::now();
    let options = RetriesRetrieveScenariosFlakyOptions::new(id.clone());
    let health = client.retries().retrieve_scenarios_flaky(options).await.unwrap();
    assert_eq!(health.status, "attempt=2");
    assert!(started.elapsed() >= Duration::from_millis(950), "{:?}", started.elapsed());
    assert_eq!(server_state(&id).0, 2);
}

#[tokio::test]
async fn flaky_service_is_not_retried_when_retries_are_off() {
    let id = scenario("flaky-off");
    let client = builder().max_retries(0).build().unwrap();
    let options = RetriesRetrieveScenariosFlakyOptions::new(id.clone());
    let error = client.retries().retrieve_scenarios_flaky(options).await.unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
    assert_eq!(error.kind(), Some(ApiErrorKind::InternalServer));
    let api = error.api().unwrap();
    assert_eq!(api.payload().unwrap().error, "unavailable");
    assert_eq!(api.headers["retry-after"], "1");
    assert_eq!(server_state(&id).0, 1);
}

#[tokio::test]
async fn rate_limits_honour_a_retry_after_date() {
    let id = scenario("rate-limited");
    let client = builder().max_retries(2).build().unwrap();
    let started = Instant::now();
    let options = RetriesRetrieveScenariosRateLimitedOptions::new(id.clone());
    let health = client.retries().retrieve_scenarios_rate_limited(options).await.unwrap();
    assert_eq!(health.status, "attempt=2");
    // The date is two seconds ahead, truncated to the second: the wait is not skipped.
    assert!(started.elapsed() >= Duration::from_millis(900), "{:?}", started.elapsed());
    assert_eq!(server_state(&id).0, 2);
}

#[tokio::test]
async fn retries_are_exhausted_after_the_configured_number() {
    let id = scenario("unavailable");
    let (client, tap) = tapped(builder().max_retries(2));
    let options = RetriesRetrieveScenariosUnavailableOptions::new(id.clone());
    let error = client.retries().retrieve_scenarios_unavailable(options).await.unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
    assert_eq!(server_state(&id).0, 3);
    // The middleware runs once per attempt, inside the retry loop.
    assert_eq!(tap.count(), 3);
}

#[tokio::test]
async fn idempotency_keys_are_reused_by_retries() {
    let id = scenario("idempotent");
    let (client, tap) = tapped(builder().max_retries(2));
    let options = RetriesScenariosIdempotentOptions::new(id.clone(), "idem-1");
    let health = client.retries().scenarios_idempotent(Payment::new(5), options).await.unwrap();
    assert_eq!(health.status, "attempts=2;key=idem-1");
    let (attempts, keys) = server_state(&id);
    assert_eq!(attempts, 2);
    assert_eq!(keys, ["idem-1", "idem-1"]);
    assert_eq!(tap.headers(1)["idempotency-key"], "idem-1");
}

#[tokio::test]
async fn dropping_a_call_stops_its_retries() {
    let id = scenario("cancelled");
    let client = builder().max_retries(2).build().unwrap();
    let options = RetriesRetrieveScenariosFlakyOptions::new(id.clone());
    let call = client.retries().retrieve_scenarios_flaky(options);
    // The first attempt fails at once, then the call waits one second for the second.
    let result = tokio::time::timeout(Duration::from_millis(400), call).await;
    assert!(result.is_err(), "the call finished: {result:?}");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(server_state(&id).0, 1, "a cancelled call must not retry");
}

#[tokio::test]
async fn status_errors_are_typed_and_not_retried() {
    let cases = [
        (400u16, ApiErrorKind::BadRequest),
        (401, ApiErrorKind::Unauthorized),
        (403, ApiErrorKind::PermissionDenied),
        (404, ApiErrorKind::NotFound),
        (409, ApiErrorKind::Conflict),
        (422, ApiErrorKind::UnprocessableEntity),
    ];
    for (code, kind) in cases {
        let (client, tap) = tapped(builder().token("tok").max_retries(2));
        let error = client.errors().retrieve_scenario_status(i32::from(code)).await.unwrap_err();
        assert_eq!(error.status().map(|status| status.as_u16()), Some(code));
        assert_eq!(error.kind(), Some(kind));
        let api = error.api().unwrap();
        assert_eq!(api.request_id(), Some("req_mock"));
        let payload = api.payload().unwrap();
        assert_eq!(payload.error, format!("status {code}"));
        assert_eq!(payload.code, Some(i32::from(code)));
        assert_eq!(tap.count(), 1, "status {code} must be answered once");
    }
}

#[tokio::test]
async fn a_failing_second_page_is_raised_not_swallowed() {
    let (client, tap) = tapped(builder().max_retries(2));
    let mut items = client.errors().list_scenarios_pages_iter(None);
    for id in ["p1", "p2"] {
        assert_eq!(items.next().await.unwrap().unwrap().id, id);
    }
    let error = items.next().await.unwrap().unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::CONFLICT));
    let payload = error.api().unwrap().payload().unwrap();
    assert_eq!((payload.error.as_str(), payload.code), ("page_gone", Some(409)));
    assert!(items.next().await.is_none(), "the iteration stops after the error");
    assert_eq!(tap.count(), 2);

    let error = client.errors().list_scenarios_pages_iter(None).collect().await.unwrap_err();
    assert_eq!(error.kind(), Some(ApiErrorKind::Conflict));

    let first = client.errors().list_scenarios_pages_iter(None).first_page().await.unwrap();
    assert!(first.has_next_page());
    let error = first.next_page().await.err().expect("the second page fails");
    assert_eq!(error.status(), Some(StatusCode::CONFLICT));
}

#[tokio::test]
async fn sse_events_are_assembled_across_chunks() {
    let client = client("tok");
    let mut stream = client.streaming().retrieve_scenarios_sse().await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    assert_eq!(
        events,
        [
            SseEvent::new("first"),
            SseEvent::new("line1\nline2").with_event("tick").with_id("7"),
            SseEvent::new(r#"{"n": 3}"#).with_id("7").with_retry(Duration::from_millis(2500)),
            SseEvent::new("tail").with_id("7"),
        ]
    );
    assert_eq!(stream.last_event_id(), Some("7"));
}

#[tokio::test]
async fn an_error_status_is_raised_before_any_event() {
    let client = client("tok");
    let error = client.streaming().retrieve_scenarios_sse_error().await.unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::FORBIDDEN));
    assert_eq!(error.kind(), Some(ApiErrorKind::PermissionDenied));
    let payload = error.api().unwrap().payload().unwrap();
    assert_eq!((payload.error.as_str(), payload.code), ("forbidden", Some(403)));
}
