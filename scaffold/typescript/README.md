{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(request_options="") %}{% if call %}{{ docs.call(call, request_options) }}{% else %}client.someResource.someMethod({{ request_options }}){% endif %}{% endmacro -%}
# @@CLIENT_NAME@@ TypeScript SDK

@@DESCRIPTION@@

```sh
npm install @@NPM_PACKAGE@@
```

The package ships ESM and CommonJS builds and runs on Node.js 20+, Deno, Bun, browsers and edge
runtimes: it only needs `fetch`. Every method of the API is listed in [api.md](api.md).

## Usage

```ts
import { @@CLIENT_NAME@@ } from "@@NPM_PACKAGE@@";

const client = new @@CLIENT_NAME@@({ apiKey: "your-api-key"{% if not sdk.has_default_base_url %}, baseURL: "https://api.example.com"{% endif %} });

{% if call and call.result %}const {{ result }} = await {{ call_of() }};
console.log({{ result }});{% else %}await {{ call_of() }};{% endif %}
```

Without `apiKey`, the client reads the `@@ENV_PREFIX@@_API_KEY` environment variable, and
without `baseURL` the `@@ENV_PREFIX@@_BASE_URL` one, then the API's default server URL. When the
API declares no server, one of them is required. The other options are `timeout` (milliseconds),
`maxRetries`, `defaultHeaders`, `defaultQuery`, `fetch`, `middleware` and `debug`.

Every API area is a property of the client, and every method takes a last `RequestOptions`
argument: `{ signal, headers, query, timeout, maxRetries, idempotencyKey }`.

## Models

Models are plain objects with camelCase properties, converted from and to their JSON form by
`XSerializer.parse(json)` and `XSerializer.serialize(value)`. Properties the API sends that this
version of the SDK does not know are kept under their JSON names, and sent back when the object
is serialized: `(value as typeof value & { new_field?: string }).new_field`.

## Errors

Everything the SDK throws is a `@@CLIENT_NAME@@Error`:

| Error | When |
|---|---|
| `APIError` | A non-2xx response: `status`, `headers`, `requestId`, `body` (the raw text) and `error` (the body parsed with its declared schema, else as JSON) |
| `BadRequestError`, `AuthenticationError`, `PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`, `RateLimitError`, `InternalServerError` | `APIError`s of statuses 400, 401, 403, 404, 409, 422, 429 and 5xx |
| `APIConnectionError` | No response: DNS, TLS or network failure, with the transport error as `cause` |
| `APIConnectionTimeoutError` | No response within the timeout (an `APIConnectionError`) |
| `APIUserAbortError` | The call's `signal` aborted |
| `APIDecodeError` | A successful response that is not valid JSON or not the expected event stream |

```ts
import { NotFoundError } from "@@NPM_PACKAGE@@";

try {
  await {{ call_of() }};
} catch (error) {
  if (error instanceof NotFoundError) {
    console.log(error.status, error.requestId, error.error);
  }
}
```

## Raw responses

Every method returns an `APIPromise`: await it for the parsed body, or ask for the HTTP response
too.

```ts
const { data, response, requestId } = await {{ call_of() }}.withResponse();
const raw: Response = await {{ call_of() }}.asResponse(); // body unread
```
{% if list %}
## Pagination

List methods return a `PagePromise`. Iterate it to get every item, pages being fetched on demand,
or await it for the first `Page`, with `items`, `body` (the response), `hasNextPage()`,
`getNextPage()` and `iterPages()`.

```ts
for await (const {{ docs.var(list.item, "item") }} of {{ docs.call(list) }}) {
  console.log({{ docs.var(list.item, "item") }});
}

let page = await {{ docs.call(list) }};
while (page.hasNextPage()) {
  page = await page.getNextPage();
}
```
{% endif %}
{%- if stream %}
## Streaming

Server-sent events come as a `Stream` to iterate with `for await`. When the API declares the
schema of the events, each one is decoded into its model, up to a `[DONE]` event, and
`stream.lastEvent` gives the raw event (`event`, `data`, `id`). Breaking out of the loop or
calling `stream.close()` closes the connection.

```ts
const stream = await {{ docs.call(stream) }};
for await (const event of stream) {
  console.log(event);
}
```
{% endif %}
## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with exponential
backoff, honouring `Retry-After` and `retry-after-ms`, when the request is idempotent or carries
an `Idempotency-Key` (POST requests get one automatically). Each attempt times out after
`timeout` milliseconds (`Infinity` waits forever).

```ts
const client = new @@CLIENT_NAME@@({ {% if not sdk.has_default_base_url %}baseURL: "https://api.example.com", {% endif %}maxRetries: 5, timeout: 20_000 });
await {{ call_of("{ maxRetries: 0, timeout: 5_000 }") }};
```

`middleware` wraps every attempt, for logging, caching or custom headers, and `fetch` replaces
the `fetch` implementation.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@

_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
