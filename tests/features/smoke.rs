use std::time::Duration;

use features::api::{
    Features, FeaturesBuilder, Page, Paginator, RequestOptions, SseEvent,
    StreamingUploadFileBody, StreamingRetrieveEventsStreamOptions, TokenProvider, Upload,
    WidgetsListEventsOptions, WireBetaSearchOptions, WireSearchOptions,
};
use features::error::ApiErrorKind;
use features::models::{
    Charge, ChargeItemsItem, ChargeShipping, ChargeShippingAddress, CompletionRequest, Filter,
    FilterAmount, Health, SearchRange, Widget, WidgetList,
};
use futures::{StreamExt, TryStreamExt};

fn builder() -> FeaturesBuilder {
    Features::builder().base_url(std::env::var("FEATURES_URL").unwrap())
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
    assert_eq!(error.api().unwrap().payload().unwrap()["error"], "unauthorized");
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

    let url = std::env::var("FEATURES_URL").unwrap();
    std::env::set_var("FEATURES_BASE_URL", &url);
    std::env::set_var("FEATURES_API_KEY", "from-env");
    let from_env = Features::from_env().unwrap();
    assert_eq!(from_env.account().retrieve_machine().await.unwrap().status, "Bearer from-env||");
    let explicit = Features::builder().token("explicit").base_url(url).build().unwrap();
    assert_eq!(explicit.account().retrieve_machine().await.unwrap().status, "Bearer explicit||");
    std::env::set_var("FEATURES_BASE_URL", "http://127.0.0.1:9");
    let unreachable = Features::builder().token("t").max_retries(0).build().unwrap();
    assert!(unreachable.account().check_health().await.unwrap_err().is_connection());
    let invalid = Features::builder().base_url("features.example.com").build().unwrap_err();
    assert!(invalid.to_string().contains("`features.example.com` is not an absolute"), "{invalid}");

    let mut widget = Widget::new("w9", "nine");
    widget.extra.insert("color".into(), "blue".into());
    assert_eq!(serde_json::to_value(&widget).unwrap()["color"], "blue");
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
    let beta = WireBetaSearchOptions::new().limit(2).features("x,y");
    assert_eq!(
        wire.beta_search(Some(beta.clone())).await.unwrap().status,
        "beta=true&limit=2|features=x,y"
    );
    let overridden = client.wire().with_options(RequestOptions::new().header("features", "z"));
    assert_eq!(
        overridden.beta_search(Some(beta)).await.unwrap().status,
        "beta=true&limit=2|features=z"
    );
    let image = wire.update_image(42, Upload::bytes("png")).await.unwrap();
    assert_eq!(image.status, "42:image/png:png");
}
