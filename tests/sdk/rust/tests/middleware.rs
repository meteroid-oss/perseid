use bytes::Bytes;
use http1::{HeaderMap, HeaderValue, Method, StatusCode};
use petstore::api::{
    middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
    Petstore, PetstoreOptions,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

const PET: &str = r#"{"id":"1","name":"Rex","created_at":"2024-01-01T00:00:00Z"}"#;

/// Answers every request itself, so tests need no server.
#[derive(Clone, Default)]
struct Origin {
    calls: Arc<AtomicUsize>,
    seen_headers: Arc<Mutex<Vec<HeaderMap>>>,
}

impl Middleware for Origin {
    fn handle<'a>(
        &'a self,
        request: Request,
        _next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_headers
            .lock()
            .unwrap()
            .push(request.headers().clone());
        Box::pin(async move {
            Ok(Response::buffered(
                StatusCode::OK,
                HeaderMap::new(),
                Bytes::from_static(PET.as_bytes()),
            ))
        })
    }
}

/// Caches successful GET bodies by URL.
#[derive(Clone, Default)]
struct Cache(Arc<Mutex<HashMap<String, Bytes>>>);

impl Middleware for Cache {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        Box::pin(async move {
            if request.method() != Method::GET {
                return next.run(request).await;
            }
            let key = request.uri().to_string();
            let hit = self.0.lock().unwrap().get(&key).cloned();
            if let Some(body) = hit {
                return Ok(Response::buffered(StatusCode::OK, HeaderMap::new(), body));
            }
            let response = next.run(request).await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let (status, headers, body) = response.into_parts().await?;
            self.0.lock().unwrap().insert(key, body.clone());
            Ok(Response::buffered(status, headers, body))
        })
    }
}

/// Adds a header to every request.
struct Tag;

impl Middleware for Tag {
    fn handle<'a>(
        &'a self,
        mut request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        request
            .headers_mut()
            .insert("x-tag", HeaderValue::from_static("yes"));
        next.run(request)
    }
}

fn client(origin: &Origin) -> Petstore {
    let mut options = PetstoreOptions::default();
    options.middleware.push(Tag);
    options.middleware.push(Cache::default());
    options.middleware.push(origin.clone());
    Petstore::new("token", Some(options))
}

#[tokio::test]
async fn cache_serves_repeated_gets_without_reaching_the_origin() {
    let origin = Origin::default();
    let petstore = client(&origin);

    for _ in 0..3 {
        let pet = petstore.pets().get_pet("1").await.unwrap();
        assert_eq!(pet.name, "Rex");
    }
    assert_eq!(origin.calls.load(Ordering::SeqCst), 1);

    petstore.pets().get_pet("2").await.unwrap();
    assert_eq!(origin.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn middleware_runs_in_order_and_can_change_the_request() {
    let origin = Origin::default();
    let petstore = client(&origin);
    petstore.pets().get_pet("1").await.unwrap();

    let seen = origin.seen_headers.lock().unwrap();
    assert_eq!(seen[0]["x-tag"], "yes");
    assert!(seen[0].contains_key("authorization"));
}
