# Comparison

How perseid compares to other OpenAPI SDK generators, as of October 2026. perseid's maintainers
wrote this page. Products in this space change fast: check the [sources](#sources) before you
decide.

## What changed in 2026

- **Stainless** joined Anthropic on May 18, 2026 and wound down its hosted products, the SDK
  generator included. New sign-ups, projects and SDKs closed that day. Customers own the SDKs
  generated so far. No end date was published for existing projects.
- **Fern** was acquired by Postman in January 2026. Its SDK generators are Apache-2.0 on GitHub.
  Generation runs on Fern's infrastructure unless you have the Enterprise plan.
- **Speakeasy** open-sourced its generator under AGPL-3.0 on September 17, 2026, with Google.
  Without a commercial license, the Speakeasy code in the SDKs it generates is AGPL-3.0-only.

## At a glance

### Hosting, license and price

| | Generator license | Runs in | Account | Price |
|---|---|---|---|---|
| perseid | Apache-2.0 | Your GitHub Actions, as one binary or a Docker image | None | Free |
| Stainless | Proprietary | Stainless's cloud | Yes | Closed to new customers |
| Fern | Apache-2.0 generators, proprietary platform | Fern's cloud. Your infrastructure on Enterprise, with a `FERN_TOKEN` | Yes | Free: TypeScript and Python, up to 200 endpoints. Enterprise: per SDK, on quote |
| Speakeasy | AGPL-3.0, or commercial | Your machine or CI (open source); the Speakeasy CLI and platform (commercial) | No for AGPL output, yes otherwise | AGPL: free, the output is AGPL. Commercial: one free SDK up to 50 methods, paid plans on quote |
| Scalar | Proprietary | Scalar's cloud | Yes | 1 SDK up to 25 endpoints free. Pro $150/month with 1 SDK up to 100 endpoints, each extra SDK $150 to $600/month |
| openapi-generator | Apache-2.0 | Anywhere: Java 11, an npm wrapper or Docker | None | Free |
| Kiota | MIT | Anywhere: a .NET tool or Docker | None | Free |

perseid needs no account. `perseid sync` installs the perseid GitHub App, whose tokens come from
a broker Meteroid runs ([perseid-gh](https://github.com/meteroid-oss/perseid-gh)). To avoid it,
use a GitHub App of your own (`perseid app`) or a token: see [credentials](ci.md#credentials).

### Languages and other outputs

| | SDK languages | Other outputs |
|---|---|---|
| perseid | Rust, TypeScript, Python (sync and async), Go, Java, C# | `api.md` and a README per SDK |
| Stainless | TypeScript, Python, Go, Java, Kotlin, Ruby, PHP, C# | CLI, MCP server, Terraform provider, docs platform |
| Fern | TypeScript, Python, Java, Go, C#, Ruby, PHP, Swift, Rust. Kotlin in progress | CLI, docs sites, WebSockets (AsyncAPI), gRPC |
| Speakeasy | TypeScript, Python, Go, Java, C#, PHP, Ruby, Unity | MCP server (TypeScript), CLI, Terraform provider, Postman collections |
| Scalar | TypeScript, Python, Go. Experimental: Java, C#, PHP, Ruby, Swift, Rust, Kotlin, Dart, C++ | CLI, docs, hosted MCP servers |
| openapi-generator | About 40 client languages, often with a choice of HTTP library | Server stubs, docs, config files |
| Kiota | C#, Go, Java, PHP, Python. Preview: TypeScript, Ruby, Dart | Microsoft 365 Copilot API plugins |

### Generated SDKs

| | Code style | Pagination | Retries | Errors |
|---|---|---|---|---|
| perseid | Resource namespaces: `client.customers.list()` | Cursor, page and offset, from `x-pagination` or rules in `perseid.toml`. Lists iterate across pages | Backoff, `Retry-After`, opt-in idempotency keys | A type per status, with the declared error schema decoded |
| Stainless | Resource namespaces | Cursor, offset and page, configured | Backoff, idempotency keys | A type per status |
| Fern | Resource namespaces | Offset, cursor and link | Backoff, idempotency headers | Typed errors |
| Speakeasy | Resource namespaces | `x-speakeasy-pagination` | Configured with `x-speakeasy-retries` | Typed errors |
| Scalar | Resource namespaces, configured in a resource tree | Declared schemes | Configured | Error messages read as configured |
| openapi-generator | A class per tag (`PetApi`, `DefaultApi`), `...WithHttpInfo` variants. Varies by generator | None | Not in most generators | A generic API exception with status and body |
| Kiota | Request builders following the path: `client.Users["id"].Messages.GetAsync()`, on Kiota runtime packages | None generated | A middleware for 429 and 503 | Classes for declared error responses |

### Releases and customization

| | Release automation | Breaking-change detection | Your edits across regenerations |
|---|---|---|---|
| perseid | A pull request per spec change, then release-please and trusted publishing from your repositories | oasdiff sizes each pull request: `feat!:`, `feat:` or `fix:`. Changelogs name breaking changes | Whole files: perseid only rewrites files marked `@generated`. Middleware, resource snippets, ejected templates |
| Stainless | Release PRs and publishing, hosted | Enterprise feature | Three-way merge of custom code |
| Fern | Pull requests or pushes to your SDK repositories. Autorelease (early access) bumps and publishes | Autorelease analyzes the API diff | `.fernignore` for whole files. Replay, a three-way merge, for edits (Enterprise) |
| Speakeasy | A GitHub Action that opens a PR or publishes directly | Bumps from `info.version` and a spec checksum, not from the spec's content | Persistent edits, a three-way merge. Code regions (Enterprise), hooks |
| Scalar | A release PR, then release-please and publish workflows in your repository | Not documented | Three-way merge on a `scalar-next` branch |
| openapi-generator | None | None | `.openapi-generator-ignore`, custom templates |
| Kiota | None | None | None built in |

## Where others are ahead

- **Languages.** Ruby, PHP, Kotlin, Swift, Dart and C++ are not perseid targets. openapi-generator
  covers far more.
- **More than SDKs.** perseid makes no docs site, CLI, MCP server or Terraform provider. Stainless
  made all four, Speakeasy makes the last three, Fern and Scalar make docs and CLIs, and Scalar
  hosts MCP servers.
- **Edits inside generated files.** Speakeasy, Scalar, Stainless and Fern (on Enterprise) merge
  your edits into generated files. perseid keeps whole files you own, and offers middleware,
  snippets and templates for the rest.
- **Protocols and spec formats.** Fern reads AsyncAPI, gRPC and its own definition format.
  openapi-generator reads Swagger 2.0, which perseid rejects. perseid skips OpenAPI 3.2's `QUERY`
  method.
- **GitHub only.** perseid's pull requests, releases and App are GitHub's. `perseid generate` and
  `--check` run in any CI, but the release flow does not.
- **Support.** The commercial vendors sell support, SLAs and migrations. perseid has GitHub issues.
- **Maturity.** perseid is at 0.12. Before 1.0, its configuration and output can still break.

## Where perseid is ahead

- **Nothing to sign up for.** Generation, pull requests and publishing run in your GitHub Actions,
  with your registries' trusted publishing. No vendor holds your spec or your release keys.
- **Your license on the output.** The generator is Apache-2.0, and each SDK gets the `license`
  set in `perseid.toml`. Speakeasy's open-source path makes its code in your SDKs AGPL-3.0.
- **Free in every language.** No per-SDK, per-endpoint or per-seat pricing.
- **Rust.** Of the commercial tools, only Fern generates Rust. Scalar's is experimental.
- **Releases sized from the spec.** oasdiff compares the spec with its previous version, so a
  removed endpoint asks for a major bump and the changelog names it.

## Choose

### perseid

Choose perseid if the six languages cover you, you release from GitHub, and you want the SDKs in
your own repositories with no vendor account or bill. Look elsewhere if you need another
language, a docs site, a CLI, an MCP server, or edits merged inside generated files.

### Speakeasy

Choose Speakeasy if you need Ruby, PHP, Unity, a Terraform provider or an MCP server, and either
accept AGPL-3.0 code in your SDKs or pay for a commercial license.

### Fern

Choose Fern if you want docs and SDKs from one vendor, already use Postman, or need WebSockets,
gRPC or Swift. Custom code merged into generated files and self-hosting are on Enterprise.

### Scalar

Choose Scalar if you want hosted docs and SDKs together, for a small API, mostly in TypeScript,
Python and Go.

### Stainless

Stainless takes no new customers. If your SDKs are there, plan a move. perseid generates a similar
call style: resources, lists that page on their own, errors typed by status.

### openapi-generator

Choose openapi-generator for a language nobody else covers, or for server stubs, when idiomatic
code, pagination and retries matter less.

### Kiota

Choose Kiota for clients that follow the API's paths, Microsoft Graph style, in C#, Go, Java,
PHP or Python, with no vendor.

## Sources

As of October 2026.

- Stainless: [Stainless is joining Anthropic](https://www.stainless.com/blog/stainless-is-joining-anthropic),
  [Anthropic acquires Stainless](https://www.anthropic.com/index/anthropic-acquires-stainless),
  [Stainless docs](https://www.stainless.com/docs),
  [breaking change detection](https://www.stainless.com/docs/enterprise/breaking-change-detection),
  [Scalar's wind-down notes](https://scalar.com/resources/stainless-wind-down)
- Fern: [Postman acquires Fern](https://www.businesswire.com/news/home/20260107174767/en/Postman-Acquires-Fern-to-Help-Businesses-Deliver-World-Class-Developer-Experiences),
  [fern-api/fern](https://github.com/fern-api/fern), [pricing](https://buildwithfern.com/pricing),
  [capabilities](https://buildwithfern.com/learn/sdks/overview/capabilities),
  [self-hosted SDKs](https://buildwithfern.com/learn/sdks/deep-dives/self-hosted),
  [custom code](https://buildwithfern.com/learn/sdks/overview/custom-code),
  [Autorelease](https://buildwithfern.com/learn/sdks/overview/autorelease)
- Speakeasy: [Why client SDK generation belongs in the open](https://developers.googleblog.com/why-client-sdk-generation-belongs-in-the-open/),
  [speakeasy-api/openapi-generation](https://github.com/speakeasy-api/openapi-generation),
  [its licensing](https://github.com/speakeasy-api/openapi-generation/blob/main/LICENSING.md),
  [getting started](https://www.speakeasy.com/docs/sdks/introduction),
  [versioning](https://www.speakeasy.com/docs/sdks/manage/versioning),
  [custom code](https://www.speakeasy.com/docs/sdks/customize/code/custom-code/custom-code),
  [GitHub setup](https://www.speakeasy.com/docs/sdks/manage/github-setup),
  [pricing](https://www.speakeasy.com/pricing)
- Scalar: [pricing](https://scalar.com/pricing), [SDK generator](https://scalar.com/products/sdks),
  [configuration](https://scalar.com/products/sdks/configuration),
  [custom code](https://scalar.com/products/sdks/custom-code),
  [publishing](https://scalar.com/products/sdks/publishing)
- openapi-generator: [OpenAPITools/openapi-generator](https://github.com/OpenAPITools/openapi-generator)
- Kiota: [microsoft/kiota](https://github.com/microsoft/kiota),
  [middleware](https://learn.microsoft.com/en-us/openapi/kiota/middleware)
- perseid: [features](features.md), [languages](languages.md), [CI and releases](ci.md),
  [customizing](customizing.md), [spec support](configuration.md#spec-support)
