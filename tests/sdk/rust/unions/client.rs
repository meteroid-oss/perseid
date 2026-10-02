//! Union bodies and open-enum query parameters of the SDK generated from
//! tests/fixtures/edge-unions.yaml, against a middleware that plays the API.
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use unions_sdk::{
    api::{
        middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
        CompletionsRetrieveOptions, UnionsSdk,
    },
    models::{
        CreateGradeRequest, CreateTranscriptionRequest, CreateTranscriptionResponse, GradeByScore,
        GradeByText, Include, ModelIds,
    },
};

/// Answers every request with `body` and records its method and URL.
#[derive(Clone)]
struct Reply {
    body: &'static str,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Reply {
    fn new(body: &'static str) -> Self {
        Self {
            body,
            seen: Arc::default(),
        }
    }

    fn client(&self) -> UnionsSdk {
        UnionsSdk::builder()
            .token("token")
            .max_retries(0)
            .middleware(self.clone())
            .build()
            .unwrap()
    }
}

impl Middleware for Reply {
    fn handle<'a>(
        &'a self,
        request: Request,
        _next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("{} {}", request.method(), request.uri()));
        let body = self.body;
        Box::pin(async move {
            Ok(Response::buffered(StatusCode::OK, HeaderMap::new(), Bytes::from_static(body.as_bytes())))
        })
    }
}

#[tokio::test]
async fn a_union_response_body_decodes_as_its_best_variant() {
    let verbose = Reply::new(r#"{"text":"hi","duration":1.5,"language":"en"}"#);
    let request = CreateTranscriptionRequest::new("file_1", ModelIds::from("alpha-1"));
    let body = verbose.client().transcriptions().create(request.clone()).await.unwrap();
    assert!(matches!(body, CreateTranscriptionResponse::TranscriptionVerbose(ref v) if v.language == "en"));

    let plain = Reply::new(r#"{"text":"hi"}"#);
    let body = plain.client().transcriptions().create(request).await.unwrap();
    assert!(matches!(body, CreateTranscriptionResponse::Transcription(ref t) if t.text == "hi"));
}

#[test]
fn a_union_request_body_encodes_as_its_variant() {
    let text = CreateGradeRequest::from(GradeByText::new("t"));
    assert_eq!(serde_json::to_value(&text).unwrap(), serde_json::json!({"text": "t"}));
    let score = CreateGradeRequest::from(GradeByScore::new(0.5));
    assert_eq!(serde_json::to_value(&score).unwrap(), serde_json::json!({"score": 0.5}));
}

#[tokio::test]
async fn open_enum_query_parameters_are_sent_as_their_values() {
    let reply = Reply::new(
        r#"{"id":"c1","model":"alpha-1","created_at":"2024-01-02T03:04:05Z","choices":[]}"#,
    );
    let options = CompletionsRetrieveOptions::default()
        .model("brand-new")
        .include(vec![Include::from("logprobs"), Include::from("usage")]);
    reply.client().completions().retrieve("c1", options).await.unwrap();
    let seen = reply.seen.lock().unwrap();
    assert!(seen[0].starts_with("GET "), "{}", seen[0]);
    assert!(
        seen[0].ends_with("/completions/c1?model=brand-new&include=logprobs&include=usage"),
        "{}",
        seen[0]
    );
}
