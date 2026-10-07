{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set create = examples.create -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(raw=false, request_options="") %}{% if call %}{{ docs.call(call, raw=raw, request_options=request_options) }}{% else %}client.{{ "withRawResponse()." if raw }}someResource().someMethod({{ request_options }}){% endif %}{% endmacro -%}
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

Requires Java 11 or later. Every method of the API is listed in [api.md](api.md).

## Usage

```java
import @@JAVA_PACKAGE@@.@@CLIENT_NAME@@;
{% if call %}{{ docs.uses(call) }}{% endif %}
try (@@CLIENT_NAME@@ client = @@CLIENT_NAME@@.fromEnv()) {
    {% if call and call.result %}var {{ result }} = {{ call_of() }};
    System.out.println({{ result }});{% else %}{{ call_of() }};{% endif %}
}
```

The client can also be configured in code:

```java
import @@JAVA_PACKAGE@@.@@CLIENT_NAME@@Options;

@@CLIENT_NAME@@ client = new @@CLIENT_NAME@@(
        @@CLIENT_NAME@@Options.builder()
                .apiKey("your-api-key")
                .baseUrl("https://api.example.com")
                .timeout(Duration.ofSeconds(20))
                .maxRetries(3)
                .build());
```

Without an API key, the client reads `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the server of the API, `@@CLIENT_NAME@@.DEFAULT_BASE_URL`; explicit settings win. An API
without a server has no default: the client then throws an `IllegalStateException` until one of
them sets the base URL. The client is `AutoCloseable`: closing it
releases its threads and connections. `httpClient(OkHttpClient)` shares your own OkHttp client
(left open on close), and `addInterceptor` wraps every attempt for logging, caching or signing.
Requests are logged through `System.Logger` (`@@JAVA_PACKAGE@@`) at `DEBUG`, or at `INFO` with
`debug(true)`.

Required path, query and header parameters are method arguments; optional ones go in an
immutable `...Options` built with `builder()`. Every method has overloads taking a
`RequestOptions` last, for the headers, timeout, retries or idempotency key of one call:

```java
{% if call and call.result %}var {{ result }} = {% endif %}{{ call_of(request_options="RequestOptions.builder().timeout(Duration.ofSeconds(5)).maxRetries(0).build()") }};
```

## Models

Models are immutable: `Model.builder()...build()` checks required properties, and
`model.toBuilder()...build()` changes a copy{% if create %}:

```java
{{ docs.uses(create) }}
{% if create.result %}var {{ docs.var(create.result, "result") }} = {% endif %}{{ docs.call(create) }};
```
{% else %}.
{% endif %}
Required properties are read directly (`model.id()`), others as an `Optional`. For an optional
property that accepts `null`, passing `null` to the builder sends `null`, while leaving it unset
leaves it out. Properties this SDK version does not know are kept in `additionalProperties()` and
sent back.

Enums keep values added to the API later: `isKnown()` tells them apart, `value()` is an enum to
`switch` on with `_UNKNOWN` for them, `known()` throws on them, and `asString()` is the raw value.
Unions keep unknown variants too (`isUnrecognized()`). A union tells its variants apart with
`isCircle()` and `asCircle()`, or with a visitor whose `visitUnknown` throws unless overridden:

```java
String description = shape.accept(new Shape.Visitor<String>() {
    @Override
    public String visitCircle(Circle circle) {
        return "circle of radius " + circle.radius();
    }

    @Override
    public String visitSquare(Square square) {
        return "square of side " + square.side();
    }
});
```

## Async and raw responses

`client.async()` has the same methods returning `CompletableFuture`s, sharing the client's
connections and retries. `withRawResponse()`, on either client, returns `ApiResponse`s with the
status code and headers along with the body:

```java
var response = {{ call_of(raw=true) }};
response.statusCode();
response.requestId();
response.body();
```
{% if list %}
## Pagination

List operations have an `...Iter` twin iterating over every item, fetching pages on demand, and
giving the pages themselves:

```java
for (var {{ docs.var(list.item, "item") }} : {{ docs.call(list, iter=true) }}) {
    System.out.println({{ docs.var(list.item, "item") }});
}

var page = {{ docs.call(list, iter=true) }}.firstPage();
page.items();
if (page.hasNextPage()) {
    page = page.nextPage();
}

client.async(){{ docs.call(list, iter=true)[6:] }}.forEach(System.out::println);
```
{% endif %}
{%- if stream %}
## Streaming

Server-sent events come as an `EventStream`, to close after use. When the API describes the
events, the stream yields them decoded and ends at `[DONE]`, and `lastEvent()` gives the raw event
(name, id, data) of the last one:

```java
{{ docs.uses(stream) }}
try (var events = {{ docs.call(stream) }}) {
    for (var event : events) {
        System.out.println(event);
    }
}
```
{% endif %}
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

```java
import @@JAVA_PACKAGE@@.exceptions.NotFoundException;

try {
    {{ call_of() }};
} catch (NotFoundException e) {
    System.out.println(e.statusCode() + " " + e.requestId());
}
```

Connection errors, timeouts, 408, 429 and 5xx responses are retried with jittered backoff,
honoring `Retry-After` and `retry-after-ms` up to a minute (the backoff otherwise), when the method
is idempotent or the request carries an `Idempotency-Key`.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@

_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
