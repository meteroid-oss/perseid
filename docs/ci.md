# CI and releases

## Repository layouts

`perseid.toml` lives in the repository that runs the generation. It lists the SDKs (`sdks`),
where they live (`repo`) and the spec they come from (`spec`).

| Layout | Pick it when | `perseid.toml` |
|---|---|---|
| [Next to the API](#next-to-the-api) | One team owns the API and the SDKs | `sdks = [...]` |
| [One repository per language](#one-repository-per-language) | Each ecosystem gets its own home | `repo = "acme/api-{lang}"` |
| [A separate SDKs repository](#a-separate-sdks-repository) | The API repository must grant nothing else | in the SDKs repository |
| [A spec at a URL](#a-spec-at-a-url) | The spec is served, not committed | `spec = "https://..."` |

perseid never creates repositories. Create the ones `repo` names with `gh repo create`.
`perseid init` and `perseid status` print the commands.

### Next to the API

Each SDK is a folder of the repository holding the spec.

```
acme/api ──PRs──▶ acme/api (typescript/, python/)
```

```toml
spec = "openapi.json"
sdks = ["typescript", "python"]
```

### One repository per language

```
acme/api ──PRs──▶ acme/api-typescript, acme/api-python
```

```toml
sdks = ["typescript", "python"]
repo = "acme/api-{lang}"
```

`{lang}` is the language as `sdks` names it. A `repo` without `{lang}`, such as
`"acme/api-sdks"`, holds every SDK in a folder named after its language.

### A separate SDKs repository

The API repository only pushes its spec to the SDKs repository, with a deploy key that writes to
that one repository. The SDKs repository generates the SDKs and gets no access to the API
repository.

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

### A spec at a URL

`spec = "https://api.acme.com/openapi.json"` needs nothing from the API side. `sdks.yml` fetches
it every day and on demand, and opens pull requests when it changed. Their description names the
URL and a digest of what it served.

## Setup

You need:

- Node 18+ for `npx perseid`, or the [install script](../install.sh).
- Admin rights on the repository holding `perseid.toml`, to add its secret.
- For `perseid app` in an organization: owner or GitHub App manager.
- For `perseid connect`: admin rights on the API repository, and ideally on the SDKs repository.
- An account on each registry you publish to: npm, PyPI, crates.io, Maven Central, NuGet.

### Who can write where

Each workflow gets a credential reaching only the repositories it writes to. The credential is
stored in the repository the workflow runs in.

| Workflow | Runs in | Writes to | Credential |
|---|---|---|---|
| `sdks.yml`, `sdk-release.yml` | the repository holding `perseid.toml`, each SDK repository | those repositories: contents, pull requests, workflows | the [`perseid app`](#perseid-app) App, else [`PERSEID_TOKEN`](#tokens) |
| `perseid-push.yml` | the API repository | the SDKs repository: contents | a deploy key, else a token or an App, see [`--auth`](#perseid-connect) |
| the publish job of `sdk-release.yml` | the SDK repository, `release` environment | the registries | [trusted publishing](#publishing), else a registry token |

- Anyone who can run workflows in a repository can use its secrets: `connect` gives the API
  repository a key to the SDKs repository only, never one to the SDK repositories.
- The `perseid app` App can push the spec too (`connect --auth app` with its ID and a new key).
  Its key then reaches every SDK repository from the API repository: protect their default
  branch and require a review on their `release` environment, or use a separate App installed on
  the SDKs repository only.

### `perseid init`

`init` works in your clone only. It finds the spec (a tracked `openapi` or `swagger` file, JSON or
YAML) and asks which SDKs to generate, where they live and the API name. It writes:

- `perseid.toml`;
- `.github/workflows/sdks.yml`, which regenerates the SDKs;
- for the SDKs kept in this repository, `sdk-release.yml` and the release-please files.

Commit and push them: workflows run from the default branch. SDKs in their own repositories get
their release files in their first pull request.

- Run `init` again after editing `perseid.toml`. It rewrites the workflows it wrote, except one
  whose first line, ``# Written by `perseid init` ``, you removed.
- Without a terminal, pass `--sdks`, and optionally `--repo`, `--spec`, `--name` and `--base-url`.
- Without a spec in the repository, it asks where the spec is: at a URL, in a file, or in another
  repository. For the last, `spec` defaults to `openapi.json`, where `perseid connect` pushes it.
  `perseid generate --spec <path|url> --out /tmp/sdks` previews the SDKs meanwhile.
- It ends with the next steps: SDK repositories to create, a GitHub App or a token, the
  `perseid connect` to run, and what each registry needs before the first release.

### `perseid generate`

| Flag | |
|---|---|
| none | Writes the SDKs where `perseid.toml` says, and prints where each one went |
| `--out <dir>` | Writes every SDK to `<dir>/<language>`, without cloning anything |
| `--check` | Fails when the SDKs differ from what the spec gives. With `--out`, compares against that directory |
| `--pr` | Commits the SDKs and opens pull requests, see [the Action](#github-action) |
| `--spec <path\|url>` | Reads another spec |
| `--no-format` | Skips the formatters |

- The first generation of an SDK writes its package skeleton: manifest, README, error types.
- The README's examples call operations of the spec (a retrieve, the first paginated list, a
  stream). It is yours afterwards. `api.md`, the reference of every method, is regenerated with
  the code.
- An SDK with its own `repo` is generated into a shallow clone under
  `.perseid/repos/<owner>/<name>`. Changes there that perseid did not make stop `generate`,
  unless `--pr` is passed.

### Tokens

`sdks.yml` opens pull requests with one of:

| Credential | |
|---|---|
| `PERSEID_TOKEN` secret | A [fine-grained token](https://github.com/settings/personal-access-tokens/new) with Contents, Pull requests and Workflows read and write, on the repository holding `perseid.toml` and every SDK repository |
| A GitHub App, set up by `perseid app` | Tokens minted on each run and never stored: nothing expires |

- Add `PERSEID_TOKEN` to every SDK repository too: `sdk-release.yml` uses it.
- Workflows permission is needed because the first pull request in an SDK repository adds
  `sdk-release.yml`.
- When the token expires within 30 days, `generate --pr` warns on each run.

Without either, the Action fails with:

```
error: no token to open the SDK pull requests: add the PERSEID_TOKEN secret (a fine-grained token
with Contents, Pull requests and Workflows read and write on the SDK repositories), or run `perseid app`
```

### `perseid app`

`perseid app` creates a GitHub App named `<name>-sdk-bot` in your browser, on the account that
owns the SDK repositories. It has Contents, Pull requests and Workflows write, and no webhook.

It then installs the App on the repository holding `perseid.toml` and the SDK repositories, and
stores the `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret on each. It prints the plan
and asks first. If you decline, it prints the steps to do it by hand:

1. Create a GitHub App (organization settings, Developer settings, GitHub Apps), webhook off,
   with repository permissions Contents, Pull requests and Workflows set to read and write.
2. Install it on the repository holding `perseid.toml` and the SDK repositories.
3. On each, store its App ID as the `SDK_APP_ID` variable and a private key as the
   `SDK_APP_PRIVATE_KEY` secret.

| Flag | |
|---|---|
| `--dry-run` | Prints the plan, exits with 2 when changes are pending |
| `--yes` | Applies without asking |
| `--no-browser` | Prints URLs instead of opening them |

Run `perseid app` again after adding an SDK repository: it installs the App there too. GitHub
shows a private key only once, so it asks you to generate a new one on the App's settings page,
checks it belongs to the App, stores it on the repositories lacking one, and offers to delete the
downloaded file. All SDK repositories must belong to one account.

### The default `GITHUB_TOKEN`

The default token cannot reach other repositories. Events it causes start no workflow, except
`workflow_dispatch` and `repository_dispatch`
([GitHub docs](https://docs.github.com/en/actions/concepts/security/github_token)). Pull requests
it opens get no CI, and merges it makes publish nothing.

For SDKs kept in the same repository, you can pass it to the Action by hand:

- `token: ${{ github.token }}`, with `contents: write` and `pull-requests: write` on the job;
- "Allow GitHub Actions to create and approve pull requests" in the repository settings;
- `ci-workflows` listing your CI workflows, with `actions: write`. The Action dispatches them on
  `perseid/update`, and each needs `on: workflow_dispatch`.

Dispatched runs do not show in the pull request's checks or count as required status checks
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
requests, which can satisfy a required review with no human involved. Keep "Workflow
permissions" read-only by default and grant write per job.

### `perseid connect`

Run it in the API repository when another repository holds `perseid.toml`:

```sh
npx perseid connect acme/api-sdks
```

It reads `perseid.toml` from the SDKs repository and finds the spec here.

| Flag | |
|---|---|
| `--spec <path>` | Another spec file |
| `--build "<command>"` | A command writing the spec in CI, when it is not committed |
| `--on change\|release\|tag` | When to push the spec. Proposes `release` when the repository publishes GitHub releases, else `change` |
| `--tags <glob>` | The tags of `--on tag`, `v*` by default |
| `--auth deploy-key\|token\|app` | How the API repository authenticates, a deploy key by default |
| `--private` | Leaves the API repository's name out of what the SDKs repository records |
| `--dry-run`, `--yes`, `--no-browser` | As for `perseid app` |

It plans two changes:

1. A write deploy key on the SDKs repository, whose private half becomes the
   `PERSEID_SDKS_DEPLOY_KEY` secret of the API repository. The key is generated in memory and
   never written to disk.
2. `.github/workflows/perseid-push.yml`, written in your clone for you to commit.

- perseid asks before adding the key and the secret. If you decline, it prints the `ssh-keygen`
  and `gh` commands to add them yourself.
- Without admin rights on the SDKs repository, it prints the public key for one of its admins.
- A deploy key does not expire and reaches one repository only.
- Running `connect` again keeps the settings you do not pass, and changes nothing once in sync.

The other `--auth` values are checked but not created:

| `--auth` | The API repository needs |
|---|---|
| `token` | `PERSEID_SDKS_TOKEN` secret: a fine-grained token with Contents read and write on the SDKs repository |
| `app` | `PERSEID_PUSH_APP_ID` variable and `PERSEID_PUSH_APP_PRIVATE_KEY` secret: a GitHub App installed on the SDKs repository only |

### `perseid status`

`status` changes nothing. It compares `perseid.toml` with GitHub and reports how the automation
fares, each problem with its fix.

| Repository | Checks |
|---|---|
| SDKs repository | Secrets, workflows not yet refreshed or pushed, the last spec pushed (commit, release, age), open `perseid/update` pull requests, the last `sdks.yml` run, the App installation |
| API repository | The key and secret `perseid-push.yml` needs, the last spec the SDKs repository received, the last `perseid-push.yml` run |

- Exits with 2 when something waits on you, 1 on errors, 0 otherwise.
- Signs in with `GH_TOKEN`, `GITHUB_TOKEN` or the token `gh` stores. Without one, it checks the
  clone only.

## GitHub Action

The `sdks.yml` that `perseid init` writes:

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

It pins the release line of the perseid that wrote it: `@v0.6` for 0.6.x. Before 1.0, a minor
release may break. Run `perseid init` with a newer perseid to move to its line.

The Action installs the perseid matching its own ref, then:

1. `perseid tools list --github-output` reads the SDKs of `perseid.toml`: the toolchains to set
   up (Rust, Go, .NET) and the repositories an App token must cover.
2. `perseid tools install` downloads the pinned formatters and oasdiff.
3. With `app-id`, it mints an App token for the repository holding `perseid.toml` and the SDK
   repositories, with Contents, Pull requests and Workflows write. Otherwise it uses `token`.
4. It runs `perseid generate --pr`, which commits the generated files on top of the current
   branch to `perseid/update`. It opens or updates a pull request in each repository holding
   SDKs, titled as described in [releases](#releases).

| Input | Default | |
|---|---|---|
| `command` | `generate --pr` | Arguments passed to perseid. `generate --check` fails on drift, for pull request checks |
| `token` | | Contents, Pull requests and Workflows write on every target repository, such as `PERSEID_TOKEN`. Unused with `app-id` |
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

`generate --pr` works the same on your machine:

- It copies the generated files, the spec and a new SDK's skeleton into a temporary worktree of
  `origin/<your branch>`, or of the default branch when yours is not pushed, and commits there.
- Your branch, index, uncommitted changes and unpushed commits stay out of the pull request.
- Without a git identity, commits are authored by `github-actions[bot]`.

It does not need the `gh` CLI. It calls the GitHub API with `GH_TOKEN`, else `GITHUB_TOKEN`, else
the token `gh` stores, else a browser login in a terminal. With a token in the environment, git
clones, fetches and pushes with it too, over the credentials `actions/checkout` persists.
Otherwise git uses your own credentials.

## Spec pushes

`perseid-push.yml` runs on the `--on` you chose:

| `--on` | Runs |
|---|---|
| `change` | On the default branch, when the spec changes. With `--build`, on every push, after the command |
| `release` | When a GitHub release is published |
| `tag` | When a tag matching `--tags` is pushed |

It can also run by hand from the default branch or a tag. Its `meteroid-oss/perseid/push` step:

- clones the SDKs repository over SSH, pinned to GitHub's published host keys;
- commits the spec to the `spec` path of that repository's `perseid.toml`, with the message
  `spec: acme/api@a1b2c3d`, or `spec: acme/api@v1.4.0 (a1b2c3d)` for a release or tag;
- records the commit and tag in `.perseid/source.json`.

It skips the push when:

- the SDKs repository has no `perseid.toml` yet: run `perseid init` there, push, then run the
  workflow again;
- the pushed commit is older than the one already synced: an older spec never overwrites a newer
  one;
- the spec did not change.

When the pushed commit and the synced one have diverged, such as a release cut from a branch, the
run fails. Remove `sha` from `.perseid/source.json` in the SDKs repository to accept the pushed
spec.

The SDK pull request names where the spec came from: "Generated from acme/api@a1b2c3d", or
"acme/api@v1.4.0 (a1b2c3d)" for a release. With `--private`, only the commit and tag are recorded.

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

Without the image:

- `perseid tools install [--dir <bin>]` downloads oasdiff and the formatters the SDKs of
  `perseid.toml` need, next to perseid by default.
- `perseid tools list` prints them with their versions.
- `rustfmt`, `gofmt` and the `dotnet` that installs `csharpier` come from the Rust, Go and .NET
  toolchains.

## Releases

Every SDK has its own version, in its own manifest, and is released on its own.

- Each repository holding SDKs gets [release-please](https://github.com/googleapis/release-please)
  files and `.github/workflows/sdk-release.yml` at its root.
- `perseid init` writes them for SDKs kept next to `perseid.toml`. The first pull request in an
  SDK repository carries them.
- `release = false` in `perseid.toml` leaves them out.
- With `perseid.toml` in a folder, the files go at the root, and release-please packages are
  named by their path from the root (`api/typescript`).

### From spec change to release

1. `generate --pr` titles its pull request as a conventional commit sized by
   [oasdiff](https://github.com/oasdiff/oasdiff): `feat(api)!:` for breaking changes,
   `feat(api):` for other API changes, `fix(api):` otherwise. oasdiff's changelog goes in the
   description.
2. Merging it, or any `fix:` or `feat:` commit touching an SDK, opens a release PR that bumps the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK and publishes it.

Sizing the change:

- The previous spec is `--base-spec` if given, else the spec before the pushed commits
  (`GITHUB_EVENT_BEFORE`, set by the Action), else the previous commit.
- Without a previous spec, or without oasdiff, the pull request asks for a minor release.
- `--bump major|minor|patch` skips the comparison.
- An open pull request keeps its largest bump.
- `--relax-enum-additions`, on by default, counts enum values added to responses as minor
  changes. Generated SDKs accept unknown enum values, which makes this safe.

Tags look like `rust/v0.4.0`, or `v0.4.0` alone in its repository. Go tags carry the module's
folder, such as `api/go/v0.4.0`, as the module proxy expects.

### Publishing

| Registry | Credential |
|---|---|
| npm, PyPI, crates.io | Trusted publishing (OIDC) |
| Maven Central | A Central Portal token and a GPG key |
| NuGet | An API key |
| Go | The module proxy, nothing to set up |

Versions already on the registry are skipped, so re-running a failed job is safe. `perseid init`
prints what each registry needs before the first release.

`sdk-release.yml` has two jobs, skipped in forks:

- `release` runs `meteroid-oss/perseid/release` on pushes to the default branch. It runs
  release-please and outputs the paths of the released packages.
- `publish` runs `meteroid-oss/perseid/publish` once per released path, in the `release`
  environment. It is the only job with an OIDC token (`id-token: write`). Registries trust this
  file name and this environment, so keep both.

Notes:

- Running `sdk-release.yml` by hand publishes one package again from the selected tag or branch.
  Its `path` input names a package of `release-please-config.json` (`.` for the root).
- `perseid init` rewrites the file when it differs from what it writes. Delete its first line to
  keep your own version.
- The publish job installs npm dependencies without running their scripts, which would otherwise
  see its OIDC token.

`meteroid-oss/perseid/release` takes:

| Input | `sdk-release.yml` passes | |
|---|---|---|
| `app-id`, `app-private-key` | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | A GitHub App whose token, minted for this repository with Contents and Pull requests write, runs release-please |
| `release-token` | `PERSEID_TOKEN` secret | The token for release-please without an App |
| `token` | | Otherwise, the default `GITHUB_TOKEN` |
| `ci-workflows` | `SDK_CI_WORKFLOWS` variable | Workflow files dispatched on the release PR branch when it was pushed with the default token |
| `path` | the `path` of a manual run | A package to publish again, skipping release-please |

It outputs `paths` (JSON array of the released package paths), `tags` (their release tags, by
path) and `releases` (release-please's outputs, as JSON). With the default token, release PRs get
no CI unless dispatched, and auto-merged release PRs publish nothing.

### Auto-merge

`--auto-merge` (`auto-merge: true`) enables GitHub auto-merge (squash) on the SDK pull requests.
It also labels them `perseid:auto-release`, so the release action auto-merges the release PR they
lead to. It needs:

- "Allow auto-merge" in the repository settings;
- required status checks, through branch protection or rulesets. Without any, GitHub merges at
  once;
- checks that run on the pull requests: `PERSEID_TOKEN` or the App, or `ci-workflows` plus a
  required commit status (see [the default `GITHUB_TOKEN`](#the-default-github_token));
- `PERSEID_TOKEN` or the App for both `sdks.yml` and `sdk-release.yml`. A merge made with the
  default `GITHUB_TOKEN` triggers no workflow, so nothing would be released.

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration.

- Locally, a missing `biome` or `ruff` runs pinned through `npx` or `uvx`.
- The Action runs `perseid tools install`, which downloads only the native formatters the
  configured languages need: no JVM, Node or Python setup.
- `csharpier` installs as a .NET tool.

## The perseid npm package

`npx perseid` runs the `perseid` npm package, which holds no binary. On first run it downloads
the release archive for its version from GitHub, checks it against the checksums the package
ships, and caches it. `PERSEID_CACHE` sets the cache directory.
