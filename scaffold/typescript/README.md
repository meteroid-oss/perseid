# @@CLIENT_NAME@@ TypeScript SDK

@@DESCRIPTION@@

```sh
npm install @@NPM_PACKAGE@@
```

The package ships ESM and CommonJS builds and runs on Node.js 20+, Deno, Bun, browsers and edge
runtimes: it only needs `fetch`.

## Client

```ts
import { @@CLIENT_NAME@@ } from "@@NPM_PACKAGE@@";

const client = new @@CLIENT_NAME@@({ apiKey: "your-api-key" });
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
is serialized: `(widget as Widget & { new_field?: string }).new_field`.

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
  await client.someResource.retrieve("id");
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
const { data, response, requestId } = await client.someResource.retrieve("id").withResponse();
const raw: Response = await client.someResource.retrieve("id").asResponse(); // body unread
```

## Pagination

List methods return a `PagePromise`. Iterate it to get every item, pages being fetched on demand,
or await it for the first `Page`, with `items`, `body` (the response), `hasNextPage()`,
`getNextPage()` and `iterPages()`.

```ts
for await (const item of client.someResource.list({ limit: 100 })) {
  console.log(item);
}

let page = await client.someResource.list();
while (page.hasNextPage()) {
  page = await page.getNextPage();
}
```

## Streaming

Server-sent events come as a `Stream` to iterate with `for await`. When the API declares the
schema of the events, each one is decoded into its model, up to a `[DONE]` event, and
`stream.lastEvent` gives the raw event (`event`, `data`, `id`). Breaking out of the loop or
calling `stream.close()` closes the connection.

```ts
const stream = await client.someResource.createStream({ prompt: "Hi" });
for await (const chunk of stream) {
  process.stdout.write(chunk.delta);
}
```

## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with exponential
backoff, honouring `Retry-After` and `retry-after-ms`, when the request is idempotent or carries
an `Idempotency-Key` (POST requests get one automatically). Each attempt times out after
`timeout` milliseconds (`Infinity` waits forever).

```ts
const client = new @@CLIENT_NAME@@({ maxRetries: 5, timeout: 20_000 });
await client.someResource.retrieve("id", { maxRetries: 0, timeout: 5_000 });
```

`middleware` wraps every attempt, for logging, caching or custom headers, and `fetch` replaces
the `fetch` implementation.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
