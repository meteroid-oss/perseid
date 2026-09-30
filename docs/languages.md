# Languages

Every SDK retries connection errors, timeouts, 408, 429 and 5xx responses with jittered backoff,
honoring `Retry-After`, but only when the method is idempotent or the request carries an
`Idempotency-Key` (POST gets one automatically). Enum values and union variants newer than the SDK
are kept rather than rejected. Requests time out after `timeout` seconds (60 by default).

API errors carry the status, the response headers (for request ids) and the raw body; error files
scaffolded by older versions keep working, without the headers. They are also typed by status
(not found, rate limited, 5xx...), and the body decodes into the error schema the operation
declares for that status (exact status, then `4XX`, then `default`).

With `typed_unions = true` (set by `init`), values told apart by their JSON type, such as Stripe's
expandable `string | Customer` or `ChargeShipping | ""`, are typed instead of untyped JSON. So
are unions of several object types without a discriminator when their properties tell them apart
(`string | Customer | DeletedCustomer`), or with `untagged_unions = "best-match"`; see
[unions of objects](configuration.md#unions-of-objects). An object no variant matches is kept as
received, and each SDK reads a value as another variant than the one picked.

## Rust

Enums and unions are `#[non_exhaustive]` and decode values this version does not know into an
`Unknown` variant that serializes back unchanged. Recursive fields are boxed. Dates are `chrono`
types when the crate depends on `chrono`, as `init` sets it up, and strings otherwise. In PATCH
bodies, nullable optional fields are `Option<Option<T>>`: `Some(None)` sends `null`.

`Options::with_connector` takes any hyper connector, for custom TLS roots, client certificates or
a proxy, and `http_client` any `HttpClient` implementation. `with_options(RequestOptions)` on a
resource sets headers, the timeout, retries or the idempotency key of its calls:
`client.items().with_options(RequestOptions::new().max_retries(0)).list(None)`. Paginators are
`futures_core::Stream`s when the crate depends on `futures-core`, as `init` sets it up. The `http`
crate is re-exported as `api::http`.

With `typed_unions = true`, unions told apart by JSON type are enums (`ChargeCustomer::String(id)`,
`ChargeCustomer::Customer(Box<Customer>)`, with `From` impls, `as_*` accessors and `id()` for
expandable objects), and union variant structs fill in their discriminator (`Circle::new(1.5)`).
Unions of objects keep unmatched objects in `Unknown(Value)`, and `decode_as::<DeletedCustomer>()`
reads the value as another variant.

`src/error.rs` is yours: the runtime builds errors with `Error::generic(Failure)` and
`Error::from_response(status, headers, body)` only. The scaffolded one is an enum of `Api`,
`Timeout`, `Transport`, `Decode` and `Request` errors with `source()`; `Error::api()` gives the
response, with `payload()` decoding it as `api::ErrorBody` (the spec's common error schema) and
`request_id()`. Methods list their documented error bodies under `# Errors`.

## TypeScript

The package ships ESM and CommonJS builds behind an `exports` map. Every method takes a last
`{ signal, headers, timeout }` argument, and models convert with `XSerializer.parse(json)` /
`XSerializer.serialize(value)`. With `int64 = "bigint"` or `"string"`, responses are parsed without
losing digits, and `parseJson` / `stringifyJson` do the same for webhook payloads. API errors are
`ApiException` subclasses by status (`NotFoundError`, `RateLimitError`, `InternalServerError`...)
with `error`, the body parsed as its declared schema (typed as `<Client>ErrorBody`), and
`requestId`; a timeout throws `ApiTimeoutError`. `requestTimeout: Infinity` turns the timeout off.
With `typed_unions = true`, expandable fields are `string | Customer` (`expandableId(value)` gives
the id either way); a union of objects parses an object with the serializer of the variant it
matches and keeps other objects as received. The webhook verifier has no
Node.js imports, so it runs in browsers, Workers and edge runtimes.

## Python

Needs Python 3.10+. Models are keyword-only dataclasses; an optional field that accepts `null`
defaults to `UNSET`, so `None` sends `null`. Methods take `extra_headers=` and `timeout=`, in sync
and async clients. Unions are models holding the discriminator and the variant
(`Shape(content=Circle(radius=1))`, both tags filled in); `flat_unions = true` in `[python.context]`
types them as `Circle | Square` instead. Values told apart by their JSON type, such as expandable
ids, are `str | Customer` with `typed_unions = true` (`expandable_id(value)` gives the id either
way). A union of objects is annotated `t.Annotated[Customer | DeletedCustomer | UnknownVariant,
ObjectUnion(...)]`: unmatched objects are an `UnknownVariant`, and `as_variant(value, Customer)`
reads a value as another variant. API errors are `ApiException` subclasses by
status (`NotFoundError`, `RateLimitError`...) exported from `api`, with the error body decoded into
its schema as `body` and a `request_id`. Generated code is `mypy --strict` clean.

## Go

Every method takes trailing options for one call: `WithHeader`, `WithTimeout`,
`WithIdempotencyKey` and `WithMaxRetries`. With `patch_nullable = true`, `Null[T]()` clears a
PATCH field. `DefaultTimeout` is `timeout` from perseid.toml. A non-2xx response is an
`*APIError`: `errors.Is(err, ErrNotFound)` (and `ErrUnauthorized`, `ErrRateLimited`, `ErrServer`...)
tests its status, `ErrorBody[T](err)` decodes its body as an error schema of the spec, and
`APIError.Detail()` as the schema most operations share. With `typed_unions = true`, a
primitive-or-object union is a struct with one field per variant (`String *string`,
`Customer *Customer`, `Empty bool` for `""`) plus `New...From...` constructors; values of another
JSON type, or objects no variant matches, are kept in `Raw()`; `ID()` returns the id of an
expandable field and `As(&target)` decodes a union of objects as another variant. Variants of a tagged union fill in their discriminator when it is
left empty. Generated files start with the `// Code generated ... DO NOT EDIT.` line linters
and editors look for.

## Java

Plain classes with explicit accessors, OkHttp underneath. Enums are Java enums whose unknown
values parse as `UNRECOGNIZED`. `x(null)` sends an explicit `null` for optional nullable fields.
Every method has an overload taking a `RequestOptions` (headers, timeout, max retries,
idempotency key) last. Errors are `ApiException`s, with a subclass per common status
(`NotFoundException`, `RateLimitException`...) and `ApiConnectionException`/`ApiTimeoutException`
when no response came; `getError(Type.class)` parses the body as the schema the operation declares.
With `edition = 2`, which `init` sets, exceptions are unchecked, enums are classes with a constant
per value (an unknown value is kept and sent back unchanged, `isKnown()` tells it apart and
`known()` returns an enum to `switch` on), unions of a primitive and an object are typed (`Charge.ChargeCustomer.ofString("cus_1")`, `id()` for expandable objects), and plumbing lives in an `internal`
package. Unions of objects keep unmatched objects as `Unrecognized` and `decodeAs(Customer.class)`
reads a value as another variant.

## C#

Targets .NET 8 with nullable reference types and System.Text.Json source generation (trimming and
AOT safe). Every method is async and takes a `RequestOptions` (`Headers`, `Timeout`, `MaxRetries`,
`IdempotencyKey`) then a `CancellationToken`. Pass your own `HttpClient`, e.g. from
`IHttpClientFactory`, or an `HttpMessageHandler` in the options. Unknown values expose `IsKnown`
and `Unrecognized`. With `patch_nullable = true`, which `init` sets, nullable optional PATCH fields
are `MaybeUnset<T>`: assign `null` to send `null`, leave them unset to omit them. Dates are
`DateTimeOffset`s, so fractions of a second beyond 100 nanoseconds are rounded. API errors are
`ApiException` subclasses by status (`NotFoundException`, `RateLimitException`,
`ServerErrorException`...); `GetError<T>()` parses the body, `GetError()` as the common error
schema, and `GetRequestId()` reads the request id. With `typed_unions = true`, a union is an
abstract record with a nested record per variant, implicit conversions and `AsX` accessors
(and `Id` for expandable objects); other JSON, and objects no variant matches, are kept in
`Unrecognized`, and `DecodeAs(context.Customer)` reads a union of objects as another variant.
