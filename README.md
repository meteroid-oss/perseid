<p align="center"><img src=".github/cover.svg" alt="perseid: OpenAPI in, idiomatic SDKs out" width="100%"></p>

**GitHub-native SDK generation. No cloud, no subscription.**
Change your OpenAPI spec, and idiomatic Rust, TypeScript, Python, Go and Java SDKs regenerate
and land as pull requests, in one repository or one per language. One static binary, running in
your CI: an open-source alternative to Fern, Speakeasy and Stainless.

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

It powers the [Meteroid SDKs](https://github.com/meteroid-oss/meteroid-clients), and started as a fork of
[Svix's openapi-codegen](https://github.com/svix/openapi-codegen).

## What you get

- **SDKs that read like handwritten code.** Typed models and enums, resource namespaces
  (`client.customers().list(...)`), retries, request ids, sync and async flavors where the language has them.
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
   `getInterceptors()` in Java (OkHttp interceptors) and `Middleware` in Rust.
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
[go]
module = "github.com/acme/acme-go"
repo = "acme/acme-go"               # lives in its own repository
```

Every table also takes `path`, `version`, `base_url`, `header_prefix`, `user_agent`, `webhooks` and a
`context` table exposed to templates. `perseid inspect` prints the model templates receive.

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

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt` and `google-java-format`, with your
SDK's own formatter configuration. Locally, missing `biome` or `ruff` run pinned through `npx` or
`uvx`. The GitHub Action reads the languages from `perseid.toml` and installs only the pinned
native formatters they need: no JVM, Node or Python setup.

## Status

Early, and honest about it:

- **OpenAPI 3.0 and 3.1**, JSON or YAML. 3.0 documents are upgraded to 3.1 on load
  (`nullable`, boolean `exclusiveMinimum`/`exclusiveMaximum`). Swagger 2.0 is rejected: convert
  it first, for example with `npx swagger2openapi`.
- Proven on [Meteroid's API](https://github.com/meteroid-oss/meteroid-clients) and our test
  specs, not yet on hundreds of APIs. Unsupported constructs make generation fail instead of
  being skipped: an issue with the spec attached is the fastest way to get one supported.
- Not there yet: pagination helpers, auth other than bearer tokens, streaming outside Rust,
  publishing to package registries (keep your usual release workflow). C# is next.

## License

Apache-2.0. Includes MIT-licensed code from Svix and Meteroid, see [NOTICE](NOTICE).
