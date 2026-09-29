# perseid

**OpenAPI in, idiomatic SDKs out.** Rust, TypeScript, Python, Go and Java from one static binary.
No account, no Docker, no JVM. An open-source, self-hosted alternative to Fern, Speakeasy and Stainless.

```sh
curl -fsSL https://sh.meteroid.com/perseid | sh

perseid init       # finds openapi.json, writes perseid.toml and package skeletons
perseid generate   # five formatted SDKs, in seconds
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

It powers the [Meteroid SDKs](https://github.com/meteroid-oss/meteroid-clients), and descends from
the generator [Svix](https://github.com/svix/svix-webhooks) uses for its own client libraries.

## What you get

- **SDKs that read like handwritten code.** Typed models and enums, resource namespaces
  (`client.customers().list(...)`), retries, request ids, sync and async flavors where the language has them.
- **Your code stays yours.** Perseid only rewrites or deletes files it marked `@generated`.
  Errors, helpers, tests and READMEs live right next to generated code.
- **Templates you own.** `perseid eject rust` copies the built-in Jinja templates and runtime to
  `.perseid/`. Keep the files you changed, delete the rest to keep receiving upstream updates.
- **CI-native.** `perseid generate --check` fails on drift. `perseid generate --pr` commits to
  `perseid/update` and opens a pull request, in this repository or in dedicated SDK repositories.

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

Every table also takes `path`, `version`, `base_url`, `header_prefix`, `user_agent` and a
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

## License

Apache-2.0. Includes MIT-licensed code from Svix and Meteroid, see [NOTICE](NOTICE).
