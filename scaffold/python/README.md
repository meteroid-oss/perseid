# @@CLIENT_NAME@@ Python SDK

@@DESCRIPTION@@

## Installation

```sh
pip install @@PACKAGE_NAME@@
```

Python 3.10 or newer. The SDK depends on `httpx` only and is fully typed.

## Usage

```python
from @@PACKAGE_NAME@@ import @@CLIENT_NAME@@

client = @@CLIENT_NAME@@(api_key="your-api-key")
```

Without `api_key`, the client reads `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the default base URL. The constructor also takes `base_url`, `timeout`,
`max_retries`, `default_headers`, `http_client` (an `httpx.Client` of yours), `middleware`, and
the credentials the API accepts (`token_provider`, `basic_auth`, `api_keys`). Close the client,
or use it as a context manager, to release its connections.

`Async@@CLIENT_NAME@@` has the same resources for asyncio:

```python
from @@PACKAGE_NAME@@ import Async@@CLIENT_NAME@@

async with Async@@CLIENT_NAME@@() as client:
    ...
```

Every method also takes `extra_headers=` and `timeout=` for that request only; these
headers, like `default_headers`, win over the client's credentials.

## Models

Models are keyword-only dataclasses with `from_dict`/`to_dict`. An optional field that
accepts `null` defaults to `UNSET` (from `@@PACKAGE_NAME@@.models`): it is left out of
the request, while `None` sends `null`. A field holding a string or an object, such as an
expandable id, is typed `str | Model`. A union variant fills in its own tag: pass
`content=` alone and the discriminator follows.

Properties the API added after this SDK was generated are kept in `extra_fields` (and read
as attributes at runtime), and sent back when the model is serialized. A property named
after a model member, such as `extra_fields` or `to_dict`, gets a trailing `_`.

## Pagination

List methods return their first page. Iterating it walks every item of every page,
fetching the next ones on demand:

```python
for item in client.items.list():
    print(item.id)

page = client.items.list()
page.items  # this page's items
page.body  # the decoded response, with its other properties
if page.has_next_page():
    page = page.get_next_page()
for page in client.items.list().iter_pages():
    ...
```

With the async client, `async for item in client.items.list()` walks the items, and
`page = await client.items.list()` returns the first page.

## Streaming

Server-sent events are iterated as they arrive. When the API documents the JSON of each
event, the stream yields models, until a `[DONE]` event; `stream.last_event` is the raw
event (`event`, `data`, `id`) of the latest one:

```python
with client.items.create_stream(request) as stream:
    for chunk in stream:
        print(chunk)
```

Other streams yield `SseEvent`s. Multipart bodies take `Upload(content, filename,
content_type)` files, and binary bodies bytes or file objects.

## Raw responses

Prefix a call with `with_raw_response` for the HTTP response next to the decoded result:

```python
response = client.with_raw_response.items.retrieve("id")
print(response.status_code, response.headers, response.request_id)
item = response.parse()
```

## Errors

Every error derives from `@@CLIENT_NAME@@Error`. A non-2xx response raises an `APIStatusError`
subclass named after its status (`BadRequestError`, `AuthenticationError`,
`PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`,
`RateLimitError`, `InternalServerError`), whose `body` is the error response decoded into its
schema (its JSON when the API declares none) and `request_id` the id to quote to support.

```python
from @@PACKAGE_NAME@@ import APIConnectionError, APITimeoutError, NotFoundError

try:
    ...
except NotFoundError as error:
    print(error.status_code, error.request_id, error.body)
except APITimeoutError:
    ...  # the request timed out, after retries
except APIConnectionError:
    ...  # no response, after retries
```

A successful response that does not decode raises `APIResponseValidationError`.

## Retries and timeouts

Connection errors, timeouts, 408, 429 and 5xx responses are retried twice with exponential
backoff, honoring `Retry-After`, when replaying the request is safe: for idempotent methods
and requests with an `Idempotency-Key` header, which every POST gets. Requests time out after
@@TIMEOUT@@ seconds.

```python
client = @@CLIENT_NAME@@(max_retries=5, timeout=20.0)
client.with_options(max_retries=0).items.list()  # for one call
client.items.list(timeout=5.0)
```

## Middleware

`middleware=[...]` wraps every HTTP attempt: a callable receiving the `httpx.Request` and
`next`, returning an `httpx.Response`, to cache, log or sign requests.
