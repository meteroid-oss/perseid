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
new Acme({ apiKey: "sk_live_..." });                              // unchanged
new Acme({ tokenProvider: () => oauth.accessToken() });           // OAuth2, refreshed by you
```

Without a token, clients read `ACME_API_KEY`, and `ACME_BASE_URL` overrides the base URL. The
prefix is the `name` in SCREAMING_SNAKE_CASE, or `env_prefix` under `[context]`.

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

`x-pagination: false` opts an operation out of `perseid.toml` rules. Paginated operations give
both every item, fetching pages on demand, and the pages themselves (items, the response body,
whether there is a next page and how to fetch it):

```ts
for await (const customer of client.customers.list({ perPage: 100 })) { ... }
const page = await client.customers.list();       // page.items, page.hasNextPage(), page.getNextPage()
```

```python
for customer in client.customers.list(per_page=100): ...
page = client.customers.list()                     # page.items, page.has_next_page(), page.get_next_page()
async for customer in async_client.customers.list(): ...
```

```go
for customer, err := range client.Customers().ListAutoPaging(ctx, nil).All() { ... }
page, err := client.Customers().List(ctx, nil) // page.Items, page.Body, page.HasNextPage(), page.NextPage(ctx)
```

```rust
let mut customers = client.customers().list_iter(None); // a Stream that owns its client
while let Some(customer) = customers.next().await { let customer = customer?; }
let page = client.customers().list_iter(None).first_page().await?; // items(), next_page()
```

```java
for (Customer customer : client.customers().listIter()) { ... }
Page<Customer> page = client.customers().listIter().firstPage(); // items(), hasNextPage(), nextPage()
```

```csharp
await foreach (var customer in client.Customers.ListAutoPagingAsync(new() { PerPage = 100 })) { ... }
var page = await client.Customers.ListAutoPagingAsync().GetFirstPageAsync(); // Items, GetNextPageAsync()
```

## Streaming

`text/event-stream` responses return an event stream (`for await`, `for`, `Next()`, `Iterable`,
`next().await`, `await foreach`), `multipart/form-data` bodies a typed `...Body` with `Upload` files,
and `application/octet-stream` bodies accept bytes or streams. Streamed uploads are not retried.
Bodies of any other media type (`image/png`, `text/plain`...) are sent as given, with their media
type, and list fields of multipart bodies as one part per item. A JSON response that may also be an
event stream gets a `..._stream` twin method, which sets the body's boolean `stream` property to
`true` when it has one.

When the `text/event-stream` content references a schema, as OpenAI's spec does, each event's
`data` decodes into that model and the stream ends at `data: [DONE]`. The raw event (`event`, `id`)
of the last item stays available as `lastEvent` / `last_event` / `Event()` / `LastEvent`.

```ts
const stream = await client.completions.createStream({ prompt: "hi" });
for await (const chunk of stream) process.stdout.write(chunk.delta);
```

## Raw responses

Every SDK can return the status, headers and request id of a successful call with its parsed body:

| Language | |
|---|---|
| TypeScript | `const { data, response, requestId } = await client.customers.retrieve(id).withResponse()` |
| Python | `raw = client.with_raw_response.customers.retrieve(id)`, then `raw.headers`, `raw.parse()` |
| Go | `client.Customers().Retrieve(ctx, id, acme.WithResponseInto(&resp))` |
| Rust | `client.customers().retrieve(id).with_response().await?`, then `.request_id()`, `.into_data()` |
| Java | `client.withRawResponse().customers().retrieve(id)`, an `ApiResponse<Customer>` |
| C# | `await client.Customers.WithRawResponse.RetrieveAsync(id)`, an `ApiResponse<Customer>` |

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
own, such as `/responses?beta=true`, keep it.

`pipeDelimited` and `spaceDelimited` lists without `explode` are sent as `ids=a|b|c` and
`ids=a b c`; with `explode` they repeat the parameter like `form`. A parameter with
`content: application/json` (query, path or header) is typed by its schema and sent as compact JSON,
percent-encoded where it needs to be.

Cookie parameters are typed like header ones and sent in one `Cookie` header, percent-encoded. A
parameter whose name another one of the operation already takes (`id` in the query and in a
header) gets a `_query`, `_header` or `_cookie` suffix in the SDK, its wire name untouched; path
variables the path uses without declaring them are strings.

Plain path parameters are strings: unions of scalars are accepted. Other path parameters follow
their `style` and `explode` as the OpenAPI table has it: `label` (`.a.b`), `matrix` (`;id=a,b`),
and lists and objects (`a,b,c`, `k,v,k2,v2`, or `k=v,k2=v2` exploded) are typed values. Header
lists are sent as the caller writes them. Operations using a construct perseid does not support are
skipped with a warning naming them.

## Bodies and responses

A request body of any JSON schema is accepted, including a bare list or scalar, a boolean schema
and a recursive alias (`Tree: array of Tree`, typed as plain JSON where an SDK cannot spell it).
Nullable items, map values and response bodies stay nullable: a `null` body decodes to the
language's empty value without an error. An operation declaring both a body and a bodiless `204`
(or any bodiless 2xx) returns an optional value, empty for the bodiless response.

Every operation is sent to the one base URL of the client. Operations or path items declaring their
own `servers` warn, naming the operation: put them in a spec of their own, or set the base URL when
creating the client.
