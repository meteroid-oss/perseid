use std::time::Duration;

use features::api::{
    BasicAuth, Features, FeaturesOptions, SseEvent, StreamingStreamEventsOptions,
    StreamingUploadFileBody, TokenProvider, Upload, WidgetsListWidgetEventsOptions,
};
use features::models::Health;

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
    assert_eq!(tok.account().health().await.unwrap().status, "||");
    assert_eq!(tok.account().machine_status().await.unwrap().status, "Bearer tok||");
    let widgets = tok.widgets();
    assert_eq!(ids(widgets.list_widgets_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let options = WidgetsListWidgetEventsOptions { kind: "created".into(), starting_after: None };
    let events = widgets.list_widget_events_iter("w1".into(), options);
    assert_eq!(ids(events, |e| e.id).await, ["e1", "e2", "e3"]);
    let mut gadgets = tok.gadgets().list_gadgets_iter(None);
    let mut gadget_ids = Vec::new();
    while let Some(gadget) = gadgets.next().await {
        gadget_ids.push(gadget.unwrap().id);
    }
    assert_eq!(gadget_ids, ["g1", "g2", "g3"]);
    assert_eq!(ids(tok.records().list_records_iter(None), |r| r.id).await, ["r1", "r2", "r3"]);

    let basic_auth = Some(BasicAuth { username: "u".into(), password: "p".into() });
    let basic = client("", FeaturesOptions { basic_auth, ..Default::default() });
    assert_eq!(basic.account().create_session().await.unwrap().status, "Basic dTpw||");
    let token_provider = Some(TokenProvider::new(|| async { Ok("fresh".to_owned()) }));
    let provided = client("", FeaturesOptions { token_provider, ..Default::default() });
    assert_eq!(provided.account().machine_status().await.unwrap().status, "Bearer fresh||");
    let api_keys = [("api_key".to_owned(), "k".to_owned())].into();
    let keyed = client("", FeaturesOptions { api_keys, ..Default::default() });
    assert_eq!(ids(keyed.widgets().list_widgets_iter(None), |w| w.id).await, ["w1", "w2", "w3"]);
    let anonymous = client("", Default::default());
    assert!(anonymous.widgets().list_widgets_iter(None).next().await.unwrap().is_err());
}

#[tokio::test]
async fn streaming() {
    let client = client("tok", Default::default());
    let streaming = client.streaming();
    let options = StreamingStreamEventsOptions { topic: Some("news".into()) };
    let mut stream = streaming.stream_events(Some(options)).await.unwrap();
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
    let body = StreamingUploadFileBody {
        file: Upload::bytes("hello").with_filename("a.txt").with_content_type("text/plain"),
        name: "doc".into(),
        count: Some(2),
        meta: Some(Health { status: "ok".into() }),
    };
    assert_eq!(
        streaming.upload_file(body).await.unwrap().status,
        r#"count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc"#
    );
    let raw = streaming.upload_content("f1".into(), Upload::bytes("raw")).await.unwrap();
    assert_eq!(raw.status, "application/octet-stream:raw");
}
