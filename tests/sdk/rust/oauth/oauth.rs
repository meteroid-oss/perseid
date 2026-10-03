//! The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml, against
//! a middleware that plays the token endpoint and the API.
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use vault::api::{
    middleware::{BoxError, BoxFuture, Middleware, Next, Request, Response},
    TokenProvider, Vault, VaultBuilder,
};
use vault::error::Error;

const BASE: &str = "https://vault.test/v1";
const TOKEN_URL: &str = "https://vault.test/v1/oauth/token";

struct Seen {
    method: Method,
    uri: String,
    headers: HeaderMap,
}

struct State {
    issued: usize,
    expires_in: u64,
    revoked: Vec<&'static str>,
    scripted: Vec<(StatusCode, &'static str)>,
    seen: Vec<Seen>,
}

/// Issues the access tokens `at-1`, `at-2`... and rejects those in `revoked` with a 401.
#[derive(Clone)]
struct Server(Arc<Mutex<State>>);

impl Server {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            issued: 0,
            expires_in: 3600,
            revoked: Vec::new(),
            scripted: Vec::new(),
            seen: Vec::new(),
        })))
    }

    fn with(self, configure: impl FnOnce(&mut State)) -> Self {
        configure(&mut self.0.lock().unwrap());
        self
    }

    fn token_requests(&self) -> Vec<(Method, HeaderMap)> {
        let state = self.0.lock().unwrap();
        let tokens = state.seen.iter().filter(|seen| seen.uri == TOKEN_URL);
        tokens.map(|seen| (seen.method.clone(), seen.headers.clone())).collect()
    }

    fn total(&self) -> usize {
        self.0.lock().unwrap().seen.len()
    }

    fn client(&self) -> VaultBuilder {
        Vault::builder()
            .base_url(BASE)
            .token("")
            .client_credentials("id", "secret")
            .max_retries(0)
            .middleware(self.clone())
    }
}

impl Middleware for Server {
    fn handle<'a>(
        &'a self,
        request: Request,
        _next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, BoxError>> {
        let mut state = self.0.lock().unwrap();
        let uri = request.uri().to_string();
        state.seen.push(Seen {
            method: request.method().clone(),
            uri: uri.clone(),
            headers: request.headers().clone(),
        });
        let (status, body) = if uri == TOKEN_URL {
            if state.scripted.is_empty() {
                state.issued += 1;
                let body = format!(
                    r#"{{"access_token":"at-{}","token_type":"Bearer","expires_in":{}}}"#,
                    state.issued, state.expires_in
                );
                (StatusCode::OK, body)
            } else {
                let (status, body) = state.scripted.remove(0);
                (status, body.to_owned())
            }
        } else {
            let token = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .unwrap_or("anonymous");
            if state.revoked.contains(&token) {
                (StatusCode::UNAUTHORIZED, r#"{"message":"revoked"}"#.to_owned())
            } else {
                (StatusCode::OK, format!(r#"{{"status":"{token}"}}"#))
            }
        };
        Box::pin(async move {
            Ok(Response::buffered(status, HeaderMap::new(), Bytes::from(body)))
        })
    }
}

fn basic(client_id: &str, secret: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{client_id}:{secret}");
    let mut out = String::from("Basic ");
    for chunk in input.as_bytes().chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

async fn status(client: &Vault) -> String {
    client.account().retrieve_machine().await.unwrap().status
}

#[tokio::test]
async fn schemes_sharing_a_token_url_keep_a_token_per_scope() {
    let server = Server::new();
    let client = server.client().build().unwrap();
    assert_eq!(status(&client).await, "at-1");
    assert_eq!(client.account().retrieve_admin().await.unwrap().status, "at-2");
    assert_eq!(status(&client).await, "at-1");
    assert_eq!(client.account().retrieve_admin().await.unwrap().status, "at-2");
    assert_eq!(server.token_requests().len(), 2);
}

#[tokio::test]
async fn a_token_is_fetched_on_first_use_and_kept() {
    let server = Server::new();
    let client = server.client().build().unwrap();
    assert_eq!(status(&client).await, "at-1");
    assert_eq!(status(&client).await, "at-1");
    let tokens = server.token_requests();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].0, Method::POST);
    assert_eq!(tokens[0].1["content-type"], "application/x-www-form-urlencoded");
    assert_eq!(tokens[0].1["authorization"], basic("id", "secret").as_str());
    let body = "grant_type=client_credentials&scope=secrets.read+secrets.write";
    assert_eq!(tokens[0].1["content-length"], body.len().to_string().as_str());
    let state = server.0.lock().unwrap();
    assert_eq!(state.seen[1].headers["authorization"], "Bearer at-1");
}

#[tokio::test]
async fn public_operations_send_no_credentials_and_fetch_no_token() {
    let server = Server::new();
    let client = server.client().build().unwrap();
    assert_eq!(client.account().check_health().await.unwrap().status, "anonymous");
    assert_eq!(server.total(), 1);
    assert_eq!(client.account().create_session().await.unwrap().status, "at-1");
}

#[tokio::test]
async fn credentials_can_be_sent_in_the_body() {
    let server = Server::new();
    let client = server.client().client_credentials_in_body().build().unwrap();
    assert_eq!(status(&client).await, "at-1");
    let tokens = server.token_requests();
    assert!(!tokens[0].1.contains_key("authorization"));
    let body = "client_id=id&client_secret=secret&grant_type=client_credentials&scope=secrets.read+secrets.write";
    assert_eq!(tokens[0].1["content-length"], body.len().to_string().as_str());
}

#[tokio::test]
async fn an_expired_token_is_renewed() {
    let server = Server::new().with(|state| state.expires_in = 0);
    let client = server.client().build().unwrap();
    assert_eq!(status(&client).await, "at-1");
    assert_eq!(status(&client).await, "at-2");
    assert_eq!(server.token_requests().len(), 2);
}

#[tokio::test]
async fn a_token_the_api_rejects_is_replaced_once() {
    let server = Server::new().with(|state| state.revoked = vec!["at-1"]);
    let client = server.client().build().unwrap();
    assert_eq!(status(&client).await, "at-2");
    assert_eq!(server.token_requests().len(), 2);
    assert_eq!(server.total() - 2, 2);
    assert_eq!(status(&client).await, "at-2");
    assert_eq!(server.token_requests().len(), 2);

    let server = Server::new().with(|state| state.revoked = vec!["at-1", "at-2", "at-3"]);
    let error = server.client().build().unwrap().account().retrieve_machine().await.unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::UNAUTHORIZED), "{error:?}");
    assert_eq!(server.token_requests().len(), 2, "a second 401 is the caller's");
}

#[tokio::test]
async fn the_token_request_is_retried_like_any_request() {
    let server = Server::new().with(|state| state.scripted = vec![(StatusCode::SERVICE_UNAVAILABLE, "{}")]);
    let client = server.client().max_retries(1).build().unwrap();
    assert_eq!(status(&client).await, "at-1");
    assert_eq!(server.token_requests().len(), 2);

    let server = Server::new()
        .with(|state| state.scripted = vec![(StatusCode::UNAUTHORIZED, r#"{"error":"invalid_client"}"#)]);
    let error = server.client().build().unwrap().account().retrieve_machine().await.unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::UNAUTHORIZED), "{error:?}");
    assert_eq!(server.total(), 1, "the API is not called without a token");

    let server = Server::new()
        .with(|state| state.scripted = vec![(StatusCode::OK, r#"{"token_type":"Bearer"}"#)]);
    let error: Error = server.client().build().unwrap().account().retrieve_machine().await.unwrap_err();
    assert!(error.status().is_none(), "{error:?}");
}

#[tokio::test]
async fn concurrent_calls_share_one_token_request() {
    let server = Server::new();
    let client = server.client().build().unwrap();
    let (a, b, c) = tokio::join!(status(&client), status(&client), status(&client));
    assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("at-1", "at-1", "at-1"));
    assert_eq!(server.token_requests().len(), 1);
}

#[tokio::test]
async fn a_token_or_a_provider_wins_over_the_client_credentials() {
    let server = Server::new();
    let provider = TokenProvider::new(|| async { Ok("mine".to_owned()) });
    let provided = server.client().token_provider(provider).build().unwrap();
    assert_eq!(status(&provided).await, "mine");
    let keyed = server.client().token("static").build().unwrap();
    assert_eq!(status(&keyed).await, "static");
    assert!(server.token_requests().is_empty());
}
