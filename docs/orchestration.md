# Project orchestration

Perseid can manage the path from an exported OpenAPI spec to reviewed SDK updates
and published packages. Generation and orchestration ship together; GitHub is an
optional destination, with no Perseid account or service dependency.

The orchestration module uses Python 3.11's standard library. Git is needed for
Git sources and external destinations. GitHub delivery also requires `gh` with
appropriate repository access. SDK builds still need their language toolchains.

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
perseid project sync
perseid project plan
perseid project generate --check
perseid project check
perseid project propose --dry-run
perseid project propose
```

Alternatively, run `python3 /path/to/perseid/project.py <command>`. The Rust CLI
locates assets through `PERSEID_DIR` (default: its build checkout). Every command
accepts `--config path/to/perseid.toml`; commands other than `sync` accept repeated
`--target` filters. The existing low-level `perseid generate --template ...` and
`generate.py --config codegen/codegen.toml` interfaces remain available.

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

The checksum verifies the installed source/assets, not the provenance of a custom
prebuilt binary. Supply a matching binary or let the Python runner build it.
Pin the Perseid checkout/image by commit or immutable image digest as well as the
package version. Exporter dependencies and SDK toolchain versions remain your CI's
responsibility; the lock does not make arbitrary hooks hermetic.

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
runs checks before applying changes; `project check` regenerates and runs checks
without applying anything, exiting nonzero on drift. Rendering receipts and
ownership manifests are kept separately per target under `.perseid/`.

Filesystem application is not a cross-repository transaction: an OS failure
midway through copying can still leave partial changes, which a rerun repairs.
Workspaces containing symlinks outside ignored dependency/build directories are
currently rejected. Standard dependency/build directories are omitted from
staging and drift comparison; do not place SDK sources inside `build`, `dist`,
`target`, or the other ignored cache directories listed in `orchestration/common.py`.

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

`propose` uses fresh clones of each destination's base branch and runs all
configured checks before the first push. It creates or updates one PR per repo
on `perseid/<project-name>/update`, using an explicit force-with-lease to reject
concurrent changes. Repeated identical runs reuse the existing PR; an obsolete PR is closed when
the current desired output already matches the base branch. A commit
without Perseid's marker at the tip of the reserved branch blocks replacement.
Reserve that branch for automation and put manual changes in normal branches.

Selecting one target for `propose` includes all targets in that repository, so
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
perseid project version minor --notes changes.md
perseid project generate --check
perseid project propose
# After the control metadata and SDK PRs have merged:
perseid project release --dry-run
perseid project release
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
the Perseid ref before enabling it. Local `project check` is also suitable for an
ordinary pull-request CI job.

The Docker image includes Python, Git, and `gh`, with `perseid` as its entry point:

```sh
docker run --rm -v "$PWD:/workspace" -w /workspace perseid project plan
```

It is a generation/orchestration image, not an all-language build environment.
Provide SDK build toolchains in a derived image or run the CLI on a CI runner.
Use `--entrypoint` to run a different command in the image.
