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
format_commands = ["cargo fmt"]
check_commands = ["cargo test --locked"]

[targets.typescript]
format_commands = ["npm ci", "npm run format"]
check_commands = ["npm test"]
```

This generates into `rust/` and `typescript/`. Target keys name languages by
default; explicit `language = "rust"` allows a name such as `public-rust`.
Names, namespace, user-agent, runtime locations, and template tasks have defaults.
All five languages have presets. The corresponding package skeletons,
dependencies, error adapters, and handwritten extensions must already exist;
orchestration does not scaffold a new publishable package.

Run through the compiled CLI:

```sh
perseid sync
perseid plan
perseid generate --check
perseid check
perseid generate --pr --dry-run
perseid generate --pr
```

Every command accepts `--config path/to/perseid.toml`; commands other than `sync`
accept repeated `--target` filters. `generate` operates locally by default;
`generate --pr` generates, runs checks, and creates or updates GitHub PRs.
`--dry-run` requires `--pr` and previews the delivery without external writes.

The `perseid project ...` command namespace remains a compatibility alias. Its
old `generate --pr` command aliases `generate --pr`. Existing `generate --template ...`
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

## Versioning and releases

API `info.version`, Perseid's own version, and SDK package versions are separate.
Versions start at `0.1.0`; configure `[release].initial_version` to migrate an
existing SDK. The default policy is `lockstep`. Set `policy = "independent"` to
bump targets independently.

```sh
perseid version minor --notes changes.md
perseid generate --check
perseid generate --pr
# After the control metadata and SDK PRs have merged:
perseid release --dry-run
perseid release
```

`version` accepts `major`, `minor`, `patch`, or an explicit stable `X.Y.Z` greater
than the current version. It writes `perseid.versions.json` with versions and
reviewable release notes. Generation applies those versions to SDK context and
package metadata. Lockstep requires every target when preparing versions;
independent mode allows `--target`. Automatic semantic version suggestions and
prerelease versions are deliberately not part of this first implementation.

Default stamping supports Rust's literal `[package].version`, Python's PEP 621
`[project].version`, and TypeScript's `package.json`. Go uses release tags.
Java requires `version_commands` because Maven/Gradle layouts differ. For custom
metadata, workspace-inherited versions, or package lockfiles, `version_commands`
replaces the default stamping and runs with `PERSEID_VERSION` set:

```toml
[targets.typescript.release]
version_commands = ["npm version --no-git-tag-version --allow-same-version \"$PERSEID_VERSION\""]
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
`{target}/v{version}`); tags in one repository must be distinct. Go module users
should configure `tag = "v{version}"` for a root module or the appropriate
subdirectory-prefixed tag.

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

The Docker image includes Git and `gh`, and no Python interpreter, with `perseid` as its entry point:

```sh
docker run --rm -v "$PWD:/workspace" -w /workspace perseid plan
```

It is a generation/orchestration image, not an all-language build environment.
Provide SDK build toolchains in a derived image or run the CLI on a CI runner.
Use `--entrypoint` to run a different command in the image.

## CLI correspondence with Fern

Fern's `generate` command combines generation and configured delivery;
`github.mode: pull-request` selects PR delivery. Perseid uses an explicit
`generate --pr` flag for that same action, with local output as the default.
There is no separate new workflow to learn behind “propose.” See Fern's
[configuration reference](https://buildwithfern.com/learn/sdks/reference/generators-yml)
and [SDK commands](https://buildwithfern.com/learn/cli-api-reference/cli-reference/sdk-commands).
Spec synchronization and package publication remain explicit commands so a local
regeneration does not implicitly refresh upstream inputs or publish a package.
