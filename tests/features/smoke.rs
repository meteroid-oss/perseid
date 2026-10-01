use std::time::Duration;

use features::api::{
    BasicAuth, Features, FeaturesOptions, RequestOptions, SseEvent, StreamingRetrieveEventsStreamOptions,
    StreamingCreateFileBody, TokenProvider, Upload, WidgetsListEventsOptions,
    WireBetaSearchOptions, WireSearchOptions,
};
use features::models::{
    Charge, ChargeItemsItem, ChargeShipping, ChargeShippingAddress, Filter, FilterAmount,
    Health, SearchRange,
};
use futures::{StreamExt, TryStreamExt};

fn client(token: &str, options: FeaturesOptions) -> Features {
    let server_url = Some(std::env::var("FEATURES_URL").unwrap());
    Features::new(token.to_owned(), Some(FeaturesOptions { server_url, ..options }))
}

async fn ids<T>(items: features::api::Paginator<'_, T>, id: fn(T) -> String) -> Vec<String>
where
    T: serde::de::DeserializeOwned,
{
    items.collect().await.unwrap().into_iter().map(id).collect()
}

#[tokio::test]
async fn smoke() {
    let tok = client("tok", Default::default());
    assert_eq!(tok.account().retrieve_health().await.unwrap().status, "||");
    assert_eq!(tok.account().retrieve_machine().await.unwrap().status, "Bearer tok||");
    let widgets = tok.widgets();
    assert_eq!(ids(widgets.list_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let options = WidgetsListEventsOptions { kind: "created".into(), starting_after: None };
    let events = widgets.list_events_iter("w1".into(), options);
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
    let other = RequestOptions::new().header("authorization", "Bearer other");
    let status = tok.account().with_options(other).retrieve_machine().await.unwrap().status;
    assert_eq!(status, "Bearer other||");

    let basic_auth = Some(BasicAuth { username: "u".into(), password: "p".into() });
    let basic = client("", FeaturesOptions { basic_auth, ..Default::default() });
    assert_eq!(basic.account().session().await.unwrap().status, "Basic dTpw||");
    let token_provider = Some(TokenProvider::new(|| async { Ok("fresh".to_owned()) }));
    let provided = client("", FeaturesOptions { token_provider, ..Default::default() });
    assert_eq!(provided.account().retrieve_machine().await.unwrap().status, "Bearer fresh||");
    let api_keys = [("api_key".to_owned(), "k".to_owned())].into();
    let keyed = client("", FeaturesOptions { api_keys, ..Default::default() });
    assert_eq!(ids(keyed.widgets().list_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let anonymous = client("", Default::default());
    assert!(anonymous.widgets().list_iter(None).next().await.unwrap().is_err());
}

#[tokio::test]
async fn streaming() {
    let client = client("tok", Default::default());
    let streaming = client.streaming();
    let options = StreamingRetrieveEventsStreamOptions { topic: Some("news".into()) };
    let mut stream = streaming.retrieve_events_stream(Some(options)).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    let event = |event: &str, data: &str, id: &str, retry: Option<u64>| SseEvent {
        event: event.into(),
        data: data.into(),
        id: Some(id.into()),
        retry: retry.map(Duration::from_millis),
    };
    assert_eq!(
        events,
        [
            event("greeting", "news", "1", None),
            event("message", "line1\nline2", "1", None),
            event("message", r#"{"n": 3}"#, "3", Some(1500)),
        ]
    );
    let body = StreamingCreateFileBody {
        file: Upload::bytes("hello").with_filename("a.txt").with_content_type("text/plain"),
        name: "doc".into(),
        count: Some(2),
        meta: Some(Health { status: "ok".into() }),
        tags: Some(vec!["a".into(), "b".into()]),
    };
    assert_eq!(
        streaming.create_file(body).await.unwrap().status,
        r#"count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc;tags=::a;tags=::b"#
    );
    let raw = streaming.update_file_content("f1", Upload::bytes("raw")).await.unwrap();
    assert_eq!(raw.status, "application/octet-stream:raw");
}

#[tokio::test]
async fn wire() {
    let client = client("tok", Default::default());
    let wire = client.wire();
    let search = WireSearchOptions {
        filter: Some(Filter {
            status: Some("open".into()),
            amount: Some(FilterAmount { gte: Some(5) }),
        }),
        expand: Some(vec!["a".into(), "b".into()]),
        metadata: Some([("k".to_owned(), "v".to_owned())].into()),
        ids: Some(serde_json::json!(["x", "y"])),
        tags: Some(vec!["t1".into(), "t2".into()]),
        range: Some(SearchRange { gte: Some(1), lt: Some(9) }),
    };
    assert_eq!(
        wire.search(Some(search)).await.unwrap().status,
        "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y\
         &metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2"
    );
    let charge = Charge {
        amount: 100,
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
            }),
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
    let beta = WireBetaSearchOptions { limit: Some(2), features: Some("x,y".into()) };
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
