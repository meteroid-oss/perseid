# Languages

Every SDK retries connection errors, timeouts, 408, 429 and 5xx responses with jittered backoff,
honoring `Retry-After`, but only when the method is idempotent or the request carries an
`Idempotency-Key` (POST gets one automatically). Enum values and union variants newer than the SDK
are kept rather than rejected.

## Rust

Enums and unions are `#[non_exhaustive]` and decode values this version does not know into an
`Unknown` variant that serializes back unchanged. Recursive fields are boxed. Dates are `chrono`
types when the crate depends on `chrono`, as `init` sets it up, and strings otherwise. In PATCH
bodies, nullable optional fields are `Option<Option<T>>`: `Some(None)` sends `null`.

`Options::with_connector` takes any hyper connector, for custom TLS roots, client certificates or
a proxy, and `http_client` any `HttpClient` implementation.

`src/error.rs` is yours: the runtime builds errors with `Error::generic(Failure)` and
`Error::from_response(status, headers, body)` only. The scaffolded one is an enum of `Api`,
`Timeout`, `Transport`, `Decode` and `Request` errors with `source()`.

## TypeScript

The package ships ESM and CommonJS builds behind an `exports` map. Every method takes a last
`{ signal, headers, timeout }` argument, and models convert with `XSerializer.parse(json)` /
`XSerializer.serialize(value)`. With `int64 = "bigint"` or `"string"`, responses are parsed
without losing digits, and `parseJson` / `stringifyJson` do the same for webhook payloads.

## Python

Needs Python 3.10+. Models are keyword-only dataclasses; an optional field that accepts `null`
defaults to `UNSET`, so `None` sends `null`. Methods take `extra_headers=` and `timeout=`, in sync
and async clients. Unions are models holding the discriminator and the variant
(`Shape(type=..., content=Circle(...))`); `flat_unions = true` in `[python.context]` types them as
`Circle | Square` instead. Generated code is `mypy --strict` clean.

## Go

Every method takes trailing options for one call: `WithHeader`, `WithTimeout`,
`WithIdempotencyKey` and `WithMaxRetries`. With `patch_nullable = true`, `Null[T]()` clears a
PATCH field.

## Java

Plain classes with explicit accessors, OkHttp underneath. Unknown enum values decode to
`UNRECOGNIZED`, and `x(null)` sends an explicit `null` for optional nullable fields.

## C#

Targets .NET 8 with nullable reference types and System.Text.Json source generation (trimming and
AOT safe). Every method is async and takes a `RequestOptions` (`Headers`, `Timeout`, `MaxRetries`,
`IdempotencyKey`) then a `CancellationToken`. Pass your own `HttpClient`, e.g. from
`IHttpClientFactory`, or an `HttpMessageHandler` in the options. Unknown values expose `IsKnown`
and `Unrecognized`. With `patch_nullable = true`, which `init` sets, nullable optional PATCH fields
are `MaybeUnset<T>`: assign `null` to send `null`, leave them unset to omit them. Dates are
`DateTimeOffset`s, so fractions of a second beyond 100 nanoseconds are rounded.
