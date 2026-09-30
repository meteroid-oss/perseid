# @@CLIENT_NAME@@ Python SDK

@@DESCRIPTION@@

```sh
pip install @@PACKAGE_NAME@@
```

```python
from @@PACKAGE_NAME@@ import @@CLIENT_NAME@@

with @@CLIENT_NAME@@("your-api-key") as client:
    ...
```

`@@CLIENT_NAME@@Async` offers the same resources for asyncio. Every method also takes
`extra_headers=` and `timeout=` for that request only.

Models are keyword-only dataclasses with `from_dict`/`to_dict`. An optional field that
accepts `null` defaults to `UNSET` (from `@@PACKAGE_NAME@@.models`): it is left out of
the request, while `None` sends `null`. A field holding a string or an object, such as an
expandable id, is typed `str | Model`. A union variant fills in its own tag: pass
`content=` alone and the discriminator follows.

## Errors

Every error derives from `@@CLIENT_NAME@@Error`. A non-2xx response raises an
`ApiException` subclass named after its status (`BadRequestError`, `AuthenticationError`,
`PermissionDeniedError`, `NotFoundError`, `ConflictError`, `UnprocessableEntityError`,
`RateLimitError`, `InternalServerError`, else `ApiStatusError`), whose `body` is the
error response decoded into its schema and `request_id` the id to quote to support.

```python
from @@PACKAGE_NAME@@ import NotFoundError

try:
    ...
except NotFoundError as error:
    print(error.status_code, error.request_id, error.body)
```

A request that gets no response raises `NetworkException`, after retries.
