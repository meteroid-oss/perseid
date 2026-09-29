# Project orchestration

Perseid can manage the path from an exported OpenAPI spec to reviewed SDK updates
and published packages. Generation and orchestration ship together; GitHub is an
optional destination, with no Perseid account or service dependency.

The orchestrator and configured generation runner are Rust modules in the same
binary. Templates and runtimes are embedded. Local file-source generation needs
neither Python nor a source checkout. Git is needed for Git sources and external
destinations; GitHub delivery also requires `gh` with repository access. SDK
builds and configured hooks still need their language toolchains.

## A small configuration

Put `perseid.toml` at the root of your clients/configuration repository:

```toml
name = "example"
perseid_version = "0.1.0"

[source]
file = "../backend/apis/generated/openapi.json"

[sdk]
default_base_url = "https://api.example.com"

[targets.rust]
check_commands = ["cargo test --locked"]

[targets.typescript]
check_commands = ["npm ci --ignore-scripts", "npm test"]
```

This generates into `rust/` and `typescript/`. Target keys name languages by
default; explicit `language = "rust"` allows a name such as `public-rust`.
Names, namespace, user-agent, runtime locations, and template tasks have defaults.
All five languages have presets. Use `perseid init` to create package manifests
and generic error adapters, or point generation at an existing SDK. Generation
preserves SDK-owned support files.

Run through the compiled CLI:

```sh
perseid sync
perseid plan
perseid generate --check
perseid check
perseid generate --pr --dry-run
perseid generate --pr
```

Lifecycle commands accept `--config path/to/perseid.toml`; commands other than `sync`
accept repeated `--target` filters. `generate` operates locally by default;
`generate --pr` generates, runs checks, and creates or updates GitHub PRs.
`--dry-run` requires `--pr` and previews the delivery without external writes.

The `perseid project ...` command namespace remains a compatibility alias. Its
hidden `propose` alias also invokes `generate --pr`. Existing `generate --template ...`
and `perseid sdk --config codegen/codegen.toml` interfaces remain available.
`generate.py` and `project.py` are optional Python wrappers around the native CLI.

Commit `perseid.toml`, `perseid.lock.json`, the spec snapshot (default
`spec/openapi.json`), generated SDK files, and `.perseid/`. Ignore
`.perseid-work/`, which contains disposable external-repository checkouts.

## Sources and locked inputs

Configure exactly one source:

```toml
# File, including an export in a sibling checkout:
[source]
file = "../backend/openapi.json"

# Or a file at a Git ref (branch, tag, or commit):
[source]
git = "acme/backend"
ref = "main"
path = "apis/generated/openapi.json"

# Or HTTP(S):
[source]
url = "https://api.example.com/openapi.json"

# Or an argument array, run at the configuration root, writing JSON to stdout:
[source]
command = ["cargo", "run", "--bin", "export-openapi"]
```

`source.snapshot` changes the destination of the committed snapshot. Sources must
produce OpenAPI 3 JSON, at most 64 MiB. Export diagnostics should go to stderr.
Private Git sources use normal Git credentials. HTTP authentication headers and
YAML inputs are not implemented.

Only `sync` resolves the source. It preserves the exported bytes and records the
resolved Git commit when applicable, spec checksum, normalized configuration
checksum, and checksum of Perseid's implementation/templates/runtime assets.
Repeated syncs with identical inputs do not rewrite files or add timestamps.
`plan` reads the lock without fetching or running an exporter. Generation rejects
stale or modified inputs rather than silently fetching a newer spec.

The engine fingerprint is baked into the compiled binary from its Rust source,
Cargo manifests, and embedded assets. A `PERSEID_DIR` development override adds the
actual external asset contents to that fingerprint. Destination template/runtime
override contents are separately hashed in each target receipt. Changing an
override changes provenance even when the source spec lock is unchanged.

The native lock uses schema version 2. Migrating a Python-created lock requires
one `perseid sync` and regeneration; old locks are rejected with instructions.
Pin the Perseid binary/image by commit or immutable image digest as well as its
package version. Exporter dependencies and SDK toolchain versions remain your
CI's responsibility; arbitrary hooks are not made hermetic by the lock.

## Layout and extension points

| Language | Default target directory | Generated source layout within it |
| --- | --- | --- |
| Rust | `rust` | `src/api`, `src/models`, runtime in `src` |
| TypeScript | `typescript` | `src/api`, `src/models`, runtime/client in `src` |
| Python | `python` | `<package_name>/api`, `<package_name>/models`, runtime in package |
| Java | `java` | `src/main/java/<java_package>` with `api` and `models` |
| Go | `go` | API, model, and runtime files at package root |

Global `[sdk]` and per-target `[targets.NAME.sdk]` accept the existing generator's
SDK settings. Package version comes from the version metadata, not `sdk.version`.
`[targets.NAME.prepare]` accepts the existing optional spec preparation settings.

Each target can override `directory`, `runtime_output_dir`, `runtime_overrides`,
`template_overrides`, `extra_codegen_args`, or `[[targets.NAME.task]]`. These map
to the existing generator extension points; no second template system is added.
Task output paths and override file paths are relative to the **destination
repository**, while `task.template` remains relative to Perseid's templates.
All generated outputs for a target must stay inside its `directory`, and target
directories in the same repository cannot overlap. Override files live in the
destination repository and are reviewed with its code.

`format_commands` and `check_commands` run in the **target directory**, under
`bash -e -o pipefail`. `PERSEID_DIR` points to shared assets.
`PERSEID_REPOSITORY_ROOT` and `PERSEID_GENERATED_MANIFEST` identify the staged
repository and current target manifest; the shared Java/TypeScript formatting
scripts accept these automatically. They are trusted
configuration and may execute arbitrary commands. Checks should install the
dependencies they need: staging does not copy dependency/build caches.

`generate` stages every destination before applying changes. Failed rendering,
formatting, or requested checks leave the SDK files untouched. `generate --check`
runs checks before applying changes; `check` regenerates and runs checks
without applying anything, exiting nonzero on drift. Rendering receipts and
ownership manifests are kept separately per target under `.perseid/`.

Filesystem application is not a cross-repository transaction: an OS failure
midway through copying can still leave partial changes, which a rerun repairs.
Local staging rejects symlinks outside ignored dependency/build directories.
Fresh delivery checkouts reject all symlinks before any generation or control-file
write, including dry-runs. Repository-relative target paths cannot contain `..`;
normalized target directories must not overlap. Standard dependency/build directories are omitted from
staging and drift comparison; do not place SDK sources inside `build`, `dist`,
`target`, or the other ignored cache directories listed in `src/project/io.rs`.

## Multiple repositories and GitHub PRs

A target only needs two additional settings to live elsewhere:

```toml
[targets.rust]
repository = "acme/example-rust"
directory = "."
# branch = "main"  # default: remote's default branch
check_commands = ["cargo test --locked"]
```

Local generation also accepts Git URLs and existing local Git repository paths.
External workspaces are cloned into `.perseid-work/` and reused without resetting
local changes. They do not automatically pull newer commits: remove a disposable
workspace to get a fresh clone, after preserving any edits you want to keep.

`generate --pr` uses fresh clones of each destination's base branch and runs all
configured checks before the first push. It creates or updates one PR per repo
on `perseid/<project-name>/update`, using an explicit force-with-lease to reject
concurrent changes. Repeated identical runs reuse the existing PR; an obsolete PR is closed when
the current desired output already matches the base branch. A commit
without Perseid's marker at the tip of the reserved branch blocks replacement.
Reserve that branch for automation and put manual changes in normal branches.

Selecting one target for `generate --pr` includes all targets in that repository, so
updating the stable PR cannot discard a sibling target's pending changes. The
controller repository also gets a PR when its snapshot/lock/version metadata has
changed, even if every SDK target lives elsewhere. Unrelated local working-tree
changes are not copied into delivery clones.

`--dry-run` generates and checks in temporary clones, showing paths and proposed
branches without pushing or calling GitHub write APIs. PR bodies include the spec
checksum and target names; the code diff is the authoritative SDK change report.
GitHub delivery currently supports github.com; other Git hosts can still use the
local generation commands.

Authentication is supplied by Git and `gh`; Perseid stores no credentials. With
GitHub Actions, use `GH_TOKEN` and configure Git authentication (for example,
`gh auth setup-git`). Cross-repository updates need a GitHub App token or PAT with
access to the destinations; a repository's default token is insufficient for
other private repositories. Use Actions concurrency to serialize updates for a
project. Multiple repositories cannot be updated atomically: rerun after a
partial failure to reconcile them.

## Bootstrapping and destination overrides

Create one or several SDK packages with their runtime dependencies and customizable
support files (entry points/errors). Init refuses file conflicts before writing:

```sh
perseid init --name acme --language rust --language typescript --spec openapi.json
perseid sync
perseid generate
perseid generate --check
```

Use `--output PATH` for another repository. A dedicated destination managed by
another controller can use:

```sh
perseid init --name acme --language go --directory . --sdk-only \
  --go-module github.com/acme/go-sdk
```

`--sdk-only` omits `perseid.toml`; the controller supplies source/repository settings.
Go requires the intended import path explicitly. Java scaffolding uses Gradle
without downloading a wrapper; install Gradle or add your usual wrapper. The
TypeScript check installs dependencies in staging and creates `package-lock.json`
on its first run, then uses `npm ci` on later runs. Commit that lockfile. Python's
starter check verifies syntax; extend it with your SDK tests. Checks require the
language toolchains; init and plain generation do not install build dependencies.

Every destination can own a discovered `.perseid/overrides.toml`:

```toml
[targets.rust]
template_overrides = ".perseid/templates"
check_commands = ["cargo test --locked"]

[targets.rust.runtime_overrides]
"request.rs" = ".perseid/runtime/request.rs"

[targets.rust.sdk]
client_name = "Acme"
```

Use controller target names as keys (the default names are the languages).
A template at `.perseid/templates/rust/api_resource.rs.jinja` replaces that
shared template; absent files fall back to embedded defaults. SDK settings and
runtime mappings merge with controller values; command lists and template roots
replace them. Local overrides can also supply `read_version` / `version_commands`.
Source, destination paths/repositories, release policy, and publication commands
stay in the controller. Override contents are hashed into generation receipts;
changes are reviewed in the destination repository and never overwritten by init
or regeneration. All paths remain repository-relative and reject symlinks/traversal.

## Versioning and releases

API `info.version`, Perseid's own version, and SDK package versions are separate.
**Independent versioning is the default. Each SDK repository owns its version.**
Rust reads `Cargo.toml`, Python reads PEP 621 `pyproject.toml`, and TypeScript reads
`package.json`. Go reads stable release tags matching its configured tag pattern.
Existing Java/custom layouts supply a `read_version` command; `init` configures
Java's `version.txt` automatically. A package with no manifest or release tag starts
at `[release].initial_version` (default `0.1.0`).

```sh
perseid version minor --target rust --notes changes.md
perseid version 2.3.0 --target typescript --notes changes.md
perseid generate --pr
# After the controller and SDK PRs have merged:
perseid release --dry-run
perseid release
```

`version` accepts `major`, `minor`, `patch`, or an explicit stable `X.Y.Z`. For
independent targets it records a **bump request and notes**, not a current version,
in `perseid.releases.json`. Generation resolves the request against the destination
checkout and stores the resolved proposal in that SDK's `.perseid/releases/NAME.json`.
The ID and destination request history prevent the same request from bumping again
after regeneration or merge; rerunning a superseded generation request fails
clearly. Historical release requests can still resume their existing immutable tag.
A new `version` invocation creates a new request. Explicit versions must exceed
the destination's version when first resolved.

You can edit a proposed package version and run generation again before merging;
Go proposals can be edited in `.perseid/releases/NAME.json`. Commit the resulting
metadata and generated changes together. Release checks reject inconsistent
metadata. A manual SDK release outside Perseid needs **no controller version
update**: the next fresh destination checkout supplies the new manifest/tag.
`generate --pr` always uses fresh clones; local external generation reuses its
`.perseid-work` checkout, which can be updated with ordinary Git commands.

For intentionally synchronized SDKs, set `[release] policy = "lockstep"`.
This opt-in mode retains the shared `perseid.versions.json` and requires all
targets when preparing a version. Automatic compatibility analysis, semantic
bump recommendations, and prerelease versions are not implemented. Bump requests
are explicit, including the desired major/minor policy before `1.0`.

Default stamping supports literal Cargo/PEP 621 versions and `package.json`,
including the SDK entry in a local `Cargo.lock` and npm lockfiles. Dependency
versions are preserved. Java/custom metadata and other workspace/lockfile layouts
can use `version_commands` instead.
Hooks run in the target directory; stamping receives `PERSEID_VERSION`.
For example, a destination's `.perseid/overrides.toml` can contain:

```toml
[targets.typescript]
version_commands = ["npm version --no-git-tag-version --allow-same-version \"$PERSEID_VERSION\""]

[targets.java]
read_version = "cat version.txt"
version_commands = ["printf '%s\\n' \"$PERSEID_VERSION\" > version.txt"]
```

Publish policy remains in the controller:

```toml
[targets.typescript.release]
publish = "npm publish"
is_published = "./scripts/is-published.sh"
tag = "typescript/v{version}"
```

Publication uses two explicit hooks instead of embedding registry SDKs:

- `is_published`: exit **0** if this package/version exists, **1** if it does not,
  and **2 or greater** on an authentication, network, or other probe failure.
- `publish`: publish the version, exiting nonzero on failure. The probe is run
  again after publication and must confirm success.

Both run in the target directory with `PERSEID_VERSION` and `PERSEID_DIR` set.
Do not map every failed registry request to “not published.” Probe scripts must
distinguish a missing version from an unavailable registry.

`release` requires committed control inputs. It checks out destination code,
regenerates and runs checks, and rejects unmerged/drifting changes before doing
any publication. Each target gets an immutable tag (default
`{target}/v{version}`); tags in one repository must be distinct. Go defaults to
`v{version}` for a root module and `DIRECTORY/v{version}` for a subdirectory.
An override is available through `release.tag`. Go major versions 2+ require the
matching `/vMAJOR` module path in `go.mod`; update it before generating the major
release. A new release must exceed existing matching stable tags; an existing
tag can only be resumed at its original commit.

The tag is pushed before registry publication. If publication or GitHub release
creation fails, rerunning uses the tagged commit and registry probe to resume,
including when the base branch has advanced. Tags are never moved. All selected
targets are validated before the first publication; successful targets remain
published if a later target fails. GitHub Releases preserve the reviewed notes;
no hosted state store or local-only publication journal is needed.

`release --dry-run` runs generation and SDK checks, but does not run publication
hooks, push tags, or create GitHub releases. Pin the controller checkout to the
release metadata being retried. Configuration or spec changes belong in a new
update cycle.

## GitHub Actions and Docker

See [`examples/orchestration-workflow.yml`](../examples/orchestration-workflow.yml)
for a manual update workflow. It is stored outside `.github/workflows` so consumer
repositories can adapt it. Install the toolchains needed by your targets and pin
the Perseid ref before enabling it. Local `check` is also suitable for an
ordinary pull-request CI job.

The same native CLI can use formatters installed on `PATH` or bundled in Docker.
Only tools for selected languages are required. No Python formatter helper scripts
or per-target formatting hooks are necessary:

| Language | Executable on PATH | Bundled version |
| --- | --- | --- |
| Rust | `rustfmt` | nightly-2025-02-27 |
| Java | `google-java-format` | 1.25.2 (Java 21 runtime) |
| TypeScript | `biome` | 2.1.4 |
| Python | `ruff` | 0.14.10 |
| Go | `gofmt` | Go 1.24.7 |

Perseid formats only files in its generated ownership manifest. It does not
follow Rust modules into handwritten files. Rust defaults to edition 2021;
configure a custom command for another edition. Native users should pin the
same formatter versions for reproducible output. Missing tools produce an
installation hint; `--no-format` explicitly skips formatting. An explicit
`format_commands` list replaces the built-in adapter (`[]` disables formatting).
These defaults apply to both `generate` and legacy `sdk` commands.

```sh
# Native: install the selected formatters in an earlier CI step.
perseid sync
perseid generate

# Container: use a locally built or published, pinned image.
docker build -t perseid .
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/workspace" -w /workspace perseid sync
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/workspace" -w /workspace perseid generate
```

The image also includes Git and `gh`; the native binary embeds templates and
runtimes. It needs no Python interpreter, Node, or Cargo to generate and format.
The bundled image currently targets Linux x86_64. `--check` / `--pr` additionally
run configured SDK checks, which require their language build tools/dependencies.
Use a CI runner or derived image for those checks and package publication. Provide
GitHub credentials/configuration when using delivery commands in Docker.

## CLI correspondence with Fern

Fern's `generate` command combines generation and configured delivery;
`github.mode: pull-request` selects PR delivery. Perseid uses an explicit
`generate --pr` flag for that same action, with local output as the default.
There is no separate new workflow to learn behind “propose.” See Fern's
[configuration reference](https://buildwithfern.com/learn/sdks/reference/generators-yml)
and [SDK commands](https://buildwithfern.com/learn/cli-api-reference/cli-reference/sdk-commands).
Spec synchronization and package publication remain explicit commands so a local
regeneration does not implicitly refresh upstream inputs or publish a package.
