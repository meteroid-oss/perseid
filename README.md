# Perseid

Perseid generates SDKs from OpenAPI for **Rust, Java, TypeScript, Python, and Go**.
It brings the generator, all 49 inherited template files, shared HTTP/runtime
sources, and regeneration scripts into one versioned project. SDK repositories
supply their own specs and configuration. Version 0.1.0 is the initial extraction.

This is an SDK generator for the schema and HTTP patterns supported by its
parser and templates, not a complete implementation of every OpenAPI feature.

## Use from an SDK repository

Check out Perseid and run it against an SDK repository's configuration:

```sh
python3 /path/to/perseid/generate.py --config /path/to/sdk/codegen/codegen.toml
python3 /path/to/perseid/generate.py --config /path/to/sdk/codegen/codegen.toml --language rust --check
```

By default it generates every configured language. Repeat `--language` to
select a subset. An SDK may provide a thin `regen_openapi.py` wrapper around
these commands; the generator does not require a particular checkout layout.

The config belongs at `<sdk-root>/codegen/codegen.toml`. Paths in the config
are relative to the SDK root, except `task.template`, which is relative to
Perseid's template directory. Commands run from the SDK root with `PERSEID_DIR` set.
Only explicitly configured languages run. `--no-format` skips formatters;
`--local` remains accepted for compatibility and is the default behavior.

Requires Python 3.11+, Rust 1.88+, and the SDK's formatting/build toolchains.
The runner builds Perseid with `cargo build --locked`. `PERSEID_BIN` may point to a
prebuilt binary, but the matching Perseid checkout is still needed for templates,
runtime files and scripts. Both the binary and the SDK configuration must
match the assets' package version.

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
and `configuration.rs`; Java shares its HTTP client/options and utility types;
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

An SDK must supply those adapters and the runtime's language dependencies,
or override the runtime files. Perseid does not yet scaffold
a complete publishable package or provide universal authentication adapters;
the supplied clients implement bearer-token authentication.

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

Multipart uploads, raw-binary uploads and SSE responses are not implemented.
With `exclude_unsupported = true`, preparation omits these operations and writes
coverage and reasons to `codegen/coverage.json`. Without this opt-in it rejects
them. Changing the spec version alone does not add transport support.

Scalar/array/map alias rendering is currently implemented in Rust; other
languages reject unsupported alias kinds. Schema support varies by language,
so compile and test generated SDKs against their actual specs.

The runner stages all rendering before replacing SDK files. It tracks ownership
per language in `codegen/generated_files.json`, preserves other languages during
partial regeneration, and deletes only stale tracked files with an `@generated`
marker. `scripts/clean_generated.py <sdk-root>` removes those tracked outputs.
Rendering failures leave previous SDK output intact; formatter/check failures
leave the generated output available for inspection.

## Tooling and checks

The consumers choose their own format/check commands. Shared helpers provide
pinned google-java-format 1.25.2 (checksum verified, Java 21) and Ruff 0.14.10
(via `uvx`, or an installed matching version). Java formatting reads the output
manifest and touches only generated files. `PERSEID_JAVA_FORMAT_JAR` can select an
already downloaded formatter; its checksum is still checked. TypeScript uses
the SDK's locked Biome dependency via `scripts/format_typescript.py <package-dir>`;
its import cleanup and formatting touch only generated files. Rust and Go use
rustfmt and gofmt.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 -m unittest discover -s tests -v
```

Tests cover schema regressions, all five template families, runtime substitution,
overrides, output paths, failed-render preservation, and partial regeneration
using a synthetic API. SDK repositories should run their own compiler and
transport tests through `check_commands`.

The Rust binary is also usable directly (`cargo run -- --help`) for generation
or IR debugging. `Dockerfile` retains the inherited binary/formatter image and
ships the shared assets under `/opt/perseid`. The normal runner uses local tools;
container-based SDK checks need the SDK dependencies and language toolchains.
The Docker build has not been validated as part of this extraction.

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
