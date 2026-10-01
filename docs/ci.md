# CI and releases

## Repository layouts

`perseid.toml` lives in the repository that runs the generation. It lists the SDKs (`sdks`), where
they live (`repo`) and the spec they come from (`spec`). Three layouts:

**Next to the API.** Each SDK is a folder of the repository holding the spec. Pick this unless
another team owns the SDKs.

```
acme/api ──PRs──▶ acme/api (typescript/, python/)
```

```toml
spec = "openapi.json"
sdks = ["typescript", "python"]
```

**One repository per language.** Each ecosystem gets its own home. `{lang}` is the language as
`sdks` names it.

```
acme/api ──PRs──▶ acme/api-typescript, acme/api-python
```

```toml
sdks = ["typescript", "python"]
repo = "acme/api-{lang}"
```

A `repo` without `{lang}`, such as `"acme/api-sdks"`, holds every SDK in a folder named after its
language.

**A separate SDKs repository.** The API repository only pushes its spec there, with a deploy key
that can write to that one repository. The SDKs repository generates the SDKs, in its own folders
or in a repository per language, and gets no access to the API repository. Pick this when the API
repository must not grant anything else.

```
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-sdks (typescript/, python/)
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-typescript, acme/api-python
```

```sh
# in acme/api-sdks
npx perseid init                    # spec = "openapi.json", which connect pushes
git add -A && git commit -m "ci: generate SDKs with perseid" && git push
# in acme/api, once perseid.toml is on the default branch of acme/api-sdks
npx perseid connect acme/api-sdks   # writes perseid-push.yml, offers a deploy key
```

**A spec at a URL.** `spec = "https://api.acme.com/openapi.json"` needs nothing from the API side.
`sdks.yml` fetches it every day and on demand, and opens pull requests when it changed. Their
description names the URL and a digest of what it served.

perseid never creates repositories. Create the SDK repositories `repo` names with
`gh repo create`. `perseid init` and `perseid status` print the commands.

## Setup

You need:

- Node 18+ for `npx perseid`, or the [install script](../install.sh).
- Admin rights on the repository holding `perseid.toml`, to add its secret.
- For `perseid app` in an organization: owner or GitHub App manager.
- For `perseid connect`: admin rights on the API repository, and ideally on the SDKs repository.
- An account on each registry you publish to: npm, PyPI, crates.io, Maven Central, NuGet.

### `perseid init`

`init` works in your clone only. It finds the spec (a tracked `openapi` or `swagger` file, JSON or
YAML), asks which SDKs to generate, where they live and the client name. It then writes:

- `perseid.toml`;
- `.github/workflows/sdks.yml`, which regenerates the SDKs;
- for the SDKs kept in this repository, `sdk-release.yml` and the release-please files.

Commit and push them: workflows run from the default branch. SDKs in their own repositories get
their release files in their first pull request.

Run `init` again after editing `perseid.toml`. It rewrites the workflows it wrote, and leaves
alone a workflow whose first line, ``# Written by `perseid init` ``, you removed. Without a
terminal, pass `--sdks`, and optionally `--repo`, `--spec`, `--name` and `--base-url`.

Without a spec in the repository, `spec` defaults to `openapi.json`, where `perseid connect` will
push it. Meanwhile `perseid generate --spec <path|url>` previews the SDKs.

`perseid generate` starts each SDK from its package skeleton (manifest, README, errors) the first
time, and prints where each one went. An SDK with its own `repo` is generated into a shallow clone
under `.perseid/repos/<owner>/<name>`. Changes there that perseid didn't make stop `generate`,
unless `--pr` is passed. `generate --out <dir>` writes every SDK to `<dir>/<language>` without
cloning anything, and `--out <dir> --check` compares against that directory.

### Tokens

`sdks.yml` opens pull requests with one of:

- **The `PERSEID_TOKEN` secret.** A
  [fine-grained token](https://github.com/settings/personal-access-tokens/new) with Contents, Pull
  requests and Workflows set to read and write, on the repository holding `perseid.toml` and every
  SDK repository. Add it to each of them: `sdk-release.yml` uses it too.
- **A GitHub App**, set up by `perseid app`. App tokens are minted on each run and never stored,
  so nothing expires.

Workflows is needed because the first pull request in an SDK repository adds `sdk-release.yml`.

Without either, the Action fails with:

```
error: no token to open the SDK pull requests: add the PERSEID_TOKEN secret (a fine-grained token
with Contents, Pull requests and Workflows read and write on the SDK repositories), or run `perseid app`
```

When the token expires within 30 days, `generate --pr` warns on each run.

**`perseid app`** creates a GitHub App named `<name>-sdk-bot` in your browser, on the account that
owns the SDK repositories. It has Contents, Pull requests and Workflows write, and no webhook.
perseid then installs it on the repository holding `perseid.toml` and the SDK repositories, and
stores the `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret on each. It prints the plan and
asks before changing anything. If you decline, it prints the steps to do it by hand:

1. Create a GitHub App (organization settings, Developer settings, GitHub Apps), webhook off,
   with repository permissions Contents, Pull requests and Workflows set to read and write.
2. Install it on the repository holding `perseid.toml` and the SDK repositories.
3. On each of them, store its App ID as the `SDK_APP_ID` variable and a private key as the
   `SDK_APP_PRIVATE_KEY` secret.

Run `perseid app` again after adding an SDK repository: it installs the App there too. All SDK
repositories must belong to one account. `--dry-run` prints the plan and exits with 2 when changes
are pending, `--yes` applies without asking, `--no-browser` prints URLs instead of opening them.

**Why not the default `GITHUB_TOKEN`?** It can't reach other repositories. Events it causes start
no workflow, except `workflow_dispatch` and `repository_dispatch`
([GitHub docs](https://docs.github.com/en/actions/concepts/security/github_token)). Pull requests
it opens get no CI, and merges it makes publish nothing.

You can still pass it to the Action by hand for SDKs kept in the same repository:
`token: ${{ github.token }}`, with `contents: write` and `pull-requests: write` on the job, and
"Allow GitHub Actions to create and approve pull requests" in the repository settings. List your
CI workflows in `ci-workflows` and grant `actions: write`: the Action then dispatches them on
`perseid/update`. Each needs `on: workflow_dispatch`. Dispatched runs don't show in the pull
request's checks or count as required status checks
([GitHub docs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)).
To gate merges on them, have the workflow post a commit status and require that context:

```yaml
on: [pull_request, workflow_dispatch]
jobs:
  test:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      statuses: write
    steps:
      - uses: actions/checkout@v5
      - run: npm ci && npm test
      - if: always()
        env:
          GH_TOKEN: ${{ github.token }}
          SHA: ${{ github.event.pull_request.head.sha || github.sha }}
          STATE: ${{ job.status == 'success' && 'success' || 'failure' }}
        run: |
          gh api "repos/$GITHUB_REPOSITORY/statuses/$SHA" -f context=sdk-ci -f state="$STATE" \
            -f target_url="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
```

"Allow GitHub Actions to create and approve pull requests" also lets any workflow approve pull
requests, which can satisfy a required review with no human involved. Keep "Workflow permissions"
read-only by default and grant write per job.

### `perseid connect`

Run it in the API repository when another repository holds `perseid.toml`:

```sh
npx perseid connect acme/api-sdks
```

It reads `perseid.toml` from the SDKs repository and finds the spec here. `--spec <path>` picks
another one. `--build "<command>"` runs a command writing the spec in CI, when it isn't committed.
It asks when to push the spec, proposing `release` when the repository publishes GitHub releases
and `change` otherwise (`--on change|release|tag`, `--tags` for `tag`).

It then plans two changes:

1. A write deploy key on the SDKs repository. Its private half becomes the
   `PERSEID_SDKS_DEPLOY_KEY` secret of the API repository. The key is generated in memory and
   never written to disk. Without admin rights on the SDKs repository, perseid prints the public
   key for one of its admins to add.
2. `.github/workflows/perseid-push.yml`, written in your clone for you to commit.

perseid asks before adding the key and the secret. If you decline, it prints the `ssh-keygen` and
`gh` commands to add them yourself. A deploy key doesn't expire and reaches one repository only.

Other ways to authenticate, which `connect` checks but doesn't create:

| `--auth` | The API repository needs |
|---|---|
| `token` | `PERSEID_SDKS_TOKEN` secret: a fine-grained token with Contents read and write on the SDKs repository |
| `app` | `PERSEID_PUSH_APP_ID` variable and `PERSEID_PUSH_APP_PRIVATE_KEY` secret: a GitHub App installed on the SDKs repository only |

Running `connect` again keeps the settings you don't pass, and changes nothing once in sync.
`--private` leaves the API repository's name out of what the SDKs repository records. It takes
`--dry-run`, `--yes` and `--no-browser` like `perseid app`.

### `perseid status`

`status` changes nothing. It compares `perseid.toml` with GitHub, then reports how the automation
fares:

- in the SDKs repository: secrets, workflows not yet refreshed or pushed, the last spec pushed
  (commit, release and age), open `perseid/update` pull requests, the last `sdks.yml` run and the
  App installation;
- in the API repository: the key and secret `perseid-push.yml` needs, the last spec the SDKs
  repository received and the last `perseid-push.yml` run.

Each problem comes with its fix. It exits with 2 when something waits on you, 1 on errors and 0
otherwise. It signs in with `GH_TOKEN`, `GITHUB_TOKEN` or the token `gh` stores. Without one, it
checks the clone only.

## GitHub Action

This is the `sdks.yml` that `perseid init` writes:

```yaml
name: SDKs

on:
  push:
    branches: ["main"]
    paths: ["openapi.json","perseid.toml",".github/workflows/sdks.yml"]
  workflow_dispatch:

permissions:
  contents: read

concurrency: sdks

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
      - if: hashFiles('openapi.json') != ''
        uses: meteroid-oss/perseid@v0.6
        with:
          token: ${{ secrets.PERSEID_TOKEN }}
          app-id: ${{ vars.SDK_APP_ID }}
          app-private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}
```

It pins the release line of the perseid that wrote it: `@v0.6` for 0.6.x. Before 1.0 a minor
release may break. Run `perseid init` with a newer perseid to move to its line.

The Action installs the perseid matching its own ref, then:

1. `perseid tools list --github-output` reads the SDKs of `perseid.toml`, for the toolchains to set
   up (Rust, Go, .NET) and the repositories an App token must cover.
2. `perseid tools install` downloads the pinned formatters and oasdiff.
3. With `app-id`, it mints an App token for the repository holding `perseid.toml` and the SDK
   repositories, with Contents, Pull requests and Workflows write. Otherwise it uses `token`.
4. It runs `perseid generate --pr`. perseid commits the generated files on top of the current
   branch to `perseid/update`, and opens or updates a pull request in each repository holding
   SDKs. [Releases](#releases) explains how it is titled.

| Input | Default | |
|---|---|---|
| `command` | `generate --pr` | Arguments passed to perseid. `generate --check` fails on drift, for pull request checks |
| `token` | | A token with Contents, Pull requests and Workflows write on every target repository, such as `PERSEID_TOKEN`. Unused with `app-id` |
| `app-id`, `app-private-key` | | A GitHub App whose token the Action mints |
| `working-directory` | `.` | The directory holding `perseid.toml` |
| `version` | the Action's ref | The perseid version to run |
| `bump` | `auto` | `--bump`: `auto`, `major`, `minor` or `patch` |
| `base-spec` | | `--base-spec`: the previous spec `auto` compares with |
| `relax-enum-additions` | `true` | `--relax-enum-additions` |
| `auto-merge` | `false` | `--auto-merge` |
| `ci-workflows` | | `--dispatch`, only when `token` is `github.token` |

The inputs reach perseid as `PERSEID_BUMP`, `PERSEID_BASE_SPEC`, `PERSEID_RELAX_ENUM_ADDITIONS`,
`PERSEID_AUTO_MERGE` and `PERSEID_DISPATCH`, which it reads like its flags.

### `generate --pr` outside the Action

`generate --pr` works the same on your machine. It copies the generated files, the spec and a new
SDK's skeleton into a temporary worktree of `origin/<your branch>`, or of the default branch when
yours isn't pushed, and commits there. Your branch, index, uncommitted changes and unpushed commits
stay out of the pull request. Without a git identity, commits are authored by
`github-actions[bot]`.

It doesn't need the `gh` CLI. perseid calls the GitHub API with `GH_TOKEN`, else `GITHUB_TOKEN`,
else the token `gh` stores, else a browser login in a terminal. With a token in the environment,
git clones, fetches and pushes with it too, replacing the credentials `actions/checkout` persists.
Otherwise git uses your own credentials.

## Spec pushes

`perseid-push.yml` runs on the `--on` you chose:

- `change`: on the default branch, when the spec changes. With `--build`, on every push, after
  running the command.
- `release`: when a GitHub release is published.
- `tag`: when a tag matching `--tags` (`v*` by default) is pushed.

It can also run by hand from the default branch or a tag. Its `meteroid-oss/perseid/push` step
clones the SDKs repository over SSH, pinned to GitHub's published host keys. It commits the spec
to the `spec` path of that repository's `perseid.toml`, with the message
`spec: acme/api@a1b2c3d` (`spec: acme/api@v1.4.0 (a1b2c3d)` for a release or tag), and records the
commit and tag in `.perseid/source.json`. It skips the push when:

- the SDKs repository has no `perseid.toml` yet: run `perseid init` there, push, then run the
  workflow again;
- the pushed commit is older than the one already synced: an older spec never overwrites a newer
  one;
- the spec didn't change.

If the pushed commit and the synced one have diverged, for example a release cut from a branch,
the run fails instead of skipping every later push. Remove `sha` from `.perseid/source.json` in the
SDKs repository to accept the pushed spec.

The SDK pull request names where the spec came from: "Generated from acme/api@a1b2c3d", or
"acme/api@v1.4.0 (a1b2c3d)" for a release. With `--private`, only the commit and tag are recorded.

To ship SDKs for each release of your API rather than each merged change, run
`perseid connect acme/api-sdks --on release`. It rewrites `perseid-push.yml` to run on
`release: published`.

## Self-hosted runners and other CIs

`ghcr.io/meteroid-oss/perseid` bundles perseid, git, oasdiff and every pinned formatter.

```yaml
jobs:
  sdks:
    runs-on: self-hosted
    container: ghcr.io/meteroid-oss/perseid:0
    steps:
      - uses: actions/checkout@v5
      - run: perseid generate --pr
        env:
          GH_TOKEN: ${{ secrets.PERSEID_TOKEN }}
```

Or anywhere:

```sh
docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/work" ghcr.io/meteroid-oss/perseid generate --check
```

Without the image, `perseid tools install [--dir <bin>]` downloads the formatters the SDKs of
`perseid.toml` need, and oasdiff, next to perseid by default. `perseid tools list` prints them
with their versions. `rustfmt`, `gofmt` and the `dotnet` that installs `csharpier` come from the
Rust, Go and .NET toolchains.

## Releases

Every SDK has its own version, in its own manifest, and is released on its own. Each repository
holding SDKs gets [release-please](https://github.com/googleapis/release-please) files and
`.github/workflows/sdk-release.yml` at its root. `perseid init` writes them for SDKs kept in the
repository holding `perseid.toml`. The first pull request in an SDK repository carries them.
`release = false` in `perseid.toml` leaves them out. With `perseid.toml` in a folder, the files
still go at the root, and release-please packages are named by their path from the root
(`api/typescript`).

How a spec change becomes a release:

1. `generate --pr` titles its pull request as a conventional commit sized by
   [oasdiff](https://github.com/oasdiff/oasdiff): `feat(api)!:` for breaking changes, `feat(api):`
   for other API changes, `fix(api):` otherwise. oasdiff's changelog goes in the description. The
   previous spec is `--base-spec` if given, else the spec before the pushed commits
   (`GITHUB_EVENT_BEFORE`, set by the Action), else the previous commit. Without one, or without
   oasdiff, it asks for a minor release. `--bump major|minor|patch` skips the comparison. An open
   pull request keeps its largest bump.
2. Merging it, or any `fix:` or `feat:` commit touching an SDK, opens a release PR that bumps the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK and publishes it. Tags look like `rust/v0.4.0`, or
   `v0.4.0` alone in its repository. Go tags carry the module's folder, such as `api/go/v0.4.0`,
   as the module proxy expects.

Publishing uses trusted publishing (OIDC) for npm, PyPI and crates.io, a Central Portal token and
GPG key for Maven Central, an API key for NuGet, and the module proxy for Go. Versions already on
the registry are skipped, so re-running a failed job is safe. `perseid init` prints what each
registry needs before the first release.

`sdk-release.yml` has two jobs, skipped in forks:

- `release` runs `meteroid-oss/perseid/release` on pushes to the default branch. It runs
  release-please and outputs the paths of the released packages.
- `publish` runs `meteroid-oss/perseid/publish` once per released path, in the `release`
  environment. It is the only job with an OIDC token (`id-token: write`). Registries trust this
  file name and this environment, so keep both.

Running `sdk-release.yml` by hand publishes one package again from the selected tag or branch. Its
`path` input names a package of `release-please-config.json` (`.` for the root). `perseid init`
rewrites the file when it differs from what it writes. Delete its first line to keep your own
version. The publish job installs npm dependencies without running their scripts, which would
otherwise see its OIDC token.

`meteroid-oss/perseid/release` takes:

| Input | `sdk-release.yml` passes | |
|---|---|---|
| `app-id`, `app-private-key` | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | A GitHub App whose token, minted for this repository with Contents and Pull requests write, runs release-please |
| `release-token` | `PERSEID_TOKEN` secret | The token for release-please without an App |
| `token` | | Otherwise, the default `GITHUB_TOKEN` |
| `ci-workflows` | `SDK_CI_WORKFLOWS` variable | Workflow files dispatched on the release PR branch when it was pushed with the default token |
| `path` | the `path` of a manual run | A package to publish again instead of running release-please |

It outputs `paths` (JSON array of the released package paths), `tags` (their release tags, by
path) and `releases` (release-please's outputs, as JSON). With the default token, release PRs get
no CI unless dispatched, and auto-merged release PRs publish nothing.

### Auto-merge

`--auto-merge` (`auto-merge: true`) enables GitHub auto-merge (squash) on the SDK pull requests. It
also labels them `perseid:auto-release`, so the release action auto-merges the release PR they
lead to. It needs:

- "Allow auto-merge" in the repository settings;
- required status checks, through branch protection or rulesets. Without any, GitHub merges at
  once;
- checks that run on the pull requests: `PERSEID_TOKEN` or the App, or `ci-workflows` plus a
  required commit status (see [Tokens](#tokens));
- `PERSEID_TOKEN` or the App for both `sdks.yml` and `sdk-release.yml`. A merge made with the
  default `GITHUB_TOKEN` triggers no workflow, so nothing would be released.

`--relax-enum-additions`, on by default, counts enum values added to responses as minor changes.
That is only safe while the SDKs accept unknown enum values, which generated SDKs do.

### perseid's own releases

`release-please.yml` dispatches `release.yml`, which publishes the binaries to GitHub and ghcr.io
and the `perseid` npm package with trusted publishing. The npm package holds no binary. On first
run it downloads the release archive for its version, checks it against the published checksums,
and caches it.

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration. Locally, a missing `biome` or `ruff` runs pinned
through `npx` or `uvx`. The Action runs `perseid tools install`, which downloads only the native
formatters the configured languages need: no JVM, Node or Python setup. `csharpier` installs as a
.NET tool.
