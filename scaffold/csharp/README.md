# @@CLIENT_NAME@@ .NET SDK

@@DESCRIPTION@@

```sh
dotnet add package @@PACKAGE_NAME@@
```

```csharp
using @@PACKAGE_NAME@@;

using var client = new @@CLIENT_NAME@@Client("your-api-key");
```

Every method is async and takes an optional `RequestOptions` (headers, timeout, retries,
idempotency key) and a `CancellationToken`. List methods have an `…IterAsync` twin to
`await foreach` over every item across pages.

Errors are `ApiException`s with `StatusCode`, `Body` and `Headers`, thrown as a subclass per
status (`NotFoundException`, `RateLimitException`, …). `GetError<T>()` parses the body as an
error model (`GetError()` for the API's usual one) and `GetRequestId()` reads the request id.

Models are records. Unions are abstract records to match on, whose nested variants keep values
added to the API after this SDK version as `Unrecognized`. Enums keep unknown values too
(`IsKnown`).
