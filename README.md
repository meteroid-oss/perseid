<p align="center"><img src=".github/cover.svg" alt="perseid: OpenAPI in, idiomatic SDKs out" width="100%"></p>

# perseid

**Idiomatic SDKs, always in sync with your OpenAPI spec.**

perseid generates Rust, TypeScript, Python, Go, Java and C# SDKs. When the spec changes, a GitHub
Action regenerates them, opens a pull request, and releases them to their registries once you
merge. It is one static binary. There is no hosted service, no account and no subscription.

## Why perseid

- **Code you would write by hand.** Resource namespaces, typed models and errors, iterators over
  paginated lists. No `DefaultApi`, no `getPetsWithHttpInfo`.
- **Your CI, your repositories.** Generation runs in a GitHub Actions job you can read. SDKs live
  next to your API, in one SDKs repository, or in a repository per language.
- **Releases included.** Each pull request is sized from the spec diff, so a breaking change asks
  for a major bump. release-please tags each SDK and publishes it with trusted publishing.
- **Your edits survive.** perseid only rewrites or deletes files it marked `@generated`.

## Quick start

In the repository that holds your OpenAPI spec:

```sh
npx perseid init              # pick the languages and where the SDKs live
gh secret set PERSEID_TOKEN   # paste a fine-grained token, see below
git add -A && git commit -m "ci: generate SDKs with perseid" && git push
```

The push runs the `SDKs` workflow, which opens a pull request with every SDK.

`PERSEID_TOKEN` is a [fine-grained token](https://github.com/settings/personal-access-tokens/new)
with **Contents**, **Pull requests** and **Workflows** set to read and write, on this repository
and the SDK repositories. Fine-grained tokens expire, and perseid warns 30 days before.
`npx perseid app` sets up a GitHub App instead, which doesn't expire.

To see the SDKs before pushing anything:

```sh
npx perseid generate --out /tmp/sdks   # every SDK in /tmp/sdks/<language>
```

perseid also installs with `curl -fsSL https://sh.meteroid.com/perseid | sh`
or runs as the `ghcr.io/meteroid-oss/perseid` image. Linux and macOS, x64 and arm64.

## What your users get

```ts
const petstore = new Petstore({ apiKey: "sk_live_..." });
const pets = await petstore.pets.list({ limit: 10, status: "available" });
```

```python
petstore = Petstore(api_key="sk_live_...")
pets = petstore.pets.list(limit=10, status=PetStatus.AVAILABLE)
```

```csharp
using var petstore = new PetstoreClient("sk_live_...");
var pets = await petstore.Pets.ListAsync(new() { Limit = 10, Status = PetStatus.Available });
```

## Features

| | |
|---|---|
| Languages | Rust, TypeScript, Python (sync and async), Go, Java, C# |
| Requests | Typed errors by status, retries with backoff and `Retry-After`, idempotency keys, per-call timeouts and headers |
| Auth | Bearer, basic, API keys, OAuth2 client credentials, token providers |
| Data | Cursor, page and offset pagination, server-sent events, file uploads |
| Models | Enums and unions that keep values the SDK does not know, unknown properties sent back |
| Webhooks | An opt-in [Standard Webhooks](https://www.standardwebhooks.com) verifier in every language |
| Tests | A test per operation in every SDK, run in process against the response the spec declares |
| Docs | An `api.md` per SDK, regenerated with the code, and a README calling your API's own operations |
| Specs | OpenAPI 3.0, 3.1 and 3.2, external `$ref`s, parameter styles, nullable and recursive schemas, webhook-only specs |
| Customizing | Middleware, resource snippets, ejectable templates |
| CI | `perseid generate --check` fails when the SDKs drift from the spec |

## How it works

```
 openapi.json changes on main
          │
          ▼
 sdks.yml ─ perseid generate --pr ─ oasdiff sizes the change
          │
          ▼
 pull request "feat(api)!: update SDKs to Acme 2.0"   ← you review and merge
          │
          ▼
 sdk-release.yml ─ release-please PR ← you merge ─ tag ─ publish to npm, PyPI, crates.io…
```

`perseid init` writes two workflows for you to commit:

- `sdks.yml` runs the `meteroid-oss/perseid` Action when the spec changes. It installs perseid
  and the pinned formatters, then runs `perseid generate --pr`, which commits the SDKs to the
  `perseid/update` branch and opens or updates one pull request per repository.
- `sdk-release.yml` runs release-please on merge and publishes each released SDK from the
  `release` environment.

The Actions are pinned to the release line of the perseid that wrote them, such as
`meteroid-oss/perseid@v0.6`. Run `perseid init` again to refresh them.

## Where the SDKs live

`perseid init` asks. The answer goes in `perseid.toml`:

| Layout | `perseid.toml` | Pull requests open in |
|---|---|---|
| Next to the API | `sdks = ["typescript", "python"]` | `acme/api`, in `typescript/` and `python/` |
| One repository per language | `repo = "acme/api-{lang}"` | `acme/api-typescript`, `acme/api-python` |
| One SDKs repository | `repo = "acme/api-sdks"` | `acme/api-sdks`, a folder per language |

- perseid never creates repositories: `gh repo create acme/api-typescript`.
- Add `PERSEID_TOKEN` to each SDK repository too: its release workflow uses it.
- Spec in another repository? Run `perseid init` in the SDKs repository, then
  `npx perseid connect acme/api-sdks` in the API repository. It writes a workflow pushing the
  spec, with a deploy key that reaches the SDKs repository only.
- A spec served at a URL needs no `connect`: `sdks.yml` fetches it daily.

See [repository layouts](docs/ci.md#repository-layouts).

## Commands

| Command | |
|---|---|
| `init` | Write `perseid.toml` and the workflows. Local only: nothing is sent to GitHub. |
| `generate` | Write the SDKs. `--out <dir>` previews, `--check` fails on drift, `--pr` opens pull requests. |
| `connect <owner/repo>` | In the API repository: push the spec to the SDKs repository. |
| `app` | Set up a GitHub App to open the pull requests instead of `PERSEID_TOKEN`. |
| `status` | Check the setup: secrets, workflows, last spec pushed, open pull requests, last runs. |
| `inspect` | Print the model the templates receive, as JSON. |
| `eject <lang>` | Copy the built-in templates and runtime of a language to `.perseid/` to edit them. |
| `tools list`, `tools install` | List or download the pinned formatters and oasdiff. |

`generate --pr` also takes `--bump`, `--auto-merge` and `--dispatch`, described in
[CI and releases](docs/ci.md#github-action). It runs on your machine too, without the `gh` CLI. It
signs in with `GH_TOKEN`, `GITHUB_TOKEN`, the token `gh` stores, or a browser login.

## Docs

- [CI and releases](docs/ci.md): the Actions, tokens, spec pushes, release-please, publishing
- [Configuration](docs/configuration.md): every key of `perseid.toml`
- [Languages](docs/languages.md): what each SDK looks like
- [Auth, pagination, streaming, raw responses and encoding](docs/features.md)
- [Customizing](docs/customizing.md): handwritten code, middleware, snippets, templates, webhooks
- [Testing](docs/testing.md): how perseid itself is tested, for contributors

## Status

perseid reads OpenAPI 3.0, 3.1 and 3.2, in JSON or YAML. Convert Swagger 2.0 first, for example with
`npx swagger2openapi`.

It generates the [Meteroid SDKs](https://github.com/meteroid-oss/meteroid-clients). SDKs from the
Stripe, GitHub, OpenAI, Twilio, DigitalOcean and Linode specs compile in every language, with one
operation excluded on Stripe and on OpenAI.

A construct no SDK can express is skipped or typed as untyped JSON, with a warning naming the
operation or schema. Names that clash fail generation, listed together. Leave an operation out
with `exclude = ["<operation id>"]`, and open an issue with the spec attached.

perseid started as a fork of [Svix's openapi-codegen](https://github.com/svix/openapi-codegen).

## License

Apache-2.0. Includes MIT-licensed code from Svix and Meteroid, see [NOTICE](NOTICE).
