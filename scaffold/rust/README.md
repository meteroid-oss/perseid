{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
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
`@@ENV_PREFIX@@_BASE_URL` when set. The builder also sets the timeout, retries and default headers:

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
neither `base_url()` nor `@@ENV_PREFIX@@_BASE_URL` is set) or an invalid one.

Every API area hangs off the client (`{{ docs.resource(call.resource) if call else "client.items()" }}`), and `with_options` sets headers, the
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
with an `Idempotency-Key` (POST requests get one automatically). Each attempt times out after 60
seconds by default.

```rust
use @@RUST_CRATE@@::api::RequestOptions;

let options = RequestOptions::new().max_retries(0).timeout(std::time::Duration::from_secs(5));
{{ call_of("options") }}.await?;
```
{% if list %}
## Pagination

`*_iter` methods return a `Paginator`, a `futures_core::Stream` of every item that fetches pages
on demand. `pages()` walks pages instead:

```rust
use futures_util::StreamExt;

let mut items = {{ docs.call(list, iter=true) }};
while let Some({{ docs.var(list.item, "item") }}) = items.next().await {
    println!("{:?}", {{ docs.var(list.item, "item") }}?);
}

let page = {{ docs.call(list, iter=true) }}.first_page().await?;
println!("{} items", page.items().len());
if let Some(next) = page.next_page().await? {
    println!("{} more", next.items().len());
}
```
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
## Features

`rustls-tls` (default) or `native-tls`, `http2`, and `webhooks` for the webhook verifier.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
