# Perseid

Perseid generates SDKs from OpenAPI for **Rust, Java, TypeScript, Python, and Go**.
It brings the generator, all 49 inherited template files, shared HTTP/runtime
sources, and regeneration scripts into one versioned project. SDK repositories
supply their own specs and configuration. Version 0.1.0 is the initial extraction.

This is an SDK generator for the schema and HTTP patterns supported by its
parser and templates, not a complete implementation of every OpenAPI feature.

## Project orchestration

Perseid also synchronizes specs, generates SDKs across one or several repositories,
opens coordinated GitHub update PRs, and prepares/publishes versioned releases.
A root `perseid.toml` supplies the spec source and targets; language presets avoid
repeating template tasks. SDK repositories own independent versions and local
`.perseid/overrides.toml` customizations. GitHub is optional for local generation.

```sh
perseid init --name example --language rust --language typescript --spec openapi.json
perseid sync
perseid generate --check
perseid generate --pr --dry-run
```

See the [orchestration guide](docs/orchestration.md) and
[small configuration example](examples/perseid.toml). The existing interfaces
below remain supported, including all template/runtime extension points.

## Use from an SDK repository

Run the native CLI against an SDK repository's existing configuration:

```sh
perseid sdk --config /path/to/sdk/codegen/codegen.toml
perseid sdk --config /path/to/sdk/codegen/codegen.toml --language rust --check
```

By default it generates every configured language. Repeat `--language` to
select a subset. An SDK may provide a thin `regen_openapi.py` wrapper around
these commands; the generator does not require a particular checkout layout.

The config belongs at `<sdk-root>/codegen/codegen.toml`. Paths in the config
are relative to the SDK root, except `task.template`, which is relative to
Perseid's template directory. Commands run from the SDK root with `PERSEID_DIR` set.
Only explicitly configured languages run. Standard formatters run automatically
from `PATH`; the Docker image bundles all five. `format_commands` replaces the
default adapter when supplied, and `--no-format` skips formatting;
`--local` remains accepted for compatibility and is the default behavior.

The CLI and configured runner are native Rust. A prebuilt binary needs no Python,
Rust compiler, Docker daemon, or source checkout for local generation: all shared
templates/runtimes and optional helper scripts are embedded. Building Perseid
requires Rust 1.88+; SDK checks need their own language toolchains.

`generate.py` and `project.py` remain optional compatibility wrappers. They use
`PERSEID_BIN` or build the native binary with Cargo; they contain no generation or
orchestration implementation. `PERSEID_DIR` can select an external asset directory
for development; its contents participate in the input fingerprint. Configured
Python formatting hooks still need Python, just like other consumer-selected tools.

A Rust configuration looks like this (the SDK package and its dependencies
must already exist):

```toml
[global]
perseid_version = "0.1.0"
input_files = ["spec/openapi.json"]
# version_file = ".version" # overrides sdk.version when set

[global.sdk]
client_name = "Example"
package_name = "example"
rust_crate = "example_rs"
java_package = "com.example"
default_base_url = "https://api.example.com"
user_agent_prefix = "example"
header_prefix = "example"
version = "0.1.0"
patch_nullable = false

[rust]
runtime_output_dir = "rust/src"
format_commands = ["cargo fmt --manifest-path rust/Cargo.toml"]
check_commands = ["cargo test --manifest-path rust/Cargo.toml"]

[[rust.task]]
template = "rust/api_resource.rs.jinja"
output_dir = "rust/src/api"
[[rust.task]]
template = "rust/api_summary.rs.jinja"
output_dir = "rust/src/api"
[[rust.task]]
template = "rust/component_type.rs.jinja"
output_dir = "rust/src/models"
[[rust.task]]
template = "rust/component_type_summary.rs.jinja"
output_dir = "rust/src/models"
```

`package_name` is a language package identifier (e.g. Go/Python), `rust_crate`
is the Rust import name, and `java_package` is the Java namespace. Per-language
`[typescript.sdk]`, `[go.sdk]`, etc. override the shared settings and can add
custom template context. TypeScript's `extra_exports = ["webhook"]` adds
consumer-owned modules to its entry point. `patch_nullable` enables the
inherited Rust/Java convention that treats optional properties on `Patch*`
models as unset-or-null; consumers must choose it explicitly.

## Shared files and consumer-owned code

| Layer | Perseid owns | SDK repository owns |
| --- | --- | --- |
| API | Resource methods, models, client/accessor templates | Spec and configuration |
| HTTP/runtime | Request execution, retries, transport and serialization helpers | API-specific error decoding |
| Packaging | Runtime output manifest | Dependency manifests, package roots, release versions |
| Extensions | Override loading and extra-method hooks | Custom methods, webhooks, tests |

The runtime sources are in `runtime/`; `runtime/manifest.json` lists exactly
which files each language installs. Rust shares `request.rs`, `connector.rs`,
`configuration.rs`, `upload.rs`, and `event_stream.rs`; Java shares its HTTP client/options and utility types;
TypeScript shares requests and datetime handling; Python shares API common code
and serialization; Go shares requests, client configuration and utility types.
Client entry points that enumerate API resources remain Jinja templates.

Runtime templates substitute explicit `@@SETTING@@` tokens using the SDK
context. Other braces are left intact. The generated files retain the same
module/class layout as the existing SDKs: there is no new runtime dependency
for SDK users. Runtime updates arrive when consumers regenerate with a newer
Perseid version. Generated sources must be committed with the SDK.

The initial runtime contracts preserve the existing SDK error adapters:

| Language | Consumer-provided integration |
| --- | --- |
| Rust | `crate::error::Error` with `generic`, async `from_response`, and `status()` for retry decisions; root exports `Configuration` and declares runtime modules |
| TypeScript | `util.ts` exports `ApiException(status, body, headers)` and `XOR`; local errors may decode generated models |
| Python | `errors.py` exports `ApiException.from_response`, `NetworkException`, `ResponseDecodeError` |
| Java | `<java_package>.exceptions.ApiException` and `Version.VERSION` |
| Go | `newAPIError`, `TransportError`, `DecodeError` in the SDK package |

`perseid init` supplies generic adapters and package manifests for these contracts.
Existing SDKs can retain their own adapters or override the runtime files.
Scaffolded support files belong to the SDK and are not overwritten by generation.
The supplied clients implement bearer-token authentication; configure package
identity, registry credentials and publishing hooks before releasing.

## Extension points

Prefer SDK settings for names, URLs, namespaces, nullable policy, versions and
extra exports. For behavior beyond those settings:

1. **Template overrides:** set `global.template_overrides = "codegen/templates"`.
   Mirror just the paths you need, e.g. `codegen/templates/rust/api_summary.rs.jinja`.
   Perseid overlays them onto a temporary copy of all shared templates, so nested
   includes fall back to the shared versions. Overrides receive `sdk` alongside
   the existing API/resource/type context and template filters.
2. **Runtime overrides:** map runtime source names to local files:

   ```toml
   [rust.runtime_overrides]
   "request.rs" = "codegen/runtime/request.rs"
   ```

   The local source uses the same `@@SETTING@@` tokens and must include an
   `@generated` marker in its first three lines. Unknown runtime keys fail.
   Omit `runtime_output_dir` to keep the entire runtime consumer-owned instead.
3. **Extra methods:** Java optionally includes
   `java/extensions/<resource>.java` and
   `java/extensions/<resource>_<operation>.java` from the template overlay;
   TypeScript uses equivalent paths under `typescript/api_extra/` with `.ts`.
   Names use snake case. These snippets are rendered inside the generated
   resource class. The two inherited Java snippets remain under `java/api_extra/`
   but are not automatically injected into unrelated APIs. An extension can
   explicitly `{% include "api_extra/message.java" %}` to reuse one.
4. **Task selection:** `[[language.task]]` chooses templates and output directories.
   `extra_codegen_args` on a language or task forwards CLI options such as
   `--exclude-op-id`, `--include-mode`, and `--include-op-id`.
5. **Repository hooks:** `format_commands` and `check_commands` are shell commands
   from the trusted SDK config. Checks run only when `--check` is supplied.

Prefer a small override or a handwritten wrapper over editing generated files.
Generic improvements belong in Perseid; domain-specific helpers belong in the SDK.

## Spec preparation and coverage

The optional `[global.prepare]` adapter accepts `exclude_unsupported` and
`normalize_tags` (both default false). Preparation accepts one spec per config;
without preparation, multiple inputs can be merged by the generator. It creates
temporary generator input,
names inline schemas, normalizes some schema composition, and adapts the input
to the inherited 3.1 parser. It never rewrites the checked-in source spec. This
is a generator-specific adapter, not a general standards-preserving OpenAPI
version converter; OpenAPI 3.2 support is not claimed.

The Rust target supports multipart uploads, raw-binary uploads, and SSE responses.
Other targets reject specs containing these transports with an explicit error;
they retain their existing JSON, form, and buffered-response behavior. Preparation
preserves the supported transports and writes coverage to `codegen/coverage.json`.
`exclude_unsupported = true` still permits omission of unsupported media types or
operations without a documented success response.

Scalar/array/map alias rendering is currently implemented in Rust; other
languages reject unsupported alias kinds. Schema support varies by language,
so compile and test generated SDKs against their actual specs.

The runner stages all rendering before replacing SDK files. It tracks ownership
per language in `codegen/generated_files.json`, preserves other languages during
partial regeneration, and deletes only stale tracked files with an `@generated`
marker. `scripts/clean_generated.py <sdk-root>` removes those tracked outputs.
Rendering failures leave previous SDK output intact. The legacy `sdk` command runs
format/check hooks in the existing workspace after rendering, preserving installed
dependencies and Git context. Its hook failures leave generated changes available
for inspection. Project `generate --check` stages generation and hooks together
before applying any destination changes.

## Rust uploads and event streams

Raw `application/octet-stream` methods take `api::Upload`. Choose
`Upload::bytes(bytes)` for a replayable buffer or
`Upload::reader(reader, Some(length))` for a single-use Tokio `AsyncRead` reader.
Passing `None` as the length enables streaming without a known content length.
Reader bodies use bounded chunks and validate declared lengths while reading.
Configure the client timeout for the expected upload duration.

Multipart operations generate a `<Resource><Operation>Body` struct in `api`.
Its binary fields take `Upload`; ordinary fields retain their schema types.
Optional fields set to `None` are omitted. File parts support
`.with_filename("data.csv").with_content_type("text/csv")`. Referenced binary
schemas are supported. Multipart arrays and custom OpenAPI `encoding` entries
are rejected explicitly in this first implementation.

Retry policy depends on the body: buffers can be replayed, keeping the same
idempotency key and multipart boundary. A request containing any reader is
never automatically retried, including after a transport failure or timeout.
Reopen the source and explicitly issue a new request when retrying such an upload.

SSE operations return `api::EventStream`; consume it with
`while let Some(event) = stream.next().await { ... }`. Each `SseEvent` exposes
`event`, `data`, `id`, and the server's `retry` hint. Decoding handles UTF-8 split
across chunks, LF/CRLF/CR line endings, multiline data and comments, following the
[SSE event format](https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation).

The client timeout covers opening an SSE response, not the whole stream.
Cancelling `next()` preserves partially decoded events; drop the stream to
release the response, or wrap `next()` in a caller-chosen idle timeout.
Each buffered event/line is limited to 1 MiB by default; change it with
`with_max_event_bytes`. EOF discards incomplete events, and a parsing or transport
error yields one error before terminating the stream. The SDK does not reconnect
a live stream or automatically apply `Last-Event-ID`; callers control resumption
using the API's declared request parameters and their own deduplication policy.

The generated Rust API module declares the new shared runtime modules and
reexports `Upload`, `EventStream` and `SseEvent`. No new SDK dependencies are
required beyond the existing Hyper/Tokio stack. Consumers overriding the Rust
API summary, request runtime, or configuration runtime should update those
overrides together.

## Tooling and checks

Standard formatting runs natively using rustfmt, google-java-format, Biome, Ruff,
and gofmt on `PATH`. The Docker image bundles pinned versions of all five; only
selected languages require a formatter. Adapters touch only generated files.
Use `format_commands` for an override or `--no-format` to skip formatting.
SDK compilation/tests remain configured through `check_commands`.

The older Python helpers remain available for existing configurations; using those
explicit hooks still requires Python. See the [orchestration guide](docs/orchestration.md)
for tool versions, Docker usage, init, and destination-owned overrides.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 -m unittest discover -s tests -v
```

Tests cover schema regressions, all five template families, runtime substitution,
overrides, output paths, failed-render preservation, and partial regeneration
using synthetic APIs. A generated Rust fixture is compiled and tested against
local HTTP servers for streaming uploads, multipart encoding, retries, SSE parsing,
cancellation and timeouts. Its dependencies are locked in
`tests/fixtures/rust-sdk/Cargo.lock`. `PERSEID_TEST_TARGET` can override the fixture's
Cargo cache directory. SDK repositories should also run their own compiler and
transport tests through `check_commands`.

The Rust binary is also usable directly (`cargo run -- --help`) for generation
or IR debugging. `Dockerfile` retains the inherited binary/formatter image and
embeds the shared assets in the binary. Generation runs in-process; configured hooks
use local tools;
container-based SDK checks need the SDK dependencies and language toolchains.
The Docker image includes the `project` commands and uses `perseid` as its entry
point; use `--entrypoint` for shell commands.

For consumer CI, check out the same Perseid revision used locally and verify
the configured `perseid_version` matches. Upgrade that pin and regenerate when
adopting a new release.

## Initial migration compatibility

Rust models now honor OpenAPI `int64`/`uint64` as `i64`/`u64`, fixing the
inherited narrowing to 32 bits. Migrating SDKs may therefore change public
field types; callers using explicitly typed 32-bit values may need conversions. The
inherited Java convenience snippets also require explicit inclusion as described
above.

## Attribution

The generator and inherited templates originate from
[Svix's svix-webhooks repository](https://github.com/svix/svix-webhooks/tree/main/codegen)
and the [Meteroid fork](https://github.com/meteroid-oss/meteroid-clients).
The existing project license remains Apache-2.0 (`LICENSE`). Imported code
retains its MIT notices in `LICENSE-Svix` and `LICENSE-Meteroid`; see `NOTICE`.
