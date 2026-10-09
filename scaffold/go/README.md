{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set create = examples.create -%}
{% set download = examples.download -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(opts="") %}{% if call %}{{ docs.call(call, opts) }}{% else %}client.SomeResource().SomeMethod(ctx{{ ", " ~ opts if opts }}){% endif %}{% endmacro -%}
{% macro assign(ex, name) %}{% if ex and ex.result %}{{ name }}, err :={% else %}err :={% endif %}{% endmacro -%}
# @@CLIENT_NAME@@ Go SDK

@@DESCRIPTION@@

Requires Go 1.23 or later.

```sh
go get @@GO_MODULE@@
```

Every method of the API is listed in [api.md](api.md).

## Usage

```go
{{ docs.imports([call]) }}import @@PACKAGE_NAME@@ "@@GO_MODULE@@"

client := @@PACKAGE_NAME@@.New("your-api-key", {% if sdk.has_default_base_url %}nil{% else %}&@@PACKAGE_NAME@@.Options{ServerURL: "https://api.example.com"}{% endif %})

{{ assign(call, result) }} {{ call_of() }}
if err != nil {
	return err
}{% if call and call.result %}
fmt.Println({{ result }}){% endif %}
```

Every API area hangs off the client as an accessor method, and every method takes a
`context.Context` first. Required parameters are arguments; optional ones go in an options struct
whose fields are pointers (`@@PACKAGE_NAME@@.Ptr(v)`), `nil` to send none. Request bodies are
structs of the package{% if create %}:

```go
{{ assign(create, docs.var(create.result, "result")) }} {{ docs.call(create) }}
```
{% else %}.
{% endif %}
Models keep the properties this SDK version does not know in `ExtraFields`, and send them back.

### Environment

`New("", nil)` reads the token from `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the default server. Explicit arguments win:
`@@PACKAGE_NAME@@.New(token, &@@PACKAGE_NAME@@.Options{ServerURL: "https://..."})`. When the API
declares no server, `@@PACKAGE_NAME@@.DefaultServerURL` is empty and every call fails with a
`*RequestError` naming both settings until one of them is set.

### Errors

Every error is a `@@PACKAGE_NAME@@.SDKError`. A non-2xx response is an `*@@PACKAGE_NAME@@.APIError`,
whose `Body` is the error body decoded as the schema the operation declares (else plain JSON);
a timeout is a `*TimeoutError`, a failed connection a `*TransportError`, an undecodable response a
`*DecodeError`:

```go
{{ assign(call, "_") }} {{ call_of() }}
var apiErr *@@PACKAGE_NAME@@.APIError
switch {
case errors.Is(err, @@PACKAGE_NAME@@.ErrNotFound):
	// ...
case errors.As(err, &apiErr):
	log.Printf("status %d, request %s", apiErr.StatusCode, apiErr.RequestID())
}
```
{% if list %}
### Pagination

A list method returns its first page, a `*@@PACKAGE_NAME@@.{{ docs.page_type(list) }}`. The page embeds
the decoded `{{ docs.body_type(list) }}` response, so its fields read directly on the page, and holds its
`Items`; `HasNextPage()` and `NextPage(ctx)` step to the next page, nil after the last one. A
response field named like one of these members stays reachable through the embedded response,
`page.{{ docs.body_type(list) }}`:

```go
page, err := {{ docs.call(list) }}
for page != nil && err == nil {
	// {{ docs.page_field(list) }}, page.Items, page.HasNextPage()
	page, err = page.NextPage(ctx)
}
```

Its `...AutoPaging` twin returns an `*AutoPager[T]` over every item, fetching further pages on
demand:

```go
for {{ docs.var(list.item, "item") }}, err := range {{ docs.call(list, suffix="AutoPaging") }}.All() {
	if err != nil {
		return err
	}
	fmt.Println({{ docs.var(list.item, "item") }})
}
```

Without `range`, loop on the pager's `Next()`, read `Current()`, then check `Err()`.
{% endif %}
{%- if stream %}
### Streaming

Server-sent events come as a `*Stream[T]` of decoded events, which ends at `[DONE]`, or an
`*EventStream` of raw `SSEEvent`s:

```go
stream, err := {{ docs.call(stream) }}
if err != nil {
	return err
}
defer stream.Close()
for event, err := range stream.All() {
	if err != nil {
		return err
	}
	fmt.Println(event){% if stream.operation.event_schema_name %} // stream.Event() is the raw event{% endif %}
}
```
{% endif %}
{%- if download %}
### Downloads

Binary responses come as a `*BinaryResponse`, returned once the headers arrive: an
`io.ReadCloser` over the body as it streams in, with its `Header`. Close it, or read it whole:

```go
file, err := {{ docs.call(download) }}
if err != nil {
	return err
}
data, err := file.Bytes() // or io.Copy(dst, file), or file.WriteToFile(path), then Close
```

The timeout covers the wait for the headers, then each read, not the whole download.
{% endif %}
### Raw responses

`WithResponseInto` gives the `*http.Response` of a call, for its status and headers:

```go
var resp *http.Response
{{ assign(call, result) }} {{ call_of(sdk.package_name ~ ".WithResponseInto(&resp)") }}
log.Print(resp.Header.Get("X-Request-Id"))
```

### Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with jittered backoff,
honoring `Retry-After` and `retry-after-ms`, when the request is idempotent or carries an
`Idempotency-Key`, and 429 responses of every request. Each attempt times out after `DefaultTimeout`.
`Options` sets them for the client (`MaxRetries`, `Timeout`), and request options for one call:
`@@PACKAGE_NAME@@.WithMaxRetries(0)`, `@@PACKAGE_NAME@@.WithTimeout(time.Minute)`,
`@@PACKAGE_NAME@@.WithIdempotencyKey(key)`, `@@PACKAGE_NAME@@.WithHeader(name, value)`.
`@@PACKAGE_NAME@@.WithQuery(name, value)` and `@@PACKAGE_NAME@@.WithJSONSet(path, value)` send a
parameter or body property this SDK version does not know yet.
`Options.Logger` logs every attempt at debug level.

```go
{{ assign(call, result) }} {{ call_of(sdk.package_name ~ ".WithMaxRetries(0), " ~ sdk.package_name ~ ".WithTimeout(5*time.Second)") }}
```

- Source: @@REPOSITORY@@
- License: @@LICENSE@@

_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
