# @@CLIENT_NAME@@ Rust SDK

@@DESCRIPTION@@

```sh
cargo add @@RUST_CRATE@@
```

Calls are futures: run them on Tokio.

## Client

```rust
use @@RUST_CRATE@@::api::@@CLIENT_NAME@@;

// The token from `@@ENV_PREFIX@@_API_KEY`, the base URL from `@@ENV_PREFIX@@_BASE_URL` when set
let client = @@CLIENT_NAME@@::from_env();

let client = @@CLIENT_NAME@@::builder()
    .token("your-api-key")
    .base_url("https://staging.example.com")
    .timeout(std::time::Duration::from_secs(20))
    .max_retries(3)
    .header("x-team", "billing")
    .build();
```

Every API area hangs off the client (`client.items()`), and `with_options` sets headers, the
timeout, retries or the idempotency key of the calls made through it:
`client.items().with_options(RequestOptions::new().max_retries(0))`. Clients are cheap to clone
and can move to other tasks.

Operations take their path parameters, their body, then their query and header parameters as an
options struct: `ItemsListOptions::new(required).limit(10)`. Models keep the properties this
version of the SDK does not know in `extra`, and send them back.

## Raw responses

Awaiting a call gives its decoded body; `with_response()` also gives the status and headers:

```rust
let response = client.items().list(None).with_response().await?;
println!("{} {:?}", response.status(), response.request_id());
let items = response.into_data();
```

## Errors

Every call fails with `@@RUST_CRATE@@::error::Error`: `Api` for a non-2xx response (after
retries), `Timeout`, `Connection`, `Decode` for an unexpected body, `Request` for a request that
could not be built.

```rust
use @@RUST_CRATE@@::error::{ApiErrorKind, Error};

match client.items().retrieve("id").await {
    Err(Error::Api(error)) if error.kind() == ApiErrorKind::NotFound => {}
    Err(error) => eprintln!("{error} (request {:?})", error.api().and_then(|e| e.request_id())),
    Ok(item) => println!("{item:?}"),
}
```

`ApiError::payload()` decodes the body as the API's common error schema, and `json::<T>()` as any
other.

## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with jittered
backoff, honoring `Retry-After`, when the request is idempotent: GET, PUT, DELETE, or any request
with an `Idempotency-Key` (POST requests get one automatically). Each attempt times out after 60
seconds by default.

## Pagination

`*_iter` methods return a `Paginator`, a `futures_core::Stream` of every item that fetches pages
on demand. `pages()` walks pages instead:

```rust
let mut items = client.items().list_iter(None);
while let Some(item) = items.next().await {
    println!("{:?}", item?);
}

let page = client.items().list_iter(None).first_page().await?;
println!("{} items", page.items().len());
if let Some(next) = page.next_page().await? {
    println!("{} more", next.items().len());
}
```

## Streaming

Event streams are `futures_core::Stream`s too. Streams of JSON events yield the decoded model and
end at `[DONE]`; `last_event()` gives the raw event, and `into_raw()` the raw events.

```rust
let mut events = client.items().watch().await?;
while let Some(event) = events.next().await {
    println!("{:?}", event?);
}
```

## Features

`rustls-tls` (default) or `native-tls`, `http2`, and `webhooks` for the webhook verifier.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
