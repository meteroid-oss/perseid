# @@CLIENT_NAME@@ Go SDK

@@DESCRIPTION@@

Requires Go 1.23 or later.

```sh
go get @@GO_MODULE@@
```

## Usage

```go
import @@PACKAGE_NAME@@ "@@GO_MODULE@@"

client := @@PACKAGE_NAME@@.New("your-api-key", nil)
```

Every API area hangs off the client as an accessor method, and every method takes a
`context.Context` first. Required parameters are arguments; optional ones go in an options struct
whose fields are pointers (`@@PACKAGE_NAME@@.Ptr(v)`), `nil` to send none. Models keep the
properties this SDK version does not know in `ExtraFields`, and send them back.

### Environment

`New("", nil)` reads the token from `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the default server. Explicit arguments win:
`@@PACKAGE_NAME@@.New(token, &@@PACKAGE_NAME@@.Options{ServerURL: "https://..."})`.

### Errors

Every error is a `@@PACKAGE_NAME@@.SDKError`. A non-2xx response is an `*@@PACKAGE_NAME@@.APIError`,
whose `Body` is the error body decoded as the schema the operation declares (else plain JSON);
a timeout is a `*TimeoutError`, a failed connection a `*TransportError`, an undecodable response a
`*DecodeError`:

```go
var apiErr *@@PACKAGE_NAME@@.APIError
switch {
case errors.Is(err, @@PACKAGE_NAME@@.ErrNotFound):
	// ...
case errors.As(err, &apiErr):
	log.Printf("status %d, request %s", apiErr.StatusCode, apiErr.RequestID())
}
```

### Pagination

List operations have a `...Iter` method iterating over every item, and a `...Page` method
returning one page:

```go
for item, err := range client.Things().ListIter(ctx, nil).All() {
	if err != nil {
		return err
	}
	// ...
}

page, err := client.Things().ListPage(ctx, nil)
for page != nil && err == nil {
	// page.Items, page.HasNextPage()
	page, err = page.NextPage(ctx)
}
```

### Streaming

Server-sent events come as a `*Stream[T]` of decoded events, which ends at `[DONE]`, or an
`*EventStream` of raw `SSEEvent`s:

```go
stream, err := client.Things().CreateStream(ctx, body)
if err != nil {
	return err
}
defer stream.Close()
for chunk, err := range stream.All() {
	// stream.Event() is the raw event of chunk
}
```

### Raw responses

`WithResponseInto` gives the `*http.Response` of a call, for its status and headers:

```go
var resp *http.Response
thing, err := client.Things().Retrieve(ctx, id, @@PACKAGE_NAME@@.WithResponseInto(&resp))
log.Print(resp.Header.Get("X-Request-Id"))
```

### Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with jittered backoff,
honoring `Retry-After` and `retry-after-ms`, when the request is idempotent or carries an
`Idempotency-Key` (POST requests get one). Each attempt times out after `DefaultTimeout`.
`Options` sets them for the client, and request options for one call:
`@@PACKAGE_NAME@@.WithMaxRetries(0)`, `@@PACKAGE_NAME@@.WithTimeout(time.Minute)`,
`@@PACKAGE_NAME@@.WithIdempotencyKey(key)`, `@@PACKAGE_NAME@@.WithHeader(name, value)`.
`Options.Logger` logs every attempt at debug level.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
