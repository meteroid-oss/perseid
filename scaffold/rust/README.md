{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set download = examples.download -%}
{% set create = examples.create -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(options="") %}{% if call %}{{ docs.call(call, options=options) }}{% else %}client.some_resource(){{ ".with_options(" ~ options ~ ")" if options }}.some_method(){% endif %}{% endmacro -%}
# @@CLIENT_NAME@@ Rust SDK

@@DESCRIPTION@@

```sh
cargo add @@PACKAGE_NAME@@
```

Calls are futures: run them on Tokio. Every method of the API is listed in [api.md](api.md).

## Usage

```rust
use @@RUST_CRATE@@::api::@@CLIENT_NAME@@;
{% if call %}{{ docs.uses(call) }}{% endif %}
let client = @@CLIENT_NAME@@::builder()
    .token("your-api-key")
{%- if not sdk.has_default_base_url %}
    .base_url("https://api.example.com")
{%- endif %}
    .build()?;

{% if call and call.result %}let {{ result }} = {{ call_of() }}.await?;
println!("{{ "{" }}{{ result }}:?}");{% else %}{{ call_of() }}.await?;{% endif %}
```

`@@CLIENT_NAME@@::from_env()?` takes the token from `@@ENV_PREFIX@@_API_KEY` and the base URL from
`@@ENV_PREFIX@@_BASE_URL` when set; `builder()` reads no environment variable, and
`@@CLIENT_NAME@@Builder::from_env()` starts from them. The builder also sets the timeout, retries
and default headers:

```rust
let client = @@CLIENT_NAME@@::builder()
    .token("your-api-key")
    .base_url("https://staging.example.com")
    .timeout(std::time::Duration::from_secs(20))
    .max_retries(3)
    .header("x-team", "billing")
    .build()?;
```

Building a client fails with `Error::Request` when it has no base URL (the API declares none and
`base_url()` is not called) or an invalid one.

Every API area hangs off the client (`{{ docs.resource(call.resource_path) if call else "client.items()" }}`), and `with_options` sets headers, the
timeout, retries or the idempotency key of the calls made through it. Clients are cheap to clone
and can move to other tasks.

Operations take their path parameters, their body, then their query and header parameters as an
options struct, built with the required ones by `new(...)`; when every parameter is optional, pass
the struct or `None`. Models keep the properties this version of the SDK does not know in `extra`,
and send them back. Request models are built from their required fields by `new(...)`{% if create %}:

```rust
{{ docs.uses(create) }}
let {{ docs.var(create.result, "result") }} = {{ docs.call(create) }}.await?;
```
{% else %}.
{% endif %}
Models only found in responses are `#[non_exhaustive]`: build them, in tests say, with
`new(required...)` and field assignments. Operations that may answer without a body return an
`Option`.

## Raw responses

Awaiting a call gives its decoded body; `with_response()` also gives the status and headers:

```rust
let response = {{ call_of() }}.with_response().await?;
println!("{} {:?}", response.status(), response.request_id());
let {{ result }} = response.into_data();
```

## Errors

Every call fails with `@@RUST_CRATE@@::error::Error`: `Api` for a non-2xx response (after
retries), `Timeout`, `Connection`, `Decode` for an unexpected body, `Request` for a request that
could not be built.

```rust
use @@RUST_CRATE@@::error::{ApiErrorKind, Error};

match {{ call_of() }}.await {
    Err(Error::Api(error)) if error.kind() == ApiErrorKind::NotFound => {}
    Err(error) => eprintln!("{error} (request {:?})", error.api().and_then(|e| e.request_id())),
    Ok({{ result }}) => println!("{{ "{" }}{{ result }}:?}"),
}
```

`ApiError::payload()` decodes the body as the API's common error schema, and `json::<T>()` as any
other.

## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with jittered
backoff, honoring `Retry-After`, when the request is idempotent: GET, PUT, DELETE, or any request
with an `Idempotency-Key`, and 429 responses of every request. Each attempt times out after 60
seconds by default.

```rust
use @@RUST_CRATE@@::api::RequestOptions;

let options = RequestOptions::new().max_retries(0).timeout(std::time::Duration::from_secs(5));
{{ call_of("options") }}.await?;
```
{% if list %}
## Pagination

A paginated list returns a `PageCall`. Awaited, it gives the first `Page`, which dereferences to
the response body and lists this page's items with `items()`; `next_page()` fetches the next one.
`items()` on the call streams every item across pages, fetching each page when needed, and
`pages()` every page; both are `futures_core::Stream`s too:

```rust
{% set pg = list.operation.pagination -%}
{% set shown = pg.next_cursor or pg.total_pages or pg.total or pg.has_more or pg["items"] -%}
{% set item = docs.var(list.item, "item") -%}
let page = {{ docs.call(list) }}.await?;
println!("{:?}, {} items", page.{{ shown[0] | to_rust_ident }}, page.items().len());
if let Some(next) = page.next_page().await? {
    println!("{} more", next.items().len());
}

let mut items = {{ docs.call(list) }}.items();
while let Some({{ item }}) = items.next().await {
    println!("{:?}", {{ item }}?);
}

let mut pages = {{ docs.call(list) }}.pages();
while let Some(page) = pages.next().await {
    println!("{} items", page?.items().len());
}
```

`into_inner()` gives a page's body, and `with_response()` the status and headers of the first page.
{% endif %}
{%- if stream %}
## Streaming

Event streams are `futures_core::Stream`s too. Streams of JSON events yield the decoded model and
end at `[DONE]`; `last_event()` gives the raw event, and `into_raw()` the raw events.

```rust
use futures_util::StreamExt;
{{ docs.uses(stream) }}
let mut events = {{ docs.call(stream) }}.await?;
while let Some(event) = events.next().await {
    println!("{:?}", event?);
}
```
{% endif %}
{%- if download %}
## Downloads

A binary response comes as a `BinaryResponse`, its body unread until you consume it: `bytes()`
reads it whole, `chunk()` (or the `Stream`) gives its chunks as they arrive, and as a
`tokio::io::AsyncRead` it streams to a file:

```rust
{{ docs.uses(download) }}let content = {{ docs.call(download) }}.await?.bytes().await?;

let mut response = {{ docs.call(download) }}.await?;
tokio::io::copy(&mut response, &mut tokio::fs::File::create("download.bin").await?).await?;
```

An error status fails the call before any body, retries end once the headers arrive, and the
timeout covers the headers, then each read, not the whole download.
{% endif %}
## Features

`rustls-tls` (default) or `native-tls`, `http2`, `webhooks` for the webhook verifier, and
`tracing` for debug logs of each attempt and retry.

The default client goes through the proxy of `HTTPS_PROXY`, `HTTP_PROXY` or `ALL_PROXY`, except
for the hosts of `NO_PROXY`.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@

_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
