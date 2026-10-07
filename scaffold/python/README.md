{% import "docs.jinja" as docs -%}
{% set call = examples.call -%}
{% set list = examples.list -%}
{% set stream = examples.stream -%}
{% set result = docs.var(call.result, "result") if call else "result" -%}
{% macro call_of(kwargs="", raw=false) %}{% if call %}{{ docs.call(call, kwargs, raw) }}{% else %}client.{{ "with_raw_response." if raw }}some_resource.some_method({{ kwargs }}){% endif %}{% endmacro -%}
{% set with_body = examples.create -%}
# @@CLIENT_NAME@@ Python SDK

@@DESCRIPTION@@

## Installation

```sh
pip install @@PACKAGE_NAME@@
```

Python 3.10 or newer. The SDK depends on `httpx` only and is fully typed. Every method of the API
is listed in [api.md](api.md).

## Usage

```python
{{ docs.imports([call]) }}from @@PACKAGE_NAME@@ import @@CLIENT_NAME@@

client = @@CLIENT_NAME@@(api_key="your-api-key"{% if not sdk.has_default_base_url %}, base_url="https://api.example.com"{% endif %})

{% if call and call.result %}{{ result }} = {{ call_of() }}
print({{ result }}){% else %}{{ call_of() }}{% endif %}
```

Without `api_key`, the client reads `@@ENV_PREFIX@@_API_KEY`, and `@@ENV_PREFIX@@_BASE_URL`
overrides the default base URL (required, or `base_url=`, when the API has none). Every argument
is a keyword argument. The constructor also takes `base_url`, `timeout`,
`max_retries`, `default_headers`, `http_client` (an `httpx.Client` of yours), `middleware`, and
the credentials the API accepts (`token_provider`, `basic_auth`, `api_keys`). Close the client,
or use it as a context manager, to release its connections.

`Async@@CLIENT_NAME@@` has the same resources for asyncio:

```python
{{ docs.imports([call]) }}from @@PACKAGE_NAME@@ import Async@@CLIENT_NAME@@

async with Async@@CLIENT_NAME@@() as client:
    {% if call and call.result %}{{ result }} = await {{ call_of() }}{% else %}await {{ call_of() }}{% endif %}
```

## Requests

The fields of a JSON or form request body are keyword arguments, next to the query and header
parameters; path parameters come first{% if with_body %}:

```python
{{ docs.call(with_body) }}
```
{% else %}.
{% endif %}
An optional argument left out is not sent; `None` sends `null` where the API accepts it. An enum
argument takes the enum or its value as a string. Other bodies (lists, unions, files) are one
`body` argument.

Every method also takes, for that request only, `extra_headers=`, `extra_query=`, `extra_body=`
(merged into the body), `timeout=` and `max_retries=`. These headers, like `default_headers`, win
over the client's credentials.

## Models

Models are keyword-only dataclasses with `from_dict`/`to_dict`. In models requests send, an
optional field that accepts `null` defaults to `UNSET` (from `@@PACKAGE_NAME@@.models`): it is
left out of the request, while `None` sends `null`; in response models it is simply `None` when
absent. A field holding a string or an object, such as an expandable id, is typed `str | Model`.
A discriminated union is the union of its variant models (`Circle | Square | UnknownVariant`),
decoded into the variant the tag names; when its variants share fields, it is a model holding the
discriminator and the variant, and passing `content=` alone fills in the tag.

Properties the API added after this SDK was generated are kept in `extra_fields` (and read
as attributes at runtime), and sent back when the model is serialized. A property named
after a model member, such as `extra_fields` or `to_dict`, gets a trailing `_`.
{% if list %}
## Pagination

List methods return their first page. Iterating it walks every item of every page,
fetching the next ones on demand:

```python
for {{ docs.var(list.item, "item") }} in {{ docs.call(list) }}:
    print({{ docs.var(list.item, "item") }})

page = {{ docs.call(list) }}
page.items  # this page's items
page.body  # the decoded response, with its other properties
if page.has_next_page():
    page = page.get_next_page()
for page in {{ docs.call(list) }}.iter_pages():
    print(len(page.items))
```

With the async client, `async for item in {{ docs.call(list) }}` walks the items, and
`page = await {{ docs.call(list) }}` returns the first page.
{% endif %}
{%- if stream %}
## Streaming

Server-sent events are iterated as they arrive. When the API documents the JSON of each
event, the stream yields models, until a `[DONE]` event; `stream.last_event` is the raw
event (`event`, `data`, `id`) of the latest one:

```python
with {{ docs.call(stream) }} as stream:
    for event in stream:
        print(event)
```

Other streams yield `SseEvent`s.
{% endif %}
Multipart bodies take `Upload(content, filename, content_type)` files, and binary bodies bytes or
file objects.

## Raw responses

Prefix a call with `with_raw_response` for the HTTP response next to the decoded result:

```python
response = {{ call_of(raw=true) }}
print(response.status_code, response.headers, response.request_id)
{{ result }} = response.parse()
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
    {{ call_of() }}
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
and requests with an `Idempotency-Key` header. Requests time out after
@@TIMEOUT@@ seconds.

```python
client = @@CLIENT_NAME@@({% if not sdk.has_default_base_url %}base_url="https://api.example.com", {% endif %}max_retries=5, timeout=20.0)
client.with_options(max_retries=0).{{ call_of()[7:] }}
{{ call_of("timeout=5.0, max_retries=0") }}  # for one call
```

## Middleware

`middleware=[...]` wraps every HTTP attempt: a callable receiving the `httpx.Request` and
`next`, returning an `httpx.Response`, to cache, log or sign requests.

_Generated by [perseid](https://github.com/meteroid-oss/perseid)._
