# Auth, pagination, streaming, raw responses and encoding

What the SDKs do on the wire, from what the spec declares. See [languages](languages.md) for
each SDK's syntax.

## Authentication

Clients read `components.securitySchemes` and honor each operation's `security`, including
`security: []` for public endpoints. Each request sends the credentials of the first alternative
that is fully configured.

| Scheme | Configured with |
|---|---|
| `http` bearer, `openIdConnect` | The constructor token, or a token provider called before each request |
| `oauth2` | The same, or a client id and secret for a `clientCredentials` flow |
| `http` basic | `basicAuth` / `basic_auth` / `BasicAuth` |
| `apiKey` in a header, query parameter or cookie | The constructor token, or one key per scheme in `apiKeys` / `api_keys` / `APIKeys` |

```ts
new Acme({ apiKey: "sk_live_..." });
new Acme({ tokenProvider: () => oauth.accessToken() });
```

- Without a token, clients read `ACME_API_KEY`. `ACME_BASE_URL` overrides the base URL.
- The prefix is the `name` in SCREAMING_SNAKE_CASE, or `env_prefix` under `[context]`.
- A spec without `securitySchemes` sends `Authorization: Bearer <token>`.
- A requirement naming a scheme `securitySchemes` does not declare fails generation, unless another
  of its alternatives can be sent: that alternative is then dropped with a warning. Operations
  left out with `exclude` or `x-internal` are not checked.

### OAuth2 client credentials

```ts
new Acme({ clientId: "...", clientSecret: "..." }); // or ACME_CLIENT_ID and ACME_CLIENT_SECRET
```

- The client posts `grant_type=client_credentials` to the `tokenUrl` on first use, asking for the
  scopes the operations require. A relative `tokenUrl` resolves against the base URL.
- Credentials go in an HTTP basic header (RFC 6749), or as form fields with the client auth
  option set to `body`.
- The token is cached until a minute before `expires_in`, or half its life when shorter.
  Concurrent requests share one token request.
- The token request goes through the client's middleware, timeout and retries.
- A `401` replaces the token once and sends the request again.
- A token provider or a constructor token takes precedence over client credentials.

## Pagination

Mark list operations with `x-pagination`:

```yaml
x-pagination:
  cursor: starting_after        # or `page: page`, or `offset: offset`
  item_cursor: id               # next cursor from the last item, or `next_cursor: <path>`
  has_more: has_more            # optional, as are `total_pages`, `total` and `first_page`
  items: data                   # `data` by default, else the response's only array of objects
```

Or describe them once in `perseid.toml`, to match every operation with that query parameter and
response shape:

```toml
[[pagination]]                  # `operations = [...]` scopes a rule
page = "page"
total_pages = "pagination_meta.total_pages"
first_page = 0

[[pagination]]
offset = "offset"
has_more = "has_more"           # each used where the response has it
total = "total"
```

A rule without `operations` uses `has_more`, `total_pages` and `total` only where the response
has them; without any, paging stops at the first empty page.

`x-pagination: false` opts an operation out of the `perseid.toml` rules.

A paginated operation has one method. It gives the first page, and iterating it gives every
item, fetching the next pages on demand; Go iterates with a `...AutoPaging` twin. A page is the response body, with the paging members:

- The response's properties are read on the page, typed: `page.total`, `page.pagination_meta`.
  They are those of each API's schema.
- The paging members are the same for every API: `items`, the items of the page, whether there
  is a next page, the next page, and the pages from this one.
- `body` gives the response as decoded. A property named like a paging member stays there, and
  generating warns about it.

```ts
for await (const customer of client.customers.list({ perPage: 100 })) { ... }
const page = await client.customers.list();       // page.total, page.items, page.hasNextPage(), page.getNextPage()
```

```python
for customer in client.customers.list(per_page=100): ...
page = client.customers.list()                     # page.total, page.items, page.has_next_page(), page.get_next_page()
```

```go
for customer, err := range client.Customers().ListAutoPaging(ctx, nil).All() { ... }
page, err := client.Customers().List(ctx, nil) // page.Total, page.Items, page.HasNextPage(), page.NextPage(ctx)
```

```rust
let mut customers = client.customers().list(None).items();
while let Some(customer) = customers.next().await { let customer = customer?; }
let page = client.customers().list(None).await?; // page.total, page.items(), page.next_page()
```

```java
for (Customer customer : client.customers().list()) { ... }
CustomersListPage page = client.customers().list(); // page.total(), items(), hasNextPage(), nextPage()
```

```csharp
await foreach (var customer in client.Customers.ListAsync(new() { PerPage = 100 })) { ... }
var page = await client.Customers.ListAsync(); // page.Total, Items, HasNextPage, GetNextPageAsync()
```

## Streaming and uploads

```ts
const stream = await client.completions.createStream({ prompt: "hi" });
for await (const chunk of stream) process.stdout.write(chunk.delta);
```

| Media type | SDK |
|---|---|
| `text/event-stream` response | An event stream: `for await`, `for`, `range`, `Stream`, `Iterable`, `await foreach` |
| `multipart/form-data` body | A typed `...Body`, with `Upload` files. List fields are one part per item |
| `application/octet-stream` body | Bytes or a stream |
| Any other media type (`image/png`, `text/plain`...) | Sent as given, with its media type |

- When the event stream content references a schema, as OpenAI's spec does, each event's `data`
  decodes into that model and the stream ends at `data: [DONE]`.
- The raw event (`event`, `id`) of the last item is `lastEvent` / `last_event` / `Event()` /
  `LastEvent`.
- A JSON response that may also be an event stream gets a `..._stream` twin method. It sets the
  body's boolean `stream` property to `true` when there is one.
- Streamed uploads are not retried.

## Raw responses

Every SDK can return the status, headers and request id of a successful call with its body.

| Language | |
|---|---|
| TypeScript | `const { data, response, requestId } = await client.customers.retrieve(id).withResponse()` |
| Python | `raw = client.with_raw_response.customers.retrieve(id)`, then `raw.headers`, `raw.parse()` |
| Go | `client.Customers().Retrieve(ctx, id, acme.WithResponseInto(&resp))` |
| Rust | `client.customers().retrieve(id).with_response().await?`, then `.request_id()`, `.into_data()` |
| Java | `client.withRawResponse().customers().retrieve(id)`, an `ApiResponse<Customer>` |
| C# | `await client.Customers.WithRawResponse.RetrieveAsync(id)`, an `ApiResponse<Customer>` |

## Query parameters and form bodies

Objects, lists of objects and untyped JSON values are sent the way Stripe-style APIs read them,
in query parameters and `application/x-www-form-urlencoded` bodies alike:

```
filter[status]=open&filter[amount][gte]=5     # objects, nested as deep as they go
items[0][price]=p1&items[0][quantity]=2       # lists of objects
filter[tags][]=a&filter[tags][]=b             # lists inside objects
expand[]=a&expand[]=b                         # lists with `style: deepObject`
```

| Parameter | Sent as |
|---|---|
| List | Repeated (`?tag=a&tag=b`), comma-separated with `explode: false` |
| `pipeDelimited`, `spaceDelimited` list | `ids=a\|b\|c`, `ids=a b c`; repeated with `explode` |
| `content: application/json` (query, path, header) | Typed by its schema, sent as compact JSON, percent-encoded where needed |
| Header | Typed by its schema like a query parameter: numbers, booleans, dates, enums, lists (comma-separated) |
| Cookie | Typed like a header, sent in one `Cookie` header, percent-encoded |
| Form body property | Follows its `encoding` (`style`, `explode`) |

- A path with a query of its own, such as `/responses?beta=true`, keeps it.
- A parameter whose name another parameter of the operation takes (`id` in the query and in a
  header) gets a `_query`, `_header` or `_cookie` suffix in the SDK. Its wire name is untouched.

### Path parameters

- A path parameter is typed like a field of the same schema: integers, numbers, booleans,
  dates and enums (named or inline) keep their type, so a value read off a response can be
  passed back as is. Strings and unions of scalars are text.
- Others follow their `style` and `explode`: `label` (`.a.b`), `matrix` (`;id=a,b`), lists and
  objects (`a,b,c`, `k,v,k2,v2`, or `k=v,k2=v2` exploded).
- Path variables the path uses without declaring them are strings.

## Bodies and responses

- A request body can be any JSON schema: a bare list or scalar, a boolean schema, a recursive
  alias (`Tree: array of Tree`, typed as plain JSON where an SDK cannot spell it).
- `type: object` without properties is a map of any JSON values (`{ [key: string]: unknown }`,
  `dict[str, t.Any]`...), `{}` any JSON value.
- A required property of one value (`const`, or an `enum` of one value) is filled in by the
  SDKs, and typed as that value in TypeScript (`"chat.completion"`) and Python (`t.Literal`).
- Nullable items, map values and response bodies stay nullable. A `null` body decodes to the
  language's empty value.
- An operation declaring both a body and a bodiless 2xx (such as `204`) returns an optional
  value, empty for the bodiless response.

## Base URL

Every operation goes to the client's one base URL. Operations or path items declaring `servers`
that the root `servers` do not list produce a warning naming them. Put them in a spec of their
own, or set the base URL when creating the client.

## Unsupported constructs

An operation using a construct perseid does not support is skipped, with a warning naming it.
See [spec support](configuration.md#spec-support).

Java and TypeScript also skip, with a warning, the operations their HTTP client cannot send:
a body on a GET or HEAD (OkHttp and `fetch` refuse it), and TRACE in TypeScript. The other
SDKs keep them.
