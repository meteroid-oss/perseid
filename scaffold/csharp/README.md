# @@CLIENT_NAME@@ .NET SDK

@@DESCRIPTION@@

## Install

```sh
dotnet add package @@PACKAGE_NAME@@
```

Targets .NET 8, trimming and native AOT safe.

## Create a client

```csharp
using @@PACKAGE_NAME@@;

using var client = new @@CLIENT_NAME@@Client("your-api-key");
```

`new @@CLIENT_NAME@@Client()` reads the key from `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the base URL; what you pass, in the constructor or in `@@CLIENT_NAME@@ClientOptions`, wins.
When the API declares no default base URL, set `BaseUrl` or `@@ENV_PREFIX@@_BASE_URL`: the constructor
throws a `@@CLIENT_NAME@@Exception` otherwise.
Create one client and reuse it: it is thread-safe and pools connections. To send requests through
your own `HttpClient`, pass it first: `new @@CLIENT_NAME@@Client(httpClient, "your-api-key")`.

Resources hang off the client as properties (the examples below use a `Pets` resource). Every
method is async and takes an optional `RequestOptions` (headers, timeout, retries,
idempotency key) and a `CancellationToken`. Models are records with `init` properties, compared by
value; properties this SDK version does not know are kept in `AdditionalProperties` and sent back.
Unions are abstract records to match on, and enums keep unknown values too (`IsKnown`); `switch`
on `status.Value` with the `Status.Values` constants.

## Errors

Everything the SDK throws derives from `@@CLIENT_NAME@@Exception`:

```csharp
try
{
    await client.Pets.RetrieveAsync("missing");
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
var client = new @@CLIENT_NAME@@Client(options: new() { MaxRetries = 5, Timeout = TimeSpan.FromSeconds(20) });
await client.Pets.RetrieveAsync("1", new RequestOptions { MaxRetries = 0, Timeout = TimeSpan.FromSeconds(5) });
```

## Pagination

`…AutoPagingAsync` methods fetch the pages as you go, item by item or, with `AsPagesAsync()`, page by
page:

```csharp
await foreach (var pet in client.Pets.ListAutoPagingAsync())
{
    Console.WriteLine(pet.Name);
}

var page = await client.Pets.ListAutoPagingAsync().GetFirstPageAsync();
while (true)
{
    Console.WriteLine(page.Items.Count);
    if (!page.HasNextPage) break;
    page = await page.GetNextPageAsync();
}
```

## Streaming

Event streams are enumerated once, with `await foreach`; those of typed events yield models and
stop at `[DONE]`, with the raw event in `LastEvent`:

```csharp
await using var stream = await client.Completions.CreateStreamAsync(new() { Prompt = "Hi" });
await foreach (var chunk in stream)
{
    Console.Write(chunk.Delta);
}
```

## Raw responses

`WithRawResponse` returns the status and headers with the decoded body:

```csharp
var response = await client.Pets.WithRawResponse.RetrieveAsync("1");
Console.WriteLine($"{response.StatusCode} {response.RequestId} {response.Value.Name}");
```

## Tests and dependency injection

`I@@CLIENT_NAME@@Client` and an interface per resource let you substitute a fake. Each call is an
`Activity` of the `@@PACKAGE_NAME@@` `ActivitySource`, for OpenTelemetry. With
`dependency_injection = true` under `[csharp.context]` in `perseid.toml`, the package registers an
`IHttpClientFactory` typed client:

```csharp
builder.Services.Add@@CLIENT_NAME@@Client(options => options.Token = builder.Configuration["@@CLIENT_NAME@@:ApiKey"]);
```
