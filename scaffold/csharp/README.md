{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set create = examples.create -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(raw=false, request_options="") %}{% if call %}{{ docs.call(call, raw=raw, request_options=request_options) }}{% else %}client.SomeResource{{ ".WithRawResponse" if raw }}.SomeMethodAsync({{ "requestOptions: " ~ request_options if request_options }}){% endif %}{% endmacro -%}
{% macro models_using(ex) %}{% if ex and ex.body %}using @@PACKAGE_NAME@@.Models;
{% endif %}{% endmacro -%}
# @@CLIENT_NAME@@ .NET SDK

@@DESCRIPTION@@

## Install

```sh
dotnet add package @@PACKAGE_NAME@@
```

Targets .NET 8, trimming and native AOT safe. Every method of the API is listed in
[api.md](api.md).

## Usage

```csharp
using @@PACKAGE_NAME@@;
{{ models_using(call) }}
using var client = new @@CLIENT_NAME@@Client("your-api-key"{% if not sdk.has_default_base_url %}, new @@CLIENT_NAME@@ClientOptions { BaseUrl = "https://api.example.com" }{% endif %});

{% if call and call.result %}var {{ result }} = await {{ call_of() }};
Console.WriteLine({{ result }});{% else %}await {{ call_of() }};{% endif %}
```

`new @@CLIENT_NAME@@Client()` reads the key from `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the base URL; what you pass, in the constructor or in `@@CLIENT_NAME@@ClientOptions`, wins.
When the API declares no default base URL, set `BaseUrl` or `@@ENV_PREFIX@@_BASE_URL`: the constructor
throws a `@@CLIENT_NAME@@Exception` otherwise.
Create one client and reuse it: it is thread-safe and pools connections. To send requests through
your own `HttpClient`, pass it first: `new @@CLIENT_NAME@@Client(httpClient, "your-api-key")`.

Resources hang off the client as properties. Every method is async and takes an optional
`RequestOptions` (headers, timeout, retries, idempotency key) and a `CancellationToken`. Models
are records with `init` properties, compared by value{% if create %}:

```csharp
{{ models_using(create) }}
{% if create.result %}var {{ docs.var(create.result, "result") }} = {% endif %}await {{ docs.call(create) }};
```
{% else %}.
{% endif %}
Properties this SDK version does not know are kept in `AdditionalProperties` and sent back.
Unions are abstract records to match on, and enums keep unknown values too (`IsKnown`); `switch`
on `status.Value` with the `Status.Values` constants.

## Errors

Everything the SDK throws derives from `@@CLIENT_NAME@@Exception`:

```csharp
try
{
    await {{ call_of() }};
}
catch (NotFoundException e)
{
    Console.WriteLine($"{e.StatusCode} {e.RequestId}: {e.Error}");
}
catch (ApiTimeoutException) { /* no response in time, after the retries */ }
catch (ApiConnectionException) { /* the API could not be reached */ }
```

Error responses are `ApiException`s, as a subclass per status (`BadRequestException`,
`UnauthorizedException`, `NotFoundException`, `RateLimitException`, `ServerErrorException`...),
with the raw `Body`, the `Headers`, and `Error`: the body parsed as the error model the operation
declares for the status, else a `JsonElement`. `GetError<T>()` parses it as any model. A
successful response the SDK cannot read throws an `ApiDecodeException`.

## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with jittered backoff
(0.5s, then 1s), honoring `Retry-After` and `retry-after-ms` up to a minute (the backoff
otherwise), when the request is idempotent or carries an `Idempotency-Key` (POST requests get one). Each attempt times out after @@TIMEOUT@@ seconds.

```csharp
var client = new @@CLIENT_NAME@@Client(options: new() { {% if not sdk.has_default_base_url %}BaseUrl = "https://api.example.com", {% endif %}MaxRetries = 5, Timeout = TimeSpan.FromSeconds(20) });
await {{ call_of(request_options="new RequestOptions { MaxRetries = 0, Timeout = TimeSpan.FromSeconds(5) }") }};
```
{% if list %}
## Pagination

`…AutoPagingAsync` methods fetch the pages as you go, item by item or, with `AsPagesAsync()`, page by
page:

```csharp
await foreach (var {{ docs.var(list.item, "item") }} in {{ docs.call(list, auto_paging=true) }})
{
    Console.WriteLine({{ docs.var(list.item, "item") }});
}

var page = await {{ docs.call(list, auto_paging=true) }}.GetFirstPageAsync();
while (true)
{
    Console.WriteLine(page.Items.Count);
    if (!page.HasNextPage) break;
    page = await page.GetNextPageAsync();
}
```
{% endif %}
{%- if stream %}
## Streaming

Event streams are enumerated once, with `await foreach`; those of typed events yield models and
stop at `[DONE]`, with the raw event in `LastEvent`:

```csharp
{{ models_using(stream) -}}
await using var stream = await {{ docs.call(stream) }};
await foreach (var item in stream)
{
    Console.WriteLine(item);
}
```
{% endif %}
## Raw responses

`WithRawResponse` returns the status and headers with the decoded body:

```csharp
var response = await {{ call_of(raw=true) }};
Console.WriteLine($"{response.StatusCode} {response.RequestId}");
```

## Tests

`I@@CLIENT_NAME@@Client` and an interface per resource let you substitute a fake. Each call is an
`Activity` of the `@@PACKAGE_NAME@@` `ActivitySource`, for OpenTelemetry.
{% if sdk.dependency_injection %}
## Dependency injection

`Add@@CLIENT_NAME@@Client` registers an `IHttpClientFactory` typed client:

```csharp
builder.Services.Add@@CLIENT_NAME@@Client(options => options.Token = builder.Configuration["@@CLIENT_NAME@@:ApiKey"]);
```
{% endif %}
_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
