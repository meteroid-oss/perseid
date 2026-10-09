# Languages

Examples use an API named `Acme` with a `customers` resource. Names follow the `name` of
`perseid.toml`: the client is `Acme`, the environment variables start with `ACME_`.

## What every SDK does

| Behavior | Description |
|---|---|
| Retries | Connection errors, timeouts, 408, 429 and 5xx, retried twice by default with jittered exponential backoff |
| Safe retries only | Idempotent methods, or requests with an `Idempotency-Key`. Every POST gets one automatically with [`idempotency_keys = true`](configuration.md#spec-name-and-sdks). A 429 is retried whatever the method, as the server refused the request before processing it |
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

| Language | Requires | Client | Base error | Paginated list | Raw response |
|---|---|---|---|---|---|
| Rust | Tokio | `Acme::builder()...build()?` | `Error` | `.items()` on `list()` | `.with_response()` |
| TypeScript | Node.js 20+, Deno, Bun, browsers | `new Acme({ apiKey })` | `AcmeError` | `for await` on `list()` | `.withResponse()` |
| Python | Python 3.10+ | `Acme(api_key=...)`, `AsyncAcme` | `AcmeError` | iterate `list()` | `client.with_raw_response` |
| Go | Go 1.23+ | `acme.New(token, opts)` | `SDKError` | `ListAutoPaging(...)` | `WithResponseInto(&resp)` |
| Java | Java 11+ | `new Acme(AcmeOptions...)` | `AcmeException` | iterate `list()` | `client.withRawResponse()` |
| C# | .NET 8 | `new AcmeClient(token)` | `AcmeException` | `await foreach` on `ListAsync()` | `.WithRawResponse` |

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

| Language | Tests | Run |
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

| Language | Test | Samples |
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

Cargo features: `rustls-tls` (default) or `native-tls`, `http1` (default), `http2`, `webhooks` for
the [webhook verifier](customizing.md#webhooks), and `tracing` for [logs](#logging).

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
- Only `from_env()` and `AcmeBuilder::from_env()` read the environment variables: `builder()` and
  `new` take nothing from them.
- The builder also takes `header`, `middleware`, `http_client`, and `connector` for any hyper
  connector (custom TLS roots, client certificates, a proxy).
- The default client goes through the proxy of `HTTPS_PROXY`, `HTTP_PROXY` or `ALL_PROXY` (or
  their lowercase names), read when the client is built: an `http://` proxy, CONNECT tunnelling
  HTTPS, or `socks5://`. Hosts, domains with their subdomains, IPs and networks listed in
  `NO_PROXY` connect directly, and so does a client given its own `connector` or `http_client`.
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
  parameter, and its `maybe_*` twin taking an `Option`. When none is required, pass the struct
  or `None`.
- A path parameter typed by a string schema of the spec takes `impl Into` of its newtype:
  `retrieve("cus_1")` and `retrieve(&customer.id)` both work.
- An optional body is `impl Into<Option<T>>`: pass the body, or `None`.
- `with_options` on a resource sets headers, timeout, retries or idempotency key for its calls.
- An operation that also declares a bodiless 2xx returns `Option<T>`.
- Files are `Upload`s: `Upload::path("a.csv").await?` reads the file, named after it and typed by
  the part's media type in the spec, else by its extension; `Upload::bytes(...)` and
  `Upload::reader(...)` take `with_filename` and `with_content_type`. Readers stream and are not
  retried.

### Pagination

```rust
let page = client.customers().list(None).await?;    // the first page
page.total;                                         // a response field, through Deref
for customer in page.items() { ... }
let next = page.next_page().await?;                 // None after the last page

let mut customers = client.customers().list(None).items();
while let Some(customer) = customers.next().await {
    let customer = customer?;
}
```

- List methods return a `PageCall`. Awaited, it gives a `Page` that dereferences to the
  response body, with `items()`, `into_items()`, `has_next_page()`, `next_page()` and
  `into_inner()`, the body itself.
- `.items()` is a `Paginator`, a `futures_core::Stream` of every item fetched page by page, with
  `next()` and `collect()`; `.pages()` streams the pages; `.with_response()` gives the first
  page with its status and headers.

### Streaming

Event streams are `Stream`s of the model each event carries, ending at `[DONE]`, or of raw
`SseEvent`s. `last_event()` gives the raw event, `into_raw()` the raw stream.

- An `error` event, whatever its data, or data that is an object with an `error` the model does
  not decode or does not declare, ends the stream with an `Error::Api` holding that data and the
  response's status and headers.
- Comments are skipped, and so are `ping` and `keepalive` events that are not the model.
- The `_stream` twin of a multipart operation sends its `stream` part as `true`, the other leaves
  it out, and neither body struct has a `stream` field.

A file download (a binary response) is a `BinaryResponse`, its body unread:

```rust
let audio = client.audio().speech(request).await?.bytes().await?;
let mut download = client.files().content(id).await?;
tokio::io::copy(&mut download, &mut tokio::fs::File::create("out.bin").await?).await?;
```

- `bytes()` reads the whole body, `chunk()` and the `futures_core::Stream` give its chunks, and
  as a `tokio::io::AsyncRead` it streams anywhere; `status()`, `headers()` and
  `content_length()` are there before the body.
- An error status fails the call before any body. Retries end once the headers arrive.
- The timeout covers the headers, then each read of the body: a long download is not cut short,
  a stalled one fails with `Error::Timeout`. Dropping the response closes the connection.

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
  `payload()` (the API's common error schema, `api::ErrorBody`), `json::<T>()`, `text()` and
  `message()`: the body's `error.message`, `message` or `detail` string.
- An API error displays as `API error (404 Not Found): No such customer`, its `message()`, else
  its body.
- Methods list their documented error bodies under `# Errors`.

### Raw responses

`client.customers().retrieve(id).with_response().await?` returns the `status()`, `headers()`,
`request_id()` and `into_data()`.

### Logging

With the `tracing` feature, each attempt is a `tracing` DEBUG event (method, URL, status or error
kind, elapsed time, retry count), and each retry an INFO event with its delay, for the subscriber
the application installs. Headers, the query string, user info, credentials and bodies are never
logged.

### Models and unions

- Structs keep undeclared properties in `extra` (`extra_properties` when the schema has an
  `extra` property). `allOf` parts are inlined into one struct.
- Structs only found in responses are `#[non_exhaustive]`: build them with `new(required...)` or
  `Default`, then assign fields. Request structs also take struct literals, and a chainable
  setter per field `new` leaves out: `CustomerUpdate::new().name("Ada")`.
- `new` takes strings, enums and unions as `impl Into<_>`:
  `CreateChatCompletionRequest::new(vec![UserMessage::new("hi").into()], "gpt-4o")`.
- `Default` is implemented when every required field has a default.
- In the structs only requests send and in PATCH bodies, nullable optional fields are
  `Option<Option<T>>`: `Some(None)` sends `null`, `None` leaves the field out. Their setters wrap
  the value, and `clear_*()` sends `null`. Structs responses carry too read them as `Option<T>`:
  `null` decodes to `None`, which is left out when sent.
- A named string schema, such as `CustomerId`, is a newtype over `String`: built `From` any
  string, read as `&str` through `Deref`, `as_str()` or `Display`, compared with strings. One ID
  type cannot be passed where another is expected.
- Recursive fields are boxed, and so are union variants far larger than the others, which
  clippy's `large_enum_variant` would flag. Dates are `chrono` types.
- Enums and unions are `#[non_exhaustive]`, with an `Unknown` variant that serializes back
  unchanged. String enums are built `From` a `&str` or `String`, or parsed with `FromStr`.
- Unions are enums with `From` impls and `as_*` accessors: `ChargeCustomer::String(id)`,
  `ChargeCustomer::Customer(Box<Customer>)`. Expandable fields have `id()`.
- Discriminated unions convert `From` each variant's struct when no other variant holds it:
  `Shape::from(Circle::new(1.5))`. Variant structs fill in their discriminator.
- Variants declaring the same tag, as OpenAI's three `message` input items, are sent with it,
  and decoded as the first whose fields the data fits.
- `decode_as::<DeletedCustomer>()` reads a union of objects as another variant.

### Notes

- `src/error.rs` is yours. The runtime only calls `Error::generic(Failure)` and
  `Error::from_response(status, headers, body)`.
- `Cargo.toml` is yours too. perseid only appends the crates and features it lacks, like the
  `tracing` feature for a crate generated before it. It leaves out the `default` feature.
- The `http` crate is re-exported as `acme::api::http`.

## TypeScript

### Install

```sh
npm install acme
```

ESM and CommonJS builds behind an `exports` map. The package only needs `fetch`, and
type-checks under `strict`, `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`. Its
`build` script bundles each entry point with esbuild, so that Node loads one module rather than
one per model.

### Client

```ts
import Acme from "acme";   // or { Acme }

const client = new Acme({ apiKey: "sk_live_...", timeout: 20_000, maxRetries: 5 });
const fast = client.withOptions({ timeout: 5_000 });   // a copy, other options kept
```

When every operation requires an API key or bearer token, a client without credentials (no
`apiKey`, `ACME_API_KEY`, `tokenProvider`, `apiKeys` or `Authorization` default header) throws
an `AcmeError` up front.

| Option | Description |
|---|---|
| `apiKey`, `baseURL` | Default to `ACME_API_KEY` and `ACME_BASE_URL` |
| `timeout` | Milliseconds, `Infinity` to wait forever |
| `maxRetries`, `retryScheduleInMs` | Retry count, or explicit delays |
| `defaultHeaders`, `defaultQuery` | Sent with every request. A `null` header removes one the SDK sets |
| `fetch`, `middleware`, `debug` | Custom `fetch`, [middleware](customizing.md#middleware), a summary of each request on stderr, prefixed with the npm package name |
| `tokenProvider`, `clientId`, `clientSecret`, `oauthClientAuth`, `basicAuth`, `apiKeys` | [Credentials](features.md#authentication) |

### Calls and options

```ts
const customer = await client.customers.retrieve("cus_1", { timeout: 5_000, maxRetries: 0 });
```

- The last argument of every method is `{ signal, headers, query, timeout, maxRetries, idempotencyKey }`.
- Unions of scalars, lists and objects are typed in query, header and path parameters.
- A multipart file is a `Blob`, `File`, `Uint8Array`, `ArrayBuffer`, `ReadableStream`, fetch
  `Response`, Node `fs.ReadStream` (any `AsyncIterable` of bytes), or `{ data, filename?,
  contentType? }`. `{ path }` reads a file on Node and is named after it, as a `Response` is
  after its URL, and typed by the part's media type in the spec, else by its extension. The file
  is read once, before the first attempt, so retries resend it.
- `int64 = "bigint"` or `"string"` under `[typescript]` parses int64 values without losing digits.
  With `"string"`, the int64 values of a union variant are `number | bigint`, as a string would
  read as a string variant.

### Pagination

```ts
for await (const customer of client.customers.list({ perPage: 100 })) { ... }
const page = await client.customers.list();
page.total;                                       // a response field, read on the page
page.items; page.hasNextPage(); await page.getNextPage();
```

- List methods return a `PagePromise`: await it for the first page, or `for await` every item.
- A `Page<Body, Item>` has the response's properties, typed, with `items`, `hasNextPage()`,
  `getNextPage()`, `iterPages()`, `body` (the response as decoded) and `response` (the HTTP
  response). `JSON.stringify(page)` writes the body.

### Streaming

```ts
const stream = await client.completions.createStream({ prompt: "hi" });
for await (const chunk of stream) process.stdout.write(chunk.delta);
```

- Events with a schema give a `Stream<Model>` ending at `[DONE]`, with `stream.lastEvent`.
- The body of the non-stream twin cannot ask for the stream: `create({ stream: true })` does not
  compile. A multipart body has no `stream` part: `createStream` sends it as `true`, `create`
  leaves it out.
- An `error` event, whatever its data, or data with an `error` property the event schema does
  not declare, throws an `APIError` holding it. `ping` and `keepalive` events that are not the
  model are skipped.
- Other streams are an `EventStream` of raw events.
- Breaking out of the loop or calling `stream.close()` closes the connection.

A file download (a binary response) is a `BinaryResponse`, its body unread:

```ts
const audio = await (await client.audio.speech({ input: "hi" })).bytes();
await writeFile("out.bin", await client.files.content(id)); // node:fs/promises, streamed
```

- `bytes()`, `arrayBuffer()`, `blob()` and `text()` read the whole body; `for await` and
  `body` (a `ReadableStream`) give its chunks; `status`, `headers` and `response` (the fetch
  `Response`) are there before the body. `cancel()` leaves the body unread.
- An error status rejects the call before any body. Retries end once the headers arrive.
- The timeout covers the headers, then each read of the body: a long download is not cut short,
  a stalled one fails with `APIConnectionTimeoutError`, any other failed read with
  `APIConnectionError`.
- A typed wrapper rather than the bare fetch `Response`, for these errors and the per-read
  timeout; `asResponse()` still gives the raw `Response`.

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
| `APIError` | Non-2xx: `status`, `headers`, `requestId`, `body` (raw text), `error` (parsed, typed `AcmeErrorBody`). The `message` quotes the body's `error.message`, `message` or `detail` when it has one |
| `BadRequestError`, `AuthenticationError`, `PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`, `RateLimitError`, `InternalServerError` | 400, 401, 403, 404, 409, 422, 429, 5xx |
| `APIConnectionError`, `APIConnectionTimeoutError` | No response, or none within the timeout |
| `APIUserAbortError` | The `signal` aborted |
| `APIDecodeError` | A 2xx body that is not valid JSON, not the expected event stream, or not what its schema describes |

### Raw responses

Methods return an `APIPromise`. `.withResponse()` gives `{ data, response, requestId }`, and
`.asResponse()` the unread `Response`.

### Models and unions

- Models are plain objects with camelCase properties.
- The schemas an `allOf` references are extended (`interface Cat extends PetBase`), unions
  intersected: literals take every property flat, and a property the model redeclares wins.
- `CustomerSerializer.parse(json)` and `.serialize(value)` convert them, keeping unknown
  properties under their JSON names. A typed `additionalProperties` gives the model an index
  signature.
- Parsing checks values against their schema: a wrong type, or a required property missing
  or `null`, throws an `APIDecodeError` naming its path (`$.items[3].total: expected a number,
  got string "abc"`). Unknown properties, unknown enum values and values no union variant holds
  are kept. `validate_responses = false` under `[typescript]` turns the checks off.
- With a non-default `int64`, `parseJson` and `stringifyJson` do the same for webhook payloads.
- Enums are `const` objects with a union type of their values: a typo does not compile. An
  open enum (`anyOf: [string, enum]`) also takes any string, and so do the enums of the models
  only responses carry, as a later API version may send new values. Parsing keeps them either way.
- Tagged unions are unions of interfaces keyed by the discriminator. A tag the variant's schema
  makes optional is optional, filled in when sent. Variants sharing a tag (OpenAI's `message`
  items) are sent with it and told apart by their properties.
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
client.customers.create(currency="EUR", name="x", address={"city": "Paris"})
client.customers.retrieve("cus_1", timeout=5.0, max_retries=0)
client.files.create(file=Upload(b"...", "a.csv"), purpose="import")
```

- Path parameters come first. Query, header and object body fields are keyword arguments,
  `allOf` bodies and required multipart bodies included.
- An omitted argument is not sent. `None` sends `null` to a nullable field, and is omitted
  elsewhere.
- Enum arguments take the enum or its literal value (`Currency | CurrencyLiteral`). An open
  enum, a spec's `anyOf: [string, enum]`, takes any `str` too.
- A model argument also takes its JSON as a dict, typed by `CustomerParam`, a `TypedDict` keyed
  by JSON names. A union of models takes the dict of a variant, its tag included. Lists of them
  take any sequence.
- Lists, unions, optional multipart and binary bodies, or a body with a field named like a
  parameter, are one `body` argument.
- The `_stream` twin of a method sets the body's `stream` itself, multipart or JSON.
- Every method takes `extra_headers=`, `extra_query=`, `extra_body=`, `timeout=` and
  `max_retries=`, prefixed with `request_` when a parameter has that name.
- These headers, like `default_headers`, win over the client's credentials.
- Multipart file fields take bytes, a binary file, a path (`Path("a.csv")`, streamed from disk,
  again for a retry, named after it and typed by the part's media type in the spec, else by its
  extension), or `Upload(content, filename, content_type)`.
- An operation that also declares a bodiless 2xx returns `Model | None`.

### Pagination

```python
for customer in client.customers.list(per_page=100): ...
page = client.customers.list()          # CustomersListPage, a CustomerList
page.total                              # a response field, read on the page
page.items, page.has_next_page(), page.get_next_page()
async for customer in async_client.customers.list(): ...
```

- List methods return the first page, such as `CustomersListPage`: a subclass of the response
  model with `items`, `has_next_page()`, `get_next_page()`, `iter_pages()` and `body`, the
  response as decoded. Iterating it yields every item, fetching pages on demand.
- The async client returns an `AsyncPaginator`: `await` it for an `AsyncCustomersListPage`, or
  `async for` the items.

### Streaming

```python
with client.completions.create_stream(prompt="hi") as stream:
    for chunk in stream:
        print(chunk.delta)
```

Events with a documented schema yield models until `[DONE]`, with `stream.last_event` the raw
event. Other streams yield `SseEvent`s. An `error` event, whatever its data, or data with an
`error` the model does not decode or declare, raises an `APIStatusError` whose `raw_body` is that
data; `ping` and `keepalive` events that are not the model are skipped.

A file download (a binary response) is a `BinaryResponse` (`AsyncBinaryResponse`), its body
unread:

```python
audio = client.audio.speech(input="hi").read()
with client.files.content(file_id) as download:
    download.write_to_file("out.bin")
```

- `read()` gives the whole body (kept for later calls), `iter_bytes(chunk_size=None)` (or
  iterating it) the chunks as they arrive and `write_to_file(path)` streams them to disk; each
  releases the connection. `status_code`, `headers`, `content_type` and `response` (the `httpx`
  response) are there before the body.
- An error status raises before any body. Retries end once the headers arrive.
- The timeout applies to each read, as `httpx` does: a long download is not cut short.
- Use it in a `with` (`async with`) block, or call `close()`, when the body may be left unread.
  A sync response dropped unread closes itself; an async one keeps its connection until closed.

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

The message quotes the start of the body: `Error code: 404 - {"error": ...}`. `request_id` is
the `x-request-id` (or `request-id`) header of the response, `None` without one.

### Raw responses

`client.with_raw_response.customers.retrieve(id)` returns an `APIResponse` with `status_code`,
`headers`, `request_id` and `parse()`.

### Logging

```sh
ACME_LOG=debug python app.py
```

- The `acme` logger, named after the package, records each attempt at DEBUG (method, URL,
  status or error type, elapsed time, retry count) and each retry at INFO, with its delay.
- `ACME_LOG=debug` or `info` sets that level, unless the logger already has one, and logs to
  stderr unless a handler is configured. Without it, records only reach the handlers you set up.
- Headers, the query string, credentials and bodies are never logged.

### Models and unions

- Models are keyword-only dataclasses with `from_dict` and `to_dict`. The parts of an `allOf`
  are inlined: their fields are the model's own.
- In models only requests send, enum fields also take their values, as arguments do.
- Unknown properties are kept in `extra_fields`, readable as attributes, and sent back.
- In models only requests send and in PATCH bodies, an optional nullable field defaults to
  `UNSET`: omitted, while `None` sends `null`. In models responses carry too it is
  `X | None = None`, `None` omitted unless it was received as `null`. A method's keyword
  arguments tell them apart whatever the model: `note=None` sends `null`, leaving it out omits it.
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

| `Options` field | Description |
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
  `WithResponseInto`. `WithQuery(name, value)` and `WithJSONSet("metadata.source", v)` send a
  query parameter or a body property the SDK does not know yet.
- A multipart file is an `Upload`. `acme.UploadFile("a.mp3")` opens the file for each attempt;
  `Upload{Reader: f}` with an `*os.File` is named after it. Its media type defaults to the
  spec's unless `application/octet-stream`, then to its extension's.
- The `...Stream` twin of a multipart operation sends its `stream` part as true; the other
  leaves it out.
- An operation that may answer a bodiless 2xx returns nil for it, scalars as a pointer.

### Pagination

```go
page, err := client.Customers().List(ctx, nil) // *CustomersListPage
page.Total                                     // a response field, promoted
page.Items; page.HasNextPage(); page, err = page.NextPage(ctx)
for customer, err := range client.Customers().ListAutoPaging(ctx, nil).All() { ... }
```

- List methods return a page, such as `*CustomersListPage`, that embeds the response
  (`page.CustomerList`) with `Items`, `HasNextPage()` and `NextPage(ctx)`, nil after the last
  page. `json.Marshal(page)` writes the response.
- The `ListAutoPaging` twin returns an `*AutoPager[Customer]`: `range pager.All()`, or
  `Next()`, `Current()` and `Err()`.

### Streaming

```go
stream, err := client.Completions().CreateStream(ctx, acme.CompletionRequest{Prompt: "hi"})
defer stream.Close()
for chunk, err := range stream.All() { ... } // stream.Event() is the raw event
```

Events with a schema give a `*Stream[T]` ending at `[DONE]`, others an `*EventStream` of
`SSEEvent`s. Lines are capped at 1 MiB. An error the API sends in a `*Stream[T]`, as an
`error` event or an object with an `error`, ends it with an `*APIError` of the stream's status;
`ping` and `keepalive` events that are not a `T` are skipped.

```go
file, err := client.Files().Content(ctx, "file_1") // *acme.BinaryResponse, returned once the headers arrive
data, err := file.Bytes()                          // or io.Copy(dst, file), or file.WriteToFile("a.pdf")
```

A binary response (`application/octet-stream`, `image/png`, `audio/mpeg`...) is a
`*BinaryResponse`: an `io.ReadCloser` over the body as it arrives, with `Header` and
`ContentLength`. `Bytes()` and `WriteToFile(path)` read it whole and close it; otherwise close
it. `WriteToFile` writes to a temporary file next to `path`, renamed over it once the body is whole,
so a failed download leaves an existing file as it was. An error status is returned as an `*APIError` before any body is handed over, and retries stop
once a 2xx's headers arrive. The timeout covers the wait for the headers, then each `Read`, not the
whole download; cancelling the context stops a read.

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
- `err.Error()` reads `acme: POST /charges: 429 Too Many Requests: <message>`. The message is
  `APIError.Message()`, from `error.message`, `message` or `detail`, else the body cut at 512
  bytes; `RawBody` keeps it whole. An `errors.go` generated before keeps its own `Error()`.

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
- Variants sharing a tag, as OpenAI's three `message` input items, are sent with it. `Type`
  still names the variant (`InputItemInputMessage`, sent as `"message"`), and decoding picks
  the variant knowing the most properties of the object.
- An enum is a string type with typed constants. A field takes `acme.StatusActive` or
  `"active"`; a string variable needs `acme.Status(s)`, a pointer field
  `acme.Ptr(acme.StatusActive)`.

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
  `debug`, `proxy(java.net.Proxy)`, `httpClient(OkHttpClient)` and `addInterceptor`.
- Without `proxy` or `httpClient`, the client goes through the proxy of `HTTPS_PROXY`,
  `HTTP_PROXY` or `ALL_PROXY` (or their lowercase names), read when it is built: an `http://`
  proxy, with the URL's credentials, tunnelling HTTPS, or `socks5://`, whose credentials the JDK
  only takes from the process-wide `java.net.Authenticator`. OkHttp cannot reach a proxy over
  TLS, so an `https://` proxy fails the client's construction. Hosts, domains with their
  subdomains, IPs and networks listed in `NO_PROXY` connect directly. Without these variables,
  the JVM's proxy settings apply.
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
- Files are `Upload`s: `Upload.of(Path.of("a.mp3"))` is named after the file, and fails at once
  when it cannot be read; `Upload.of(bytes)`, `of(File)` and `of(InputStream, length)` take
  `withFilename` and `withContentType`. Without `withContentType`, a part takes the media type of
  the spec unless `application/octet-stream`, else the one of its file name's extension. Input
  streams are not retried, nor is a form holding one.
- The `...Stream` twin of an operation whose body has a `stream` flag sends it as `true`: on a
  copy of a JSON body, and in place of the property the form bodies' builders of both twins
  leave out.

### Pagination

```java
for (Customer customer : client.customers().list()) { ... }
CustomersListPage page = client.customers().list();
page.total();                                   // a response property, read on the page
page.items(); page.hasNextPage(); page.nextPage();
```

- List methods return the first page, such as `CustomersListPage`: a getter per response
  property, `items()`, `hasNextPage()`, `nextPage()`, `pages()` and `body()`, the response as
  decoded. It is `Iterable` over every item, fetching pages on demand, and has `stream()`.
- The async client returns a `CompletableFuture<CustomersListAsyncPage>`, whose `nextPage()`
  is a future too, with `forEach`, `forEachPage` and `toList`.

### Streaming

```java
try (var events = client.completions().createStream(request)) {
    for (var chunk : events) { ... }
}
```

Event streams are `EventStream<Chunk>`s, ending at `[DONE]`, with `lastEvent()` for the raw event.

- An `error` event, or one whose data is an object with an `error` the event model does not
  declare, whatever the event's name, ends the stream with an `ApiException` holding that data and
  the response's status and headers.
- Comments, `ping` and `keepalive` events are skipped.

```java
byte[] pdf = client.files().content("file_1").bytes();
client.files().content("file_1").writeTo(Path.of("a.pdf"));
try (BinaryResponse file = client.files().content("file_1")) {
    file.inputStream().transferTo(out); // file.headers(), contentType(), contentLength()
}
```

A binary response (`application/octet-stream`, `image/png`, `audio/mpeg`...) is a
`BinaryResponse`, returned once its headers arrive: `inputStream()` reads the body as it streams
in, `bytes()` and `writeTo(Path)` read it whole and close it; otherwise close it, e.g. with
try-with-resources. `writeTo` writes to a temporary file next to the path, moved over it once the
body is whole, so a failed download leaves an existing file as it was. An error status is thrown before any body is handed over, and retries stop
once a 2xx's headers arrive. The timeout bounds the wait for the headers, then each read of the
body (OkHttp's read timeout), not the whole download.

### Errors

| Exception | When |
|---|---|
| `AcmeException` | Base of every exception, all unchecked |
| `ApiException` | Error response: `statusCode()`, `headers()`, `body()`, `requestId()`, `error()` (the body parsed into the status's error schema, else a `JsonNode`), `error(Type.class)`. Its message gives the body's `error.message`, `message` or `detail` string, else the body: `POST /charges failed with status 429: Slow down` |
| `BadRequestException`, `AuthenticationException`, `PermissionDeniedException`, `NotFoundException`, `ConflictException`, `UnprocessableEntityException`, `RateLimitException`, `InternalServerException` | Subclasses by status |
| `ApiConnectionException`, `ApiTimeoutException` | No response, or none within the timeout |
| `InvalidDataException` | A response that is not what the API describes, such as a missing required property, thrown by its getter |

### Raw responses

`client.withRawResponse().customers().retrieve(id)` returns an `ApiResponse<Customer>` with
`statusCode()`, `headers()`, `requestId()` and `body()`.

### Models and unions

- Models are immutable final classes: `Customer.builder()...build()` throws on a missing required
  property, and `toBuilder()` changes a copy.
- Builders add to lists and maps one item at a time (`addTagsItem`, `putMetadataItem`). A union
  property also has a setter per variant type, as far as Java overloads tell them apart
  (`content("hi")`, `content(List.of(part))`), and an open enum one a `String` setter
  (`model("gpt-4o")`). Nullable properties have neither, so `x(null)` still sends `null`.
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
- Variants declaring the same tag, as OpenAI's three `message` input items, are sent with it,
  and decoded as the one that has the data's required properties and knows the most of the
  others, else as the variant the tag names.

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
  ([middleware](customizing.md#middleware)), `HttpMessageHandler`, `Log`.
- Credentials: `TokenProvider`, `ClientId`, `ClientSecret`, `OAuthClientAuthInBody`,
  `BasicAuth`, `ApiKeys`.
- `new AcmeClient(httpClient, token)` sends requests through your own `HttpClient`.
- Without a base URL, the constructor throws an `AcmeException`.
- The client is thread-safe and pools connections: create one and reuse it.

### Logging

```csharp
new AcmeClientOptions { Log = attempt => Console.WriteLine(attempt) };
// GET https://api.acme.com/v1/customers: 503 in 120 ms, retried in 500 ms
```

`Log` receives an `ApiAttempt` per HTTP attempt: `Operation`, `Method`, `Url` (without its query
or user info), `Attempt`, `StatusCode` or `Error`, `Elapsed`, `RetryIn`, and `TokenRenewed` when
the API rejected the OAuth2 access token, which is renewed and the attempt made again at once. It
never holds headers, credentials or bodies. Log it at debug level, and at information level when
`IsRetried`. With [dependency injection](#notes), attempts go to the `ILoggerFactory` of the
container, in the `Acme` category; `AcmeServiceCollectionExtensions.LogTo(logger)` sends them to
another `ILogger`.

### Calls and options

```csharp
await client.Customers.RetrieveAsync("cus_1",
    requestOptions: new RequestOptions { MaxRetries = 0, Timeout = TimeSpan.FromSeconds(5) });
```

- Every method is async and takes a `RequestOptions` (`Headers`, `Timeout`, `MaxRetries`,
  `IdempotencyKey`), then a `CancellationToken`.
- Optional parameters go in an options object: `ListAsync(new() { PerPage = 100 })`.
- An operation that may answer a bodiless 2xx returns `Customer?`.
- Files are `Upload`s: `Upload.FromFile(path)`, `FromBytes` and `FromStream` take a file name and
  a content type; streams and files are sent once, without retries. Without a content type, a
  multipart part takes the spec's unless `application/octet-stream`, else its file name's
  extension's.

### Pagination

```csharp
await foreach (var customer in client.Customers.ListAsync(new() { PerPage = 100 })) { ... }
var page = await client.Customers.ListAsync();   // CustomersListPage
page.Total;                                      // a response property, read on the page
page.Items; page.HasNextPage; await page.GetNextPageAsync();
```

- List methods return an `AsyncPager`: `await` it for the first page, or `await foreach` every
  item; `AsPagesAsync()` walks the pages. The cancellation token applies to every request.
- A page, such as `CustomersListPage`, has a property per response property, `Items`,
  `HasNextPage`, `GetNextPageAsync()`, `AsPagesAsync()` and `Body`, the response as decoded.
  `await foreach` over it yields every item from this page on.
- `WithRawResponse` lists return the response of one request, with its status and headers.

### Streaming

```csharp
await using var stream = await client.Completions.CreateStreamAsync(new CompletionRequest { Prompt = "hi" });
await foreach (var chunk in stream) { ... }
```

Typed events give an `EventStream<T>` of models ending at `[DONE]`, with `LastEvent` for the raw
event. Streams are enumerated once. An `error` event, or data that is no `T` but an object with an
`error`, throws an `ApiException` with the status and headers of the response and the event's data
as `Body`; `ping` and `keepalive` events that are no `T` are skipped. The `CreateStreamAsync` twin
of a multipart operation sends `stream=true` itself, and neither body has a `Stream` property.

```csharp
byte[] pdf = await client.Files.ContentAsync("file_1").ReadAsBytesAsync();
await client.Files.ContentAsync("file_1").WriteToFileAsync("a.pdf");
await using var file = await client.Files.ContentAsync("file_1");
var stream = await file.OpenStreamAsync(); // file.Headers, ContentType, ContentLength
```

A binary response (`application/octet-stream`, `image/png`, `audio/mpeg`...) is a
`BinaryResponse`, returned once its headers arrive (`HttpCompletionOption.ResponseHeadersRead`):
`OpenStreamAsync()` reads the body as it streams in, `ReadAsBytesAsync()`, `CopyToAsync(stream)`
and `WriteToFileAsync(path)` read it whole and close it, also straight on the call's task;
otherwise dispose of it. `WriteToFileAsync` writes to a temporary file next to the path, moved
over it once the body is whole, so a failed download leaves an existing file as it was. An error status is thrown before any body is handed over, and retries
stop once a 2xx's headers arrive. The timeout bounds the wait for the headers, then each read of
the body, not the whole download; the call's `CancellationToken` still cancels the reads.

### Errors

| Exception | When |
|---|---|
| `AcmeException` | Base of everything the SDK throws |
| `ApiException` | Error response: `StatusCode`, `Headers`, `Body`, `Error`, `GetError<T>()`, `RequestId` |
| `BadRequestException`, `UnauthorizedException`, `NotFoundException`, `RateLimitException`, `ServerErrorException`... | Subclasses by status |
| `ApiConnectionException`, `ApiTimeoutException` | No response, or none within the timeout |
| `ApiDecodeException` | A 2xx body the SDK cannot read |

`Error` is the body parsed as the schema declared for the status, else a `JsonElement`. The message
reads `HTTP 429 (rate_limit_exceeded): Slow down`: the `code` (else `type`) and `message` of the
body's `error` object, else its `message` or `detail`, else the body, truncated.

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
- A tagged union converts implicitly from each variant's model that no other variant holds:
  `InputItem item = new InputMessage { ... }`. Variants are sent with the tag their schema
  declares; several may share it, and decoding picks the one knowing the most of the object's
  properties.
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
- `ApiException.cs` is yours after the first generation. It formats its message with
  `ApiExceptionExtensions.Describe(statusCode, body)`: call it from the constructor of an older
  one, which reads `Acme API error (status 429): {body}`.
