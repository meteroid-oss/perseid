# Languages

Examples use an API named `Acme` with a `customers` resource. Names follow the `name` of
`perseid.toml`: the client is `Acme`, the environment variables start with `ACME_`.

## What every SDK does

| Behavior | |
|---|---|
| Retries | Connection errors, timeouts, 408, 429 and 5xx, retried twice by default with jittered exponential backoff |
| Safe retries only | Idempotent methods, or requests with an `Idempotency-Key`. Every POST gets one automatically |
| `Retry-After` | `retry-after-ms` and `Retry-After` set the wait when at most 60 seconds, else the backoff applies |
| Timeout | Per attempt, `timeout` of `perseid.toml` (60 seconds by default), settable per client and per call |
| Environment | `ACME_API_KEY` for the token, `ACME_BASE_URL` for the base URL, `ACME_CLIENT_ID` and `ACME_CLIENT_SECRET` for OAuth2 client credentials |
| No base URL | When neither the spec, `base_url` nor the caller gives one, creating the client (or the first call in Go) fails, naming both settings |
| Errors | One base error type, API errors typed by status, the body decoded into the error schema of that status, then `4XX`, then `default`, then the API-wide error schema |
| Unknown values | Enum values and union variants the SDK does not know are kept and sent back unchanged |
| Unknown properties | Kept on the model and sent back when it is serialized |
| Per-call options | Headers, timeout, max retries and idempotency key |
| `api.md` | Every method by resource, in the language's syntax, with its HTTP method, path and models. Regenerated with the code |
| `README.md` | Written on the first generation, with examples calling the API's own operations. Yours afterwards |
| Tests | A [test per operation](#tests), regenerated with the code |

Generated files carry an `@generated` marker. perseid never overwrites or deletes the others.
See [features](features.md) for auth, pagination and encoding, and
[configuration](configuration.md#names) for how spec names become identifiers.

## At a glance

| | Requires | Client | Base error | Paginated list | Raw response |
|---|---|---|---|---|---|
| Rust | Tokio | `Acme::builder()...build()?` | `Error` | `list_iter(...)` | `.with_response()` |
| TypeScript | Node.js 20+, Deno, Bun, browsers | `new Acme({ apiKey })` | `AcmeError` | `for await` on `list()` | `.withResponse()` |
| Python | Python 3.10+ | `Acme(api_key=...)`, `AsyncAcme` | `AcmeError` | iterate `list()` | `client.with_raw_response` |
| Go | Go 1.23+ | `acme.New(token, opts)` | `SDKError` | `ListAutoPaging(...)` | `WithResponseInto(&resp)` |
| Java | Java 11+ | `new Acme(AcmeOptions...)` | `AcmeException` | `listIter()` | `client.withRawResponse()` |
| C# | .NET 8 | `new AcmeClient(token)` | `AcmeException` | `ListAutoPagingAsync()` | `.WithRawResponse` |

## Formats

A string `format` types the value wherever it appears: fields, list items, parameters, bodies,
and named schemas of it.

| `format` | Rust | TypeScript | Python | Go | Java | C# |
|---|---|---|---|---|---|---|
| `date-time` | `chrono::DateTime<Utc>` | `Date` | `datetime` | `time.Time` | `OffsetDateTime` | `DateTimeOffset` |
| `date` | `chrono::NaiveDate` | `string` | `date` | `Date` | `LocalDate` | `DateOnly` |
| `decimal` | `rust_decimal::Decimal` | `string` | `Decimal` | `string` | `BigDecimal` | `decimal` |
| `uuid` | `uuid::Uuid` | `string` | `UUID` | `uuid.UUID` | `UUID` | `Guid` |
| `uri` | `String` | `string` | `str` | `string` | `URI` | `string` |

- TypeScript keeps dates as `"2024-01-31"` strings: a `Date` is an instant, which time zones
  would shift to another day.
- `uuid` is `github.com/google/uuid` in Go. `[types] uuid = "string"` in
  [perseid.toml](configuration.md#sdk-defaults) keeps UUIDs strings everywhere.
- Go's `Date` (`Year`, `Month`, `Day`) is in the SDK package: `DateOf(t)` and `ParseDate(s)`
  build one, `In(loc)` turns it back into a `time.Time`.

## Tests

Each operation gets a test calling it with sample arguments. A mock transport answers it in
process with a sample of the declared response, so no server or extra tool is needed. Samples
come from the spec's schemas. The test checks the method and path sent, and that the response
decodes.

| | Tests | Run |
|---|---|---|
| Rust | `tests/api/` | `cargo test` |
| TypeScript | `tests/api/*.test.ts` | `npm test` |
| Python | `tests/test_*.py` | `python -m unittest discover -s tests`, or `pytest` |
| Go | `*_test.go` | `go test ./...` |
| Java | `src/test/java/.../api/` | `gradle test`, with JUnit 5 |
| C# | `Acme.Tests/` | `dotnet test`, with xUnit |

- Some operations get no test: those with required query or header parameters, multipart,
  binary or list bodies, or non-scalar path parameters.
- The manifests perseid writes on the first generation set up the runner.
- `tests = false`, at the top level or in a language table, leaves them out and deletes the
  generated ones.

### Round trips

`round_trips = true` adds a test decoding and encoding sample JSON of every model, from
`round_trips.json`: every property set, required ones only, `null`s, each union variant and enum
value. What comes back must be the same JSON, up to the spelling of date-times and decimals.

| | Test | Samples |
|---|---|---|
| Rust | `tests/round_trips.rs` | `tests/round_trips.json` |
| TypeScript | `tests/api/roundTrips.test.ts` | `tests/api/round_trips.json` |
| Python | `tests/test_round_trips.py` | `tests/round_trips.json` |
| Go | `round_trips_test.go` | `testdata/round_trips.json` |
| Java | `src/test/java/.../api/RoundTripsTest.java` | `src/test/resources/.../api/round_trips.json` |
| C# | `Acme.Tests/RoundTripsTests.cs` | `Acme.Tests/round_trips.json` |

- It's off by default: the samples grow with the spec, to 19 MB for Stripe's and 25 MB for GitHub's.
- It needs `tests`, and leaving it out deletes the generated round trips.

## Rust

### Install

```sh
cargo add acme
```

Cargo features: `rustls-tls` (default) or `native-tls`, `http1` (default), `http2`, and
`webhooks` for the [webhook verifier](customizing.md#webhooks).

### Client

```rust
let client = Acme::builder()
    .token("sk_live_...")
    .base_url("https://staging.acme.com")
    .timeout(Duration::from_secs(20))
    .max_retries(3)
    .build()?;
```

- `Acme::new(token)` and `Acme::from_env()` are shortcuts. All three return a `Result`.
- The builder also takes `header`, `middleware`, `http_client`, and `connector` for any hyper
  connector (custom TLS roots, client certificates, a proxy).
- Credentials: `token_provider`, `client_credentials(id, secret)`, `basic_auth`,
  `api_key(scheme, key)`.
- Building fails with `Error::Request` without a base URL, or with one that is not absolute
  http(s).
- Clients are cheap to clone. Clients, resources, calls and paginators own what they need, so
  they move into `tokio::spawn`.

### Calls and options

```rust
let customer = client.customers().retrieve("cus_1").await?;
let charge = client.charges().create(Charge { capture: Some(true), ..Charge::new(100) }).await?;

let options = RequestOptions::new().max_retries(0).timeout(Duration::from_secs(5));
client.customers().with_options(options).list(None).await?;
```

- Methods take path parameters, the body, then query and header parameters as an options struct.
- Options structs are `#[non_exhaustive]`: `new(required...)`, then a setter per optional
  parameter. When none is required, pass the struct or `None`.
- `with_options` on a resource sets headers, timeout, retries or idempotency key for its calls.
- An operation that also declares a bodiless 2xx returns `Option<T>`.

### Pagination

```rust
let mut customers = client.customers().list_iter(None);
while let Some(customer) = customers.next().await {
    let customer = customer?;
}
let page = client.customers().list_iter(None).first_page().await?;
```

`*_iter` methods return a `Paginator`, a `futures_core::Stream` of items. `pages()` and
`first_page()` give `Page`s with `items()`, `has_next_page()` and `next_page()`; the response
body is reachable through `Deref`.

### Streaming

Event streams are `Stream`s of the model each event carries, ending at `[DONE]`, or of raw
`SseEvent`s. `last_event()` gives the raw event, `into_raw()` the raw stream.

### Errors

```rust
match client.customers().retrieve("cus_1").await {
    Err(Error::Api(error)) if error.kind() == ApiErrorKind::NotFound => {}
    Err(error) => eprintln!("{error} (request {:?})", error.api().and_then(|e| e.request_id())),
    Ok(customer) => {}
}
```

- `Error` has `Api`, `Timeout`, `Connection`, `Decode` and `Request` variants.
- `ApiError` has `kind()` (`NotFound`, `RateLimited`, `InternalServer`...), `request_id()`,
  `payload()` (the API's common error schema, `api::ErrorBody`) and `json::<T>()`.
- Methods list their documented error bodies under `# Errors`.

### Raw responses

`client.customers().retrieve(id).with_response().await?` returns the `status()`, `headers()`,
`request_id()` and `into_data()`.

### Models and unions

- Structs keep undeclared properties in `extra` (`extra_properties` when the schema has an
  `extra` property). `allOf` parts are inlined into one struct.
- Structs only found in responses are `#[non_exhaustive]`: build them with `new(required...)` or
  `Default`, then assign fields. Request structs also take struct literals.
- `Default` is implemented when every required field has a default.
- In PATCH bodies, nullable optional fields are `Option<Option<T>>`: `Some(None)` sends `null`.
- Recursive fields are boxed. Dates are `chrono` types.
- Enums and unions are `#[non_exhaustive]`, with an `Unknown` variant that serializes back
  unchanged.
- Unions are enums with `From` impls and `as_*` accessors: `ChargeCustomer::String(id)`,
  `ChargeCustomer::Customer(Box<Customer>)`. Expandable fields have `id()`.
- Union variant structs fill in their discriminator: `Circle::new(1.5)`.
- `decode_as::<DeletedCustomer>()` reads a union of objects as another variant.

### Notes

- `src/error.rs` is yours. The runtime only calls `Error::generic(Failure)` and
  `Error::from_response(status, headers, body)`.
- The `http` crate is re-exported as `acme::api::http`.

## TypeScript

### Install

```sh
npm install acme
```

ESM and CommonJS builds behind an `exports` map. The package only needs `fetch`, and
type-checks under `strict`, `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`.

### Client

```ts
const client = new Acme({ apiKey: "sk_live_...", timeout: 20_000, maxRetries: 5 });
```

| Option | |
|---|---|
| `apiKey`, `baseURL` | Default to `ACME_API_KEY` and `ACME_BASE_URL` |
| `timeout` | Milliseconds, `Infinity` to wait forever |
| `maxRetries`, `retryScheduleInMs` | Retry count, or explicit delays |
| `defaultHeaders`, `defaultQuery` | Sent with every request. A `null` header removes one the SDK sets |
| `fetch`, `middleware`, `debug` | Custom `fetch`, [middleware](customizing.md#middleware), a summary of each request on stderr |
| `tokenProvider`, `clientId`, `clientSecret`, `oauthClientAuth`, `basicAuth`, `apiKeys` | [Credentials](features.md#authentication) |

### Calls and options

```ts
const customer = await client.customers.retrieve("cus_1", { timeout: 5_000, maxRetries: 0 });
```

- The last argument of every method is `{ signal, headers, query, timeout, maxRetries, idempotencyKey }`.
- Unions of scalars and lists are typed in query, header and path parameters.
- `int64 = "bigint"` or `"string"` under `[typescript]` parses int64 values without losing digits.
  With `"string"`, the int64 values of a union variant are `number | bigint`, as a string would
  read as a string variant.

### Pagination

```ts
for await (const customer of client.customers.list({ perPage: 100 })) { ... }
const page = await client.customers.list();
```

List methods return a `PagePromise`. A `Page` has `items`, `body`, `hasNextPage()`,
`getNextPage()` and `iterPages()`.

### Streaming

```ts
const stream = await client.completions.createStream({ prompt: "hi" });
for await (const chunk of stream) process.stdout.write(chunk.delta);
```

- Events with a schema give a `Stream<Model>` ending at `[DONE]`, with `stream.lastEvent`.
- Other streams are an `EventStream` of raw events.
- Breaking out of the loop or calling `stream.close()` closes the connection.

### Errors

```ts
try {
  await client.customers.retrieve("cus_1");
} catch (error) {
  if (error instanceof NotFoundError) console.log(error.status, error.requestId, error.error);
}
```

| Error | When |
|---|---|
| `AcmeError` | Base of everything the SDK throws |
| `APIError` | Non-2xx: `status`, `headers`, `requestId`, `body` (raw text), `error` (parsed, typed `AcmeErrorBody`) |
| `BadRequestError`, `AuthenticationError`, `PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`, `RateLimitError`, `InternalServerError` | 400, 401, 403, 404, 409, 422, 429, 5xx |
| `APIConnectionError`, `APIConnectionTimeoutError` | No response, or none within the timeout |
| `APIUserAbortError` | The `signal` aborted |
| `APIDecodeError` | A 2xx body that is not valid JSON, not the expected event stream, or not what its schema describes |

### Raw responses

Methods return an `APIPromise`. `.withResponse()` gives `{ data, response, requestId }`, and
`.asResponse()` the unread `Response`.

### Models and unions

- Models are plain objects with camelCase properties.
- `CustomerSerializer.parse(json)` and `.serialize(value)` convert them, keeping unknown
  properties under their JSON names. A typed `additionalProperties` gives the model an index
  signature.
- Parsing checks values against their schema: a wrong type, or a required property missing
  or `null`, throws an `APIDecodeError` naming its path (`$.items[3].total: expected a number,
  got string "abc"`). Unknown properties, unknown enum values and values no union variant holds
  are kept. `validate_responses = false` under `[typescript]` turns the checks off.
- With a non-default `int64`, `parseJson` and `stringifyJson` do the same for webhook payloads.
- Enums are `const` objects with a union type of their values.
- Tagged unions are unions of interfaces keyed by the discriminator.
- Expandable fields are `string | Customer`, and `expandableId(value)` gives the id either way.
- A union of objects parses with the serializer of the variant it matches, and keeps other
  objects as received.

### Notes

- The webhook verifier has no Node.js imports: it runs in browsers, Workers and edge runtimes.
- `exports` under `[typescript]` re-exports more modules from the entry point.

## Python

### Install

```sh
pip install acme
```

Python 3.10+, `httpx` only. Generated code passes `mypy --strict`, pyright and ruff.

### Client

```python
client = Acme(api_key="sk_live_...", timeout=20.0, max_retries=5)

async with AsyncAcme() as client:
    customer = await client.customers.retrieve("cus_1")
```

- Arguments are keyword-only: `api_key`, `base_url`, `timeout` (seconds, `None` waits),
  `max_retries`, `default_headers`, `http_client` (an `httpx.Client`), `middleware`.
- Credentials: `token_provider`, `client_id`, `client_secret`, `oauth_client_auth`,
  `basic_auth=(user, password)`, `api_keys`.
- `AsyncAcme` is the asyncio client, with the same resources.
- Close the client, or use it as a context manager, to release its connections.
- `client.with_options(max_retries=0)` returns a copy with other settings.

### Calls and options

```python
client.customers.create(currency="EUR", name="x")
client.customers.retrieve("cus_1", timeout=5.0, max_retries=0)
```

- Path parameters come first. Query, header and object body fields are keyword arguments.
- An omitted argument is not sent. `None` sends `null` to a nullable field, and is omitted
  elsewhere.
- Enum arguments take the enum or its literal value (`Currency | CurrencyLiteral`).
- Lists, unions, multipart and binary bodies, or a body with a field named like a parameter,
  are one `body` argument.
- Every method takes `extra_headers=`, `extra_query=`, `extra_body=`, `timeout=` and
  `max_retries=`, prefixed with `request_` when a parameter has that name.
- These headers, like `default_headers`, win over the client's credentials.
- Multipart file fields take bytes, a binary file, or `Upload(content, filename, content_type)`.
- An operation that also declares a bodiless 2xx returns `Model | None`.

### Pagination

```python
for customer in client.customers.list(per_page=100): ...
page = client.customers.list()          # page.items, page.body, page.has_next_page(), page.get_next_page()
async for customer in async_client.customers.list(): ...
```

List methods return a `SyncPage`, iterated item by item; `iter_pages()` walks the pages. The
async client returns an `AsyncPaginator`: `await` it for the first page, or `async for` the items.

### Streaming

```python
with client.completions.create_stream(prompt="hi") as stream:
    for chunk in stream:
        print(chunk.delta)
```

Events with a documented schema yield models until `[DONE]`, with `stream.last_event` the raw
event. Other streams yield `SseEvent`s.

### Errors

```python
try:
    client.customers.retrieve("cus_1")
except NotFoundError as error:
    print(error.status_code, error.request_id, error.body)
except APITimeoutError: ...
except APIConnectionError: ...
```

| Error | When |
|---|---|
| `AcmeError` | Base of every error |
| `APIStatusError` | Non-2xx: `status_code`, `body` (decoded into its schema, else JSON), `request_id` |
| `BadRequestError`, `AuthenticationError`, `PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`, `RateLimitError`, `InternalServerError` | Subclasses by status |
| `APIConnectionError`, `APITimeoutError` | No response, or none within the timeout |
| `APIResponseValidationError` | A 2xx body that does not decode |

The message quotes the start of the body: `Error code: 404 - {"error": ...}`.

### Raw responses

`client.with_raw_response.customers.retrieve(id)` returns an `APIResponse` with `status_code`,
`headers`, `request_id` and `parse()`.

### Models and unions

- Models are keyword-only dataclasses with `from_dict` and `to_dict`.
- Unknown properties are kept in `extra_fields`, readable as attributes, and sent back.
- In request models, an optional nullable field defaults to `UNSET`: omitted, while `None` sends
  `null`. In response models it is `X | None = None`.
- A property named after a model member (`extra_fields`, `to_dict`) gets a trailing `_`.
- A discriminated union is the union of its variants:
  `t.Annotated[Circle | Square | UnknownVariant, Discriminator(...)]`.
- When its variants share fields or reuse a model, it is a model holding the variant:
  `Shape(content=Circle(radius=1))`, with the tag filled in.
- Expandable fields are `str | Customer`, and `expandable_id(value)` gives the id either way.
- A union of objects is `t.Annotated[Customer | DeletedCustomer | UnknownVariant,
  ObjectUnion(...)]`. `as_variant(value, Customer)` reads it as another variant.

### Notes

- `errors.py`, which re-exports the error types, is yours after the first generation.
- Models and resources are imported the first time they are used, so importing the package
  stays fast on large APIs.

## Go

### Install

```sh
go get github.com/acme/acme-go
```

Go 1.23+. The module path is set by `module` under `[go]`, else derived from the repository.

### Client

```go
client := acme.New("sk_live_...", &acme.Options{ServerURL: "https://staging.acme.com"})
```

| `Options` field | |
|---|---|
| `ServerURL` | Else `ACME_BASE_URL`, else `DefaultServerURL` |
| `Timeout`, `MaxRetries`, `RetrySchedule` | Defaults: `DefaultTimeout`, 2, exponential backoff |
| `HTTPClient`, `Middleware`, `UserAgent` | Transport, [middleware](customizing.md#middleware) |
| `Logger` | A `*slog.Logger`, logging every attempt at debug level |
| `TokenProvider`, `ClientID`, `ClientSecret`, `OAuthClientAuth`, `BasicAuth`, `APIKeys` | [Credentials](features.md#authentication) |

`New("", nil)` reads `ACME_API_KEY`. Without any server URL, every call fails with a
`*RequestError` naming both settings.

### Calls and options

```go
customer, err := client.Customers().Retrieve(ctx, "cus_1", acme.WithMaxRetries(0), acme.WithTimeout(5*time.Second))
```

- Every method takes a `context.Context` first, then required parameters.
- Optional query and header parameters go in a `*...Options` struct of pointers
  (`acme.Ptr(v)`), `nil` for none.
- Per-call options: `WithHeader`, `WithTimeout`, `WithIdempotencyKey`, `WithMaxRetries`,
  `WithResponseInto`.
- An operation that may answer a bodiless 2xx returns nil for it, scalars as a pointer.

### Pagination

```go
for customer, err := range client.Customers().ListAutoPaging(ctx, nil).All() { ... }
page, err := client.Customers().List(ctx, nil) // page.Items, page.Body, page.HasNextPage(), page.NextPage(ctx)
```

- List methods return a page, such as `*CustomersListPage`, an alias of `*Page[Customer, *CustomerList]`.
- `Body` is the whole typed response, for totals.
- The `ListAutoPaging` twin returns an `*AutoPager[Customer]`: `range pager.All()`, or
  `Next()`, `Current()` and `Err()`.

### Streaming

```go
stream, err := client.Completions().CreateStream(ctx, acme.CompletionRequest{Prompt: "hi"})
defer stream.Close()
for chunk, err := range stream.All() { ... } // stream.Event() is the raw event
```

Events with a schema give a `*Stream[T]` ending at `[DONE]`, others an `*EventStream` of
`SSEEvent`s. Lines are capped at 1 MiB.

### Errors

```go
var apiErr *acme.APIError
switch {
case errors.Is(err, acme.ErrNotFound):
case errors.As(err, &apiErr):
	log.Printf("status %d, request %s", apiErr.StatusCode, apiErr.RequestID())
}
```

- Every error is an `SDKError`: `*APIError`, `*TimeoutError`, `*TransportError` (no
  response), `*DecodeError`, `*RequestError`.
- `*DecodeError` also covers a 2xx body missing a required property, or holding it as `null`
  when it is not nullable: it is not decoded as the zero value.
- `errors.Is` tests the status: `ErrNotFound`, `ErrUnauthorized`, `ErrRateLimited`, `ErrServer`...
- `APIError.Body` holds the body decoded as the declared error schema, else plain JSON.
- `ErrorBody[T](err)` decodes it as any schema, `APIError.Detail()` as the API-wide one.

### Raw responses

`WithResponseInto(&resp)` hands over the `*http.Response`, for its status and headers.

### Models and unions

- Initialisms are spelled the Go way: `CustomerID`, `APIKey`.
- `allOf` parts are inlined into flat structs. Unknown properties are kept in `ExtraFields`.
- Nullable optional PATCH fields are `*Nullable[T]`: `NewNullable(v)` sets one,
  `ExplicitNull[T]()` sends `null`. `IsNull()` is true only for an explicit null.
- A union is a struct with one `Of...` field per variant, at most one set: `OfString *string`,
  `OfCustomer *Customer`, `OfEmpty bool` for `""`.
- `New...From...` constructors build one. `Kind()` names the variant set, `As...()` returns it.
- `Raw()` holds values no variant matches. `ID()` returns the id of an expandable field.
- `As(&target)` decodes a union of objects as another variant.
- A tagged union has a `Type` field of its own string type (`ShapeType`, with `ShapeCircle`...
  constants), and a pointer per variant. Variants fill in an empty discriminator.
- Inline object variants of tagged unions are structs of their own: `ContentPartTextVariant`.

### Notes

- The package of a multi-word name is one word: `realworld`.
- Generated files start with `// Code generated by perseid. DO NOT EDIT.`.
- `doc.go`, `errors.go` and `version.go` are yours. Add methods to the resource types from any
  file of the package.

## Java

### Install

```kotlin
implementation("com.acme:acme:0.1.0")
```

Java 11+, OkHttp and Jackson underneath.

### Client

```java
try (Acme client = Acme.fromEnv()) { ... }

Acme client = new Acme(AcmeOptions.builder()
        .apiKey("sk_live_...")
        .timeout(Duration.ofSeconds(20))
        .maxRetries(3)
        .build());
```

- `AcmeOptions` is immutable. Its builder also takes `baseUrl`, `header`, `retrySchedule`,
  `debug`, `httpClient(OkHttpClient)` and `addInterceptor`.
- Credentials: `tokenProvider`, `clientCredentials(id, secret)`, `clientAuthInBody`,
  `basicAuth`, `putApiKey`.
- The base URL defaults to `ACME_BASE_URL`, then `Acme.DEFAULT_BASE_URL`. Without either, the
  constructor throws an `IllegalStateException`.
- The client is `AutoCloseable`. A client given to `httpClient` is left open.
- Requests are logged through `System.Logger` at `DEBUG`, or `INFO` with `debug(true)`.

### Calls and options

```java
var customer = client.customers().retrieve("cus_1",
        RequestOptions.builder().timeout(Duration.ofSeconds(5)).maxRetries(0).build());
```

- Required path, query and header parameters are arguments. Optional ones go in an immutable
  `...Options.builder()`.
- Every method has an overload taking `RequestOptions` last.
- `client.async()` has the same methods, returning `CompletableFuture`s.
- An operation that may answer a bodiless 2xx returns an `Optional`.

### Pagination

```java
for (Customer customer : client.customers().listIter()) { ... }
Page<Customer> page = client.customers().listIter().firstPage(); // items(), hasNextPage(), nextPage()
```

`...Iter` methods return a `Paginator`: `Iterable`, `stream()`, `firstPage()`, `pages()`. The
async client returns an `AsyncPaginator` with `forEach` and `toList`.

### Streaming

```java
try (var events = client.completions().createStream(request)) {
    for (var chunk : events) { ... }
}
```

Event streams are `EventStream<Chunk>`s, ending at `[DONE]`, with `lastEvent()` for the raw event.

### Errors

| Exception | When |
|---|---|
| `AcmeException` | Base of every exception, all unchecked |
| `ApiException` | Error response: `statusCode()`, `headers()`, `body()`, `requestId()`, `error()` (the body parsed into the status's error schema, else a `JsonNode`), `error(Type.class)` |
| `BadRequestException`, `AuthenticationException`, `PermissionDeniedException`, `NotFoundException`, `ConflictException`, `UnprocessableEntityException`, `RateLimitException`, `InternalServerException` | Subclasses by status |
| `ApiConnectionException`, `ApiTimeoutException` | No response, or none within the timeout |
| `InvalidDataException` | A response that is not what the API describes, such as a missing required property, thrown by its getter |

### Raw responses

`client.withRawResponse().customers().retrieve(id)` returns an `ApiResponse<Customer>` with
`statusCode()`, `headers()`, `requestId()` and `body()`.

### Models and unions

- Models are immutable final classes: `Customer.builder()...build()` throws on a missing required
  property, and `toBuilder()` changes a copy.
- Required properties are read directly, others as `Optional`s.
- For an optional nullable property, `x(null)` sends `null` and leaving it unset omits it.
- Unknown properties are kept in `additionalProperties()`.
- Enums are classes with a constant per value. `isKnown()`, `value()` (with `_UNKNOWN`) to
  `switch` on, `known()` (throws on unknown values), and `asString()` or `asLong()`.
- Unions have `of...` factories (`ChargeCustomer.ofString("cus_1")`), `is...()` and `as...()`
  per variant, and `id()` for expandable objects.
- Unmatched objects are `Unrecognized` (`isUnrecognized()`), and `decodeAs(Customer.class)` reads
  a value as another variant.
- `accept(Visitor<R>)` has a `visitX` per variant. `visitUnknown` throws `InvalidDataException`
  unless overridden.

### Notes

- `exceptions/ApiException.java` is yours after the first generation.
- The HTTP plumbing lives in an `internal` package.

## C#

### Install

```sh
dotnet add package Acme
```

.NET 8, nullable reference types, System.Text.Json source generation: trimming and native AOT
safe. Builds clean with `AnalysisLevel` `latest-recommended`.

### Client

```csharp
using var client = new AcmeClient("sk_live_...", new AcmeClientOptions { MaxRetries = 5 });
```

- `new AcmeClient()` reads `ACME_API_KEY` and `ACME_BASE_URL`.
- Options: `BaseUrl`, `Token`, `Timeout`, `MaxRetries`, `RetrySchedule`, `UserAgent`, `Handlers`
  ([middleware](customizing.md#middleware)), `HttpMessageHandler`.
- Credentials: `TokenProvider`, `ClientId`, `ClientSecret`, `OAuthClientAuthInBody`,
  `BasicAuth`, `ApiKeys`.
- `new AcmeClient(httpClient, token)` sends requests through your own `HttpClient`.
- Without a base URL, the constructor throws an `AcmeException`.
- The client is thread-safe and pools connections: create one and reuse it.

### Calls and options

```csharp
await client.Customers.RetrieveAsync("cus_1",
    requestOptions: new RequestOptions { MaxRetries = 0, Timeout = TimeSpan.FromSeconds(5) });
```

- Every method is async and takes a `RequestOptions` (`Headers`, `Timeout`, `MaxRetries`,
  `IdempotencyKey`), then a `CancellationToken`.
- Optional parameters go in an options object: `ListAsync(new() { PerPage = 100 })`.
- An operation that may answer a bodiless 2xx returns `Customer?`.

### Pagination

```csharp
await foreach (var customer in client.Customers.ListAutoPagingAsync(new() { PerPage = 100 })) { ... }
var page = await client.Customers.ListAutoPagingAsync().GetFirstPageAsync();
```

`AsyncPager` gives pages through `AsPagesAsync()` and `GetFirstPageAsync()`. A `Page` has
`Items`, `Response`, `HasNextPage` and `GetNextPageAsync()`.

### Streaming

```csharp
await using var stream = await client.Completions.CreateStreamAsync(new CompletionRequest { Prompt = "hi" });
await foreach (var chunk in stream) { ... }
```

Typed events give an `EventStream<T>` of models ending at `[DONE]`, with `LastEvent` for the raw
event. Streams are enumerated once.

### Errors

| Exception | When |
|---|---|
| `AcmeException` | Base of everything the SDK throws |
| `ApiException` | Error response: `StatusCode`, `Headers`, `Body` (truncated in the message), `Error`, `GetError<T>()`, `RequestId` |
| `BadRequestException`, `UnauthorizedException`, `NotFoundException`, `RateLimitException`, `ServerErrorException`... | Subclasses by status |
| `ApiConnectionException`, `ApiTimeoutException` | No response, or none within the timeout |
| `ApiDecodeException` | A 2xx body the SDK cannot read |

`Error` is the body parsed as the schema declared for the status, else a `JsonElement`.

### Raw responses

`await client.Customers.WithRawResponse.RetrieveAsync(id)` returns an `ApiResponse<Customer>`
with `StatusCode`, `Headers`, `RequestId` and `Value`.

### Models and unions

- Models are `sealed record`s with `init` properties and read-only collections, compared by
  value. Unknown properties are kept in `AdditionalProperties`, and read as the values a typed
  `additionalProperties` declares with `TypedAdditionalProperties()`.
- Dates are `DateTimeOffset`s (`DateOnly` for `format: date`): fractions beyond 100 nanoseconds
  are rounded.
- Nullable optional PATCH fields are `MaybeUnset<T>`: assign `null` to send `null`, leave unset
  to omit.
- Enums expose `IsKnown`. Known values are static properties (`Status.Active`) and constants in
  `Status.Values`, to `switch` on `status.Value`. A string converts to any value.
- A union is an abstract record with a nested record per variant (`StringValue`, `Customer`,
  `ArrayOfIntegers`...), implicit conversions, `AsX` accessors and `Id` for expandable objects.
- Unmatched values are `Unrecognized`. `DecodeAs(context.Customer)` reads a union of objects
  as another variant.
- Inline body unions are named after the operation: `CreateTranscriptionResponse`.

### Notes

- Each resource implements an interface (`IAcmeClient.Customers` is an `ICustomersApi`) to mock
  in tests.
- Each call is an `Activity` of the `ActivitySource` named after the package, for OpenTelemetry.
- `dependency_injection = true` under `[csharp.context]` adds
  `services.AddAcmeClient(o => o.Token = ...)`, an `IHttpClientFactory` typed client. Its
  `HttpClient` leaves the timeout to the SDK.
- `ApiException.cs` is yours after the first generation.
