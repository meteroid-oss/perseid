# @@CLIENT_NAME@@ Java SDK

@@DESCRIPTION@@

```kotlin
implementation("@@JAVA_PACKAGE@@:@@NPM_PACKAGE@@:@@VERSION@@")
```

```xml
<dependency>
  <groupId>@@JAVA_PACKAGE@@</groupId>
  <artifactId>@@NPM_PACKAGE@@</artifactId>
  <version>@@VERSION@@</version>
</dependency>
```

Requires Java 11 or later.

## Client

```java
import @@JAVA_PACKAGE@@.@@CLIENT_NAME@@;
import @@JAVA_PACKAGE@@.@@CLIENT_NAME@@Options;

try (@@CLIENT_NAME@@ client = @@CLIENT_NAME@@.fromEnv()) {
    // client.<resource>().<method>(...)
}

@@CLIENT_NAME@@ client = new @@CLIENT_NAME@@(
        @@CLIENT_NAME@@Options.builder()
                .apiKey("your-api-key")
                .baseUrl("https://api.example.com")
                .timeout(Duration.ofSeconds(20))
                .maxRetries(3)
                .build());
```

Without an API key, the client reads `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the default base URL; explicit settings win. The client is `AutoCloseable`: closing it
releases its threads and connections. `httpClient(OkHttpClient)` shares your own OkHttp client
(left open on close), and `addInterceptor` wraps every attempt for logging, caching or signing.
Requests are logged through `System.Logger` (`@@JAVA_PACKAGE@@`) at `DEBUG`, or at `INFO` with
`debug(true)`.

Required path, query and header parameters are method arguments; optional ones go in an
immutable `...Options` built with `builder()`. Every method has overloads taking a
`RequestOptions` last, for the headers, timeout, retries or idempotency key of one call:
`RequestOptions.builder().timeout(Duration.ofSeconds(5)).maxRetries(0).build()`.

The examples below use an API with a `widgets` resource.

## Models

Models are immutable: `Widget.builder().id("w1").name("n").build()` checks required properties,
and `widget.toBuilder().name("m").build()` changes a copy. Required properties are read directly
(`widget.id()`), others as an `Optional`. For an optional property that accepts `null`, passing
`null` to the builder sends `null`, while leaving it unset leaves it out. Properties this SDK
version does not know are kept in `additionalProperties()` and sent back. Enums keep values added
to the API later (`isKnown()`, `known()` to `switch` on), and so do unions (`isUnrecognized()`);
a tagged union tells its variants apart with `isCircle()` and `asCircle()`.

## Async and raw responses

`client.async()` has the same methods returning `CompletableFuture`s, sharing the client's
connections and retries. `withRawResponse()`, on either client, returns `ApiResponse`s with the
status code and headers along with the body:

```java
ApiResponse<Widget> response = client.withRawResponse().widgets().retrieve("w1");
response.statusCode();
response.requestId();
response.body();
```

## Pagination

List operations have an `...Iter` twin iterating over every item, fetching pages on demand, and
giving the pages themselves:

```java
for (Widget widget : client.widgets().listIter()) { ... }

Page<Widget> page = client.widgets().listIter().firstPage();
page.items();
if (page.hasNextPage()) {
    page = page.nextPage();
}

client.async().widgets().listIter().forEach(widget -> ...);
```

## Streaming

Server-sent events come as an `EventStream`, to close after use. When the API describes the
events, the stream yields them decoded and ends at `[DONE]`, and `lastEvent()` gives the raw event
(name, id, data) of the last one:

```java
try (EventStream<CompletionChunk> chunks = client.completions().createStream(request)) {
    for (CompletionChunk chunk : chunks) { ... }
}
```

## Errors

Every exception the SDK throws is a `@@CLIENT_NAME@@Exception`:

- `ApiException` for an error response, with `statusCode()`, `headers()`, `body()`,
  `requestId()` and `error(Type.class)` parsing the body as the error the API declares. Common
  statuses have a subclass: `BadRequestException`, `AuthenticationException`,
  `PermissionDeniedException`, `NotFoundException`, `ConflictException`,
  `UnprocessableEntityException`, `RateLimitException` and `InternalServerException`.
- `ApiConnectionException` when no response came, and its subclass `ApiTimeoutException`.
- `InvalidDataException` when a response is not what the API describes, such as a required
  property it left out.

Connection errors, timeouts, 408, 429 and 5xx responses are retried with jittered backoff,
honoring `Retry-After` and `retry-after-ms` up to a minute (the backoff otherwise), when the method
is idempotent or the request carries an `Idempotency-Key` (POST requests get one automatically).

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
