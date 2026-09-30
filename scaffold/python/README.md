# @@CLIENT_NAME@@ Python SDK

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
the request, while `None` sends `null`.
