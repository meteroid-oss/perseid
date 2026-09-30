# Auth, pagination, streaming and encoding

## Authentication

Clients read `components.securitySchemes` and honor the `security` of each operation, including
`security: []` for public endpoints. For every request they send the credentials of the first
alternative that is fully configured:

| Scheme | Configured with |
|---|---|
| `http` bearer, `oauth2`, `openIdConnect` | the constructor token, or a token provider called before each request |
| `http` basic | `basicAuth` / `basic_auth` / `BasicAuth` / `setBasicAuth` |
| `apiKey` in a header, query parameter or cookie | the constructor token, or per scheme in `apiKeys` / `api_keys` / `ApiKeys` |

```ts
new Acme("sk_live_...");                                          // unchanged
new Acme(null, { tokenProvider: () => oauth.accessToken() });     // OAuth2, refreshed by you
```

A spec without `securitySchemes` keeps sending `Authorization: Bearer <token>`. There is no
built-in OAuth2 token exchange yet: bring a token provider.

## Pagination

Mark list operations with `x-pagination`, or describe them once in `perseid.toml` to match every
operation with that query parameter and response shape:

```yaml
x-pagination:
  cursor: starting_after        # or `page: page`, or `offset: offset`
  item_cursor: id               # next cursor from the last item, or `next_cursor: <path>`
  has_more: has_more            # optional, as are `total_pages`, `total` and `first_page`
  items: data                   # the default
```

```toml
[pagination]                    # or [[pagination]] with `operations = [...]` to scope rules
page = "page"
total_pages = "pagination_meta.total_pages"
first_page = 0
```

`x-pagination: false` opts an operation out of `perseid.toml` rules. Paginated operations get an
iterator next to the list method:

```ts
for await (const customer of client.customers.listCustomersIter({ perPage: 100 })) { ... }
```

```python
for customer in client.customers.list_customers_iter(per_page=100): ...
async for customer in async_client.customers.list_customers_iter(): ...
```

```go
pager := client.Customers().ListCustomersIter(ctx, nil)
for pager.Next() { customer := pager.Current() }   // or range over pager.All() with Go 1.23+
```

Rust has `let mut customers = client.customers().list_customers_iter(None); customers.next().await`,
Java a `Paginator<Customer>` from `listCustomersIter()` that is `Iterable` and has `stream()`, C# an
`IAsyncEnumerable<Customer>` from `ListCustomersIterAsync()` for `await foreach`.

## Streaming

`text/event-stream` responses return an event stream (`for await`, `for`, `Next()`, `Iterable`,
`next().await`, `await foreach`), `multipart/form-data` bodies a typed `...Body` with `Upload` files,
and `application/octet-stream` bodies accept bytes or streams. Streamed uploads are not retried.
Bodies of any other media type (`image/png`, `text/plain`...) are sent as given, with their media
type, and list fields of multipart bodies as one part per item. A JSON response that may also be an
event stream gets a `..._stream` twin method.

## Query parameters and form bodies

Lists repeat their parameter (`?tag=a&tag=b`), or are comma-separated with `explode: false`.
Objects, lists of objects and untyped JSON values are sent the way Stripe-style APIs read them,
for query parameters and `application/x-www-form-urlencoded` bodies alike:

```
filter[status]=open&filter[amount][gte]=5     # objects, nested as deep as they go
items[0][price]=p1&items[0][quantity]=2       # lists of objects
filter[tags][]=a&filter[tags][]=b             # lists inside objects
expand[]=a&expand[]=b                         # lists with `style: deepObject`
```

Form body properties follow their `encoding` (`style`, `explode`). Paths with a query of their
own, such as `/responses?beta=true`, keep it. Header and path parameters are strings: unions of
scalars are accepted, and a list or `content` header is sent as the caller writes it.
