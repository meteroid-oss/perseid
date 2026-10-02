# Languages

Every SDK retries connection errors, timeouts, 408, 429 and 5xx responses with jittered backoff,
honoring `Retry-After`, but only when the method is idempotent or the request carries an
`Idempotency-Key` (POST gets one automatically). Enum values and union variants newer than the SDK
are kept rather than rejected. Requests time out after `timeout` seconds (60 by default).

API errors carry the status, the response headers (for request ids) and the raw body. They are
also typed by status (not found, rate limited, 5xx...), and the body decodes into the error schema
the operation declares for that status (exact status, then `4XX`, then `default`).

Values told apart by their JSON type, such as Stripe's expandable `string | Customer` or
`ChargeShipping | ""`, are typed rather than untyped JSON. So are unions of several object types
without a discriminator when their properties tell them apart
(`string | Customer | DeletedCustomer`), or with `untagged_unions = "best-match"`; see
[unions of objects](configuration.md#unions-of-objects). An object no variant matches is kept as
received, and each SDK reads a value as another variant than the one picked.

## Rust

Clients come from `Acme::builder()` (token, base URL, timeout, `max_retries`, headers,
middleware, `http_client`, or `connector` for any hyper connector: custom TLS roots, client
certificates, a proxy), `Acme::new(token)` or `Acme::from_env()`. Methods return a `Call`, a
future of the decoded body; `.with_response().await` also gives the status, headers and
`request_id()`. `with_options(RequestOptions)` on a resource sets headers, the timeout, retries or
the idempotency key of its calls:
`client.items().with_options(RequestOptions::new().max_retries(0)).list(None)`. Query and header
parameters are `#[non_exhaustive]` options structs, built with `new(required...)` then a setter per
optional parameter. A query parameter that is a union of scalars or lists is an enum. Clients, resources, calls and paginators own what they need, so they move into
`tokio::spawn`. `*_iter` methods return a `Paginator<Page, Item>`, a `futures_core::Stream` of
items whose `pages()` and `first_page()` give `Page`s (`items()`, `has_next_page()`,
`next_page()`, the response through `Deref`). Event streams are `Stream`s of `SseEvent`s, or of
the model each event carries until `[DONE]`, with `last_event()` for the raw event. The `http`
crate is re-exported as `api::http`.

Structs keep the properties they do not declare in `extra` and send them back. Enums and unions
are `#[non_exhaustive]` and decode values this version does not know into an `Unknown` variant
that serializes back unchanged. Recursive fields are boxed. Dates are `chrono` types. In PATCH
bodies, nullable optional fields are `Option<Option<T>>`: `Some(None)` sends `null`.

Unions told apart by JSON type are enums (`ChargeCustomer::String(id)`,
`ChargeCustomer::Customer(Box<Customer>)`, with `From` impls, `as_*` accessors and `id()` for
expandable objects), and union variant structs fill in their discriminator (`Circle::new(1.5)`).
Unions of objects keep unmatched objects in `Unknown(Value)`, and `decode_as::<DeletedCustomer>()`
reads the value as another variant.

`src/error.rs` is yours: the runtime builds errors with `Error::generic(Failure)` and
`Error::from_response(status, headers, body)` only. The scaffolded one is an enum of `Api`,
`Timeout`, `Connection`, `Decode` and `Request` errors with `source()`; `Error::api()` gives the
response, with `kind()` (`NotFound`, `RateLimited`, `InternalServer`...), `payload()` decoding it
as `api::ErrorBody` (the spec's common error schema) and `request_id()`. Methods list their
documented error bodies under `# Errors`.

## TypeScript

The package ships ESM and CommonJS builds behind an `exports` map, and type-checks under
`strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes` and `noImplicitOverride`.
`new Acme({ apiKey })` reads `ACME_API_KEY` and `ACME_BASE_URL` when not given
(`new Acme(token, options)` still works). Every method takes a last
`{ signal, headers, timeout, maxRetries, idempotencyKey }` argument and returns an `APIPromise`:
await it for the body, or call `.withResponse()` for `{ data, response, requestId }` and
`.asResponse()` for the unread `Response`. List methods return a `PagePromise`, iterated with
`for await` over every item or awaited for a `Page` (`items`, `body`, `hasNextPage()`,
`getNextPage()`, `iterPages()`); `listIter()` is a deprecated alias. Event streams whose events
have a schema are a `Stream<Model>` up to `[DONE]`, with the raw event as `stream.lastEvent`, and
`..._stream` twins send `stream: true`; others are an `EventStream` of raw events.

Models convert with `XSerializer.parse(json)` / `XSerializer.serialize(value)`, keeping the
properties the SDK does not know under their JSON names. With `int64 = "bigint"` or `"string"`,
responses are parsed without losing digits, and `parseJson` / `stringifyJson` do the same for
webhook payloads. Everything the SDK throws is an `<Client>Error`: `APIError` subclasses by status
(`NotFoundError`, `RateLimitError`, `InternalServerError`...) with `status`, `headers`,
`requestId`, `body` and `error`, the body parsed as its declared schema (typed as
`<Client>ErrorBody`) or as JSON; `APIConnectionError` and its `APIConnectionTimeoutError`,
`APIUserAbortError` and `APIDecodeError`. `ApiError`, `ApiException` and `ApiTimeoutError` are
deprecated aliases. `requestTimeout: Infinity` turns the timeout off. Unions of scalars and lists
are typed in query, header and path parameters. Expandable fields are `string | Customer`
(`expandableId(value)` gives the id either way); a union of objects parses an object with the
serializer of the variant it matches and keeps other objects as received. The webhook verifier
has no Node.js imports, so it runs in browsers, Workers and edge runtimes.

## Python

Needs Python 3.10+; generated code passes `mypy --strict`, pyright and ruff. Clients take keyword
arguments, Stainless style: `Acme(api_key=..., base_url=..., timeout=..., max_retries=...,
default_headers=..., http_client=...)`, and `client.with_options(max_retries=0)` changes them for
one call. `AsyncAcme` is the asyncio client. Methods take `extra_headers=` and `timeout=`.
Models are keyword-only dataclasses; an optional field that accepts `null` defaults to `UNSET`, so
`None` sends `null`, and properties the SDK does not know are kept in `extra_fields` (read as
attributes at runtime) and sent back. Unions are models holding the discriminator and the variant
(`Shape(content=Circle(radius=1))`, both tags filled in); `flat_unions = true` in `[python]`
types them as `Circle | Square` instead. Values told apart by their JSON type, such as expandable
ids and query parameters taking a value or a list, are `str | Customer` (`expandable_id(value)`
gives the id either way). A union of objects is annotated `t.Annotated[Customer |
DeletedCustomer | UnknownVariant, ObjectUnion(...)]`: unmatched objects are an `UnknownVariant`,
and `as_variant(value, Customer)` reads a value as another variant.

List methods return a `SyncPage` (`items`, `body`, `has_next_page()`, `get_next_page()`,
`iter_pages()`) whose iteration walks every item of every page; the async ones an
`AsyncPaginator` to `await` for the page or `async for` the items. Event streams with a documented
event schema are `Stream[Chunk]`s yielding models until `[DONE]`, with `last_event` the raw event.
`client.with_raw_response.items.retrieve(...)` returns an `APIResponse` (`status_code`,
`headers`, `request_id`, `parse()`).

Every error derives from `AcmeError`. API errors are `APIStatusError` subclasses by status
(`NotFoundError`, `RateLimitError`, `InternalServerError`...) with `body`, the error response
decoded into its schema (else its JSON), and `request_id`. No response raises
`APIConnectionError`, or its subclass `APITimeoutError`; an undecodable 2xx body
`APIResponseValidationError`. The former `ApiException`, `ApiStatusError`, `NetworkException`,
`ResponseDecodeError`, `AcmeOptions` and `AcmeAsync` names remain as deprecated aliases.

## Go

Needs Go 1.23+. Required query and header parameters are arguments, optional ones go in a
`*...Options` struct (pointers, `nil` to omit); every method also takes trailing options for one
call: `WithHeader`, `WithTimeout`, `WithIdempotencyKey`, `WithMaxRetries` and
`WithResponseInto(&resp)`, which hands over the `*http.Response` (status, headers). Names spell
initialisms the Go way (`CustomerID`, `APIKey`), and the package of a multi-word name is one word
(`realworld`). `allOf` parts are inlined into flat structs, and every struct keeps the properties
it does not know in `ExtraFields`, sent back when encoding. Nullable optional PATCH fields are
`*Nullable[T]`: `NewNullable(v)` sets one and `ExplicitNull[T]()` clears it. `DefaultTimeout` is
`timeout` from perseid.toml, and `Options.Logger` (a `*slog.Logger`) logs every attempt.

Every error is an `SDKError`: `*APIError` for a non-2xx response, `*TimeoutError`,
`*TransportError` when no response came, `*DecodeError` and `*RequestError`. `errors.Is(err,
ErrNotFound)` (and `ErrUnauthorized`, `ErrRateLimited`, `ErrServer`...) tests the status,
`APIError.Body` holds the body decoded as the error schema the operation declares (else plain
JSON), `ErrorBody[T](err)` decodes it as any schema and `APIError.Detail()` as the one most
operations share.

List operations have `ListPage`, a `*Page[T]` with `Items`, `HasNextPage()` and `NextPage(ctx)`,
and `ListIter`, a `*Pager[T]` over every item (`Next`/`Current`/`Err`, or `range pager.All()`).
Event streams with a schema are a `*Stream[T]` of decoded events ending at `[DONE]` (`Event()`
gives the raw `SSEEvent`), others an `*EventStream`; lines are capped at 1 MiB.

A primitive-or-object union is a struct with one field per variant (`String *string`,
`Customer *Customer`, `Empty bool` for `""`) plus `New...From...` constructors, also for query
parameters; values of another JSON type, or objects no variant matches, are kept in `Raw()`;
`ID()` returns the id of an expandable field and `As(&target)` decodes a union of objects as
another variant. Variants of a tagged union fill in their discriminator when it is left empty.
Generated files start with the `// Code generated ... DO NOT EDIT.` line linters and editors look
for; `doc.go`, `errors.go` and `version.go` are yours.

## Java

Final classes with fluent accessors, OkHttp and Jackson underneath, for Java 11 and later. Models
are immutable: `Widget.builder()...build()` throws on a missing required property and
`toBuilder()` changes a copy. Required properties are read directly, the others as `Optional`s;
for optional nullable ones, `x(null)` on the builder sends `null` while leaving `x` unset leaves it
out. Properties the SDK does not know are kept in `additionalProperties()` and sent back.

`new Client(options)` or `Client.fromEnv()` builds an `AutoCloseable` client from immutable
`ClientOptions.builder()` settings. Required path, query and header parameters are method
arguments; optional ones go in an immutable `...Options.builder()`, and unions of scalars such as
`string | string[]` are typed (`Ids.ofList(...)`). Every method has an overload taking a
`RequestOptions` (headers, timeout, max retries, idempotency key) last. `client.async()` has the
same methods returning `CompletableFuture`s over OkHttp's async calls, and `withRawResponse()`
returns `ApiResponse`s with the status and headers. `...Iter` methods return a `Paginator`
(`Iterable`, `stream()`, `firstPage()`, `pages()`) of `Page`s (`items()`, `hasNextPage()`,
`nextPage()`), or an `AsyncPaginator` (`forEach`, `toList`). Event streams are typed
`EventStream<Chunk>`s ending at `[DONE]`, with `lastEvent()` for the raw event. Requests are
logged through `System.Logger`.

Every exception is unchecked and derives from `ClientException`: `ApiException` for error
responses (`statusCode()`, `headers()`, `body()`, `requestId()`), with a subclass per common status
(`NotFoundException`, `RateLimitException`...), `ApiConnectionException` and its
`ApiTimeoutException` when no response came, and `InvalidDataException` for a response that is not
what the API describes, such as a required property left out (thrown by its getter).
`error(Type.class)` parses the body as the schema the operation declares, and `error()` falls back
to a `JsonNode`. `exceptions/ApiException.java` is yours: SDKs scaffolded by an earlier perseid must update it
to the constructor taking the status, headers, body and parsed error. Enums are classes with a
constant per value: an unknown value is kept and sent back unchanged, `isKnown()` tells it apart
and `known()` returns an enum to `switch` on. Unions of a primitive and an object are typed
(`Charge.ChargeCustomer.ofString("cus_1")`, `id()` for expandable objects); unions of objects keep
unmatched objects as `Unrecognized`, and `decodeAs(Customer.class)` reads a value as another
variant. The HTTP plumbing lives in an `internal` package.

## C#

Targets .NET 8 with nullable reference types and System.Text.Json source generation (trimming and
AOT safe); generated code builds clean with `AnalysisLevel` `latest-recommended` and documents every
public member. `new AcmeClient()` reads `ACME_API_KEY` and `ACME_BASE_URL`. Every method is async
and takes a `RequestOptions` (`Headers`, `Timeout`, `MaxRetries`, `IdempotencyKey`) then a
`CancellationToken`. Each resource implements an interface (`IAcmeClient.Customers` is an
`ICustomersApi`) to mock in tests, and `client.Customers.WithRawResponse.RetrieveAsync(id)` returns
an `ApiResponse<Customer>` with `StatusCode`, `Headers`, `RequestId` and the `Value`. Pass your own
`HttpClient` or an `HttpMessageHandler` in the options, or set `dependency_injection = true` under
`[csharp.context]` for `services.AddAcmeClient(o => o.Token = ...)`, an `IHttpClientFactory` typed
client (the scaffolded project then references `Microsoft.Extensions.Http`). Each call is an
`Activity` of the `ActivitySource` named after the package.

Models are `sealed record`s with `init` properties and read-only collections, compared by value
(collections and JSON included); properties the SDK does not know are kept in
`AdditionalProperties` and sent back. Unknown enum values expose `IsKnown`. Nullable optional PATCH
fields are `MaybeUnset<T>`: assign `null` to send `null`, leave them unset to omit them. Dates are
`DateTimeOffset`s, so fractions of a second beyond 100 nanoseconds are rounded. A union is an
abstract record with a nested record per variant (`StringValue`, `ListValue`... for JSON types),
implicit conversions and `AsX` accessors (and `Id` for expandable objects); other JSON, and objects
no variant matches, are kept in `Unrecognized`, and `DecodeAs(context.Customer)` reads a union of
objects as another variant. Query parameters that are unions of scalars and lists are typed the
same way.

`ListAutoPagingAsync()` returns an `AsyncPager` to `await foreach` over every item, whose
`AsPagesAsync()` and `GetFirstPageAsync()` give `Page`s with `Items`, `Response`, `HasNextPage` and
`GetNextPageAsync()` (`ListIterAsync()` is its deprecated name). Event streams whose events the
spec types are `EventStream<T>`s of models, ending at `[DONE]`, with `LastEvent` for the raw event.

Everything the SDK throws derives from `AcmeException`: `ApiException` subclasses by status
(`NotFoundException`, `RateLimitException`, `ServerErrorException`...),
`ApiConnectionException` and its `ApiTimeoutException`, and `ApiDecodeException`. An
`ApiException` has the `Body` (truncated in the message), `Error`, the body parsed as the schema
the operation declares for its status (else a `JsonElement`), `GetError<T>()` and `RequestId`
(`GetDeclaredError()` and `GetRequestId()` in projects scaffolded by earlier versions).
