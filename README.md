
<p align="center"><img src=".github/cover.svg" alt="perseid: OpenAPI in, idiomatic SDKs out" width="100%"></p>

# perseid

Change your OpenAPI spec, and idiomatic Rust, TypeScript, Python, Go, Java and C# SDKs regenerate
and land as pull requests, in one repository or one per language. 

One static binary, running in your CI: an open-source and headless alternative to Fern, Speakeasy and Stainless.

```sh
curl -fsSL https://sh.meteroid.com/perseid | sh

perseid init       # finds openapi.json, writes perseid.toml and package skeletons
perseid generate   # formatted SDKs for every language, in seconds
```

```ts
const petstore = new Petstore("sk_live_...");
const pets = await petstore.pets.listPets({ limit: 10, status: "available" });
```

```python
petstore = Petstore("sk_live_...")
pets = petstore.pets.list_pets(limit=10, status=PetStatus.AVAILABLE)
```

```go
pets, err := petstore.New("sk_live_...", nil).Pets().ListPets(ctx, &petstore.PetsListPetsOptions{Limit: petstore.Ptr[int32](10)})
```

```csharp
using var petstore = new PetstoreClient("sk_live_...");
var pets = await petstore.Pets.ListPetsAsync(new() { Limit = 10, Status = PetStatus.Available });
```

It powers the [Meteroid SDKs](https://github.com/meteroid-oss/meteroid-clients), and started as a fork of
[Svix's openapi-codegen](https://github.com/svix/openapi-codegen).

## What you get

- **SDKs that read like handwritten code.** Typed models and enums, resource namespaces
  (`client.customers().list(...)`), retries, request ids, sync and async flavors where the language has them.
- **Auth, pagination and streaming.** Every `securitySchemes` flavor clients need (bearer, basic,
  API keys, OAuth2 tokens), iterators over paginated lists, server-sent events and file uploads.
- **Your code stays yours.** Perseid only rewrites or deletes files it marked `@generated`, and
  refuses to overwrite anything else. Errors, helpers, tests and READMEs live right next to generated code.
- **Templates you own.** `perseid eject rust` copies the built-in Jinja templates and runtime to
  `.perseid/`. Keep the files you changed, delete the rest to keep receiving upstream updates.
- **CI-native.** `perseid generate --check` fails on drift. `perseid generate --pr` commits to
  `perseid/update` and opens a pull request, in this repository or in dedicated SDK repositories.

## Customizing

Four extension points, none of which needs a fork:

1. **Handwritten files.** Perseid never deletes a file without the `@generated` marker, and refuses to
   overwrite one: put your modules next to the generated ones. In Go, define extra methods on the
   generated resource types from any file of the package.
2. **Middleware.** Every SDK lets you wrap each HTTP attempt to cache, log, sign or rewrite requests:
   `middleware` in the TypeScript options (`(request, next) => Response`), `middleware` and
   `async_middleware` in the Python options, `Options.Middleware` in Go (a `RoundTripper` wrapper),
   `getInterceptors()` in Java (OkHttp interceptors, also added to your own client given to
   `setHttpClient`) and `Middleware` in Rust.
3. **Resource snippets.** A file at `.perseid/templates/<lang>/extensions/<resource>.<ext>` is inlined
   in the body of that resource's class (Rust, TypeScript, Java, and Python where
   `<resource>_async.py` targets the async class), to add custom methods.
4. **Ejected templates and runtime.** `perseid eject <lang>` copies the built-ins to `.perseid/`; files
   you keep there override the built-ins, and `context` tables in `perseid.toml` reach templates as `sdk.*`.

A local cache for GET endpoints, as TypeScript middleware:

```ts
const cache = new Map<string, string>();
const petstore = new Petstore("sk_live_...", {
  middleware: [
    async (request, next) => {
      if (request.method !== "GET" || !new URL(request.url).pathname.startsWith("/pets")) {
        return next(request);
      }
      const hit = cache.get(request.url);
      if (hit) return new Response(hit, { headers: { "content-type": "application/json" } });
      const response = await next(request);
      if (response.ok) cache.set(request.url, await response.clone().text());
      return response;
    },
  ],
});
```

and in Rust:

```rust
struct Cache(Mutex<HashMap<String, Bytes>>);

impl Middleware for Cache {
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Result<Response, BoxError>> {
        Box::pin(async move {
            let key = request.uri().to_string();
            if request.method() != Method::GET || !request.uri().path().starts_with("/pets") {
                return next.run(request).await;
            }
            if let Some(body) = self.0.lock().unwrap().get(&key).cloned() {
                return Ok(Response::buffered(StatusCode::OK, HeaderMap::new(), body));
            }
            let response = next.run(request).await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let (status, headers, body) = response.into_parts().await?;
            self.0.lock().unwrap().insert(key, body.clone());
            Ok(Response::buffered(status, headers, body))
        })
    }
}

let mut options = PetstoreOptions::default();
options.middleware.push(Cache(Default::default()));
```

Middleware runs inside the retry loop, and the first one registered is the outermost.

### Webhooks

Set `webhooks = true` (top-level or per language table) to install a dependency-free
[Standard Webhooks](https://www.standardwebhooks.com) verifier, also accepting Svix's `svix-*` headers:

```ts
new Webhook("whsec_...").verify(rawBody, request.headers); // throws WebhookVerificationError
```

It is `Webhook` in every language (`Webhook::new(secret)?.verify(&body, &headers)` in Rust,
`NewWebhook(secret)` then `Verify(body, r.Header)` in Go). Payload models come from `webhooks` and
`x-webhooks` in the spec. In Rust the verifier lives behind the `webhooks` cargo feature that `init`
adds to `Cargo.toml` and `lib.rs`.

### Rust

Enums and unions are `#[non_exhaustive]` and decode values this version does not know into an
`Unknown` variant that serializes back unchanged. Recursive fields are boxed. Dates are `chrono`
types when the crate depends on `chrono`, as `init` sets it up, and strings otherwise. In PATCH
bodies, nullable optional fields are `Option<Option<T>>`: `Some(None)` sends `null`.

Failed requests are retried on connection errors, timeouts, 408, 429 and 5xx responses, honoring
`Retry-After`, but only when the method is idempotent or the request carries an `Idempotency-Key`
(POST gets one automatically). `Options::with_connector` takes any hyper connector, for custom
TLS roots, client certificates or a proxy, and `http_client` any `HttpClient` implementation.

`src/error.rs` is yours: the runtime builds errors with `Error::generic(Failure)` and
`Error::from_response(status, headers, body)` only. The scaffolded one is an enum of `Api`,
`Timeout`, `Transport`, `Decode` and `Request` errors with `source()`.

## perseid.toml

```toml
spec = "openapi.json"               # or an https:// URL, JSON or YAML
name = "Acme"                       # Acme client, `acme` packages
base_url = "https://api.acme.com"

[rust]                              # generated into ./rust
[typescript]
package = "@acme/sdk"
[python]
[java]
package = "com.acme.sdk"
[csharp]                            # Acme namespace and NuGet package by default
[go]
module = "github.com/acme/acme-go"
repo = "acme/acme-go"               # lives in its own repository
```

The Python SDK needs Python 3.10+. Its models are keyword-only dataclasses; an optional field
that accepts `null` defaults to `UNSET`, so `None` sends `null`, and enum values or union variants
newer than the SDK are kept rather than rejected. Methods take `extra_headers=` and `timeout=`.
Unions are models holding the discriminator and the variant (`Shape(type=..., content=Circle(...))`);
`flat_unions = true` in `[python.context]` types them as `Circle | Square` instead.

Every table also takes `path`, `version`, `base_url`, `header_prefix`, `user_agent`, `webhooks`,
`exclude` (operation ids left out of that SDK only) and a `context` table exposed to templates. `perseid inspect` prints the model templates receive. In
templates, `name | ident("snake", "rust")` turns a spec name into an identifier (cases `snake`,
`camel`, `pascal` and `shouty`, keywords escaped for `rust`, `python`, `go`, `java` or
`typescript`), and `names | idents(case, language, owner)` fails when two names collide.

`[typescript]` also takes `int64`: `"number"` (the default, exact up to 2^53), `"bigint"` or
`"string"`. With the last two, responses are parsed without losing digits, and `parseJson` /
`stringifyJson` do the same for webhook payloads. The TypeScript package ships ESM and CommonJS
builds behind an `exports` map; every method takes a last `{ signal, headers, timeout }` argument,
and models convert with `XSerializer.parse(json)` / `XSerializer.serialize(value)`.

In `[go]`, `initialisms = true` spells names the Go way (`CustomerID`, `APIKey`) and
`patch_nullable = true` types the optional nullable fields of PATCH bodies as `*Nullable[T]`,
so `Null[T]()` clears a field. `perseid init` turns both on; they rename or retype public API, so
existing SDKs opt in. Every Go method takes trailing options for one call:
`WithHeader`, `WithTimeout`, `WithIdempotencyKey` and `WithMaxRetries`. Retries cover network
errors, 408, 429 and 5xx, honor `Retry-After` up to a minute and add jitter to the backoff.

## Authentication

Clients read `components.securitySchemes` and honor the `security` of each operation, including
`security: []` for public endpoints. For every request they send the credentials of the first
alternative that is fully configured:

| Scheme | Configured with |
|---|---|
| `http` bearer, `oauth2`, `openIdConnect` | the constructor token, or a token provider called before each request |
| `http` basic | `basicAuth` / `basic_auth` / `BasicAuth` / `setBasicAuth` |
| `apiKey` in a header, query parameter or cookie | the constructor token, or per scheme in `apiKeys` / `api_keys` |

```ts
new Acme("sk_live_...");                                          // unchanged
new Acme(null, { tokenProvider: () => oauth.accessToken() });     // OAuth2, refreshed by you
```

A spec without `securitySchemes` keeps sending `Authorization: Bearer <token>`.

## Pagination

Mark list operations with `x-pagination`, or describe them once in perseid.toml to match every
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

`x-pagination: false` opts an operation out of perseid.toml rules. Paginated operations get an iterator next to the list method:

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
Java a `Paginator<Customer>` from `listCustomersIter()` that is `Iterable` and has `stream()`.

## Streaming

`text/event-stream` responses return an event stream (`for await`, `for`, `Next()`, `Iterable`,
`next().await`), `multipart/form-data` bodies a typed `...Body` with `Upload` files, and
`application/octet-stream` bodies accept bytes or streams. Streamed uploads are not retried.

## From your API repository to SDK pull requests

```yaml
# .github/workflows/sdks.yml in the repository that owns openapi.json
on:
  push:
    branches: [main]
    paths: [openapi.json, perseid.toml]
jobs:
  sdks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: meteroid-oss/perseid@v0
        with:
          token: ${{ secrets.SDK_TOKEN }}   # contents + pull requests write on the SDK repositories
```

For pull request checks, pass `command: generate --check`.

## Releases

Every SDK keeps its own version, in its own manifest, and is released on its own. `perseid init`
also scaffolds [release-please](https://github.com/googleapis/release-please)
(`release-please-config.json`) and `.github/workflows/sdk-release.yml` (skip them with
`--no-release`), and gives SDK repositories without a manifest the same on their first pull request:

1. The Action titles its pull request as a conventional commit sized by
   [oasdiff](https://github.com/oasdiff/oasdiff): `feat(api)!:` for breaking API changes,
   `feat(api):` for other API changes, `fix(api):` otherwise (`bump` input, `--bump` flag).
2. Merging it, or any `fix:`/`feat:` commit touching an SDK, opens a release PR bumping the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK (`rust/v0.4.0`, or `v0.4.0` alone in its repository) and
   publishes it through `meteroid-oss/perseid/publish`: trusted publishing (OIDC) for crates.io,
   npm and PyPI, a Central Portal token and GPG key for Maven Central, the module proxy for Go.
   Versions already on the registry are skipped, so re-running a failed job is safe.

`relax-enum-additions` (default `true`) counts enum values added to responses as minor changes.
That is only safe while the SDKs accept unknown enum values; set it to `false` otherwise.

`auto-merge: true` enables GitHub auto-merge on the generated pull requests and, through their
`perseid:auto-release` label, on the release PRs they lead to. It needs:

- "Allow auto-merge" in the repository settings;
- required status checks, through branch protection or rulesets: without any, GitHub merges at once;
- a GitHub App token as the Action's `token` and as the `RELEASE_TOKEN` secret, since merges made
  with the default `GITHUB_TOKEN` trigger no workflow, so nothing would be released or published.

## Self-hosted runners and other CIs

`ghcr.io/meteroid-oss/perseid` bundles perseid, git, gh and every pinned formatter.

```yaml
jobs:
  sdks:
    runs-on: self-hosted
    container: ghcr.io/meteroid-oss/perseid:0.2.0
    steps:
      - uses: actions/checkout@v4
      - run: gh auth setup-git && perseid generate --pr
        env:
          GH_TOKEN: ${{ secrets.SDK_TOKEN }}
```

Or anywhere: `docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/work" ghcr.io/meteroid-oss/perseid generate --check`.

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration. Locally, missing `biome` or `ruff` run pinned through
`npx` or `uvx`. The GitHub Action reads the languages from `perseid.toml` and installs only the
pinned native formatters they need: no JVM, Node or Python setup (`csharpier` installs as a .NET
tool).

## Status

Early, and honest about it:

- **OpenAPI 3.0 and 3.1**, JSON or YAML. 3.0 documents are upgraded to 3.1 on load
  (`nullable`, boolean `exclusiveMinimum`/`exclusiveMaximum`). Swagger 2.0 is rejected: convert
  it first, for example with `npx swagger2openapi`.
- Proven on [Meteroid's API](https://github.com/meteroid-oss/meteroid-clients) and our test
  specs. SDKs generated from the GitHub, OpenAI, Twilio and Stripe specs compile in Rust,
  TypeScript, Python, Go and Java once a few operations are excluded. `$ref` parameters, bodies
  and responses, inline objects, `allOf` compositions, list bodies and `application/*+json` are
  read as is; inline objects become named types.
- Unsupported constructs make generation fail instead of being skipped, each listed with its
  operation id and path or its schema: `exclude = ["<operation id>"]` skips an operation, and an
  issue with the spec attached is the fastest way to get one supported. Unions without a
  discriminator are typed as untyped JSON, with a warning. A variant missing from the
  discriminator `mapping` is tagged with the `const` or `enum` of its discriminator property,
  falling back to its schema name.
- Not there yet: a built-in OAuth2 token exchange (bring a token provider), and auth,
  pagination and streaming in C#. C# generation fails on upload and event stream operations,
  naming them for `exclude` in its `[csharp]` table.

## License

Apache-2.0. Includes MIT-licensed code from Svix and Meteroid, see [NOTICE](NOTICE).
