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

[Targets](#targets) add repositories perseid writes one folder of, such as a docs site's, or
generates a program wrapping an SDK in, such as a CLI.

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

The API repository only pushes its spec to the SDKs repository, as the GitHub App of the SDKs or
with a token. The SDKs repository generates the SDKs and gets no access to the API repository.

```
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-sdks (typescript/, python/)
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-typescript, acme/api-python
```

```sh
# in acme/api-sdks
npx perseid init                    # spec = "openapi.json", which connect pushes
npx perseid sync                    # installs the perseid App here and on the SDK repositories
git add -A && git commit -m "ci: generate SDKs with perseid" && git push
# in acme/api, once perseid.toml is on the default branch of acme/api-sdks
npx perseid connect acme/api-sdks   # writes perseid-push.yml, opens a pull request adding `source`
```

The API repository needs no App and stores nothing: the `source` of the SDKs repository's
`perseid.toml` lets it push there, and nothing gets access to it.

### A spec at a URL

`spec = "https://api.acme.com/openapi.json"` needs nothing from the API side. `sdks.yml` fetches
it every day and on demand, and opens pull requests when it changed. Their description names the
URL and a digest of what it served.

## Setup

You need:

- Node 18+ for `npx perseid`, or the [install script](../install.sh).
- For `perseid sync`: rights to install a GitHub App on the account holding the repositories
  (an organization owner, or a repository admin who requests it), and write access to the SDK
  repositories, to commit their release workflow.
- For `perseid app` in an organization: owner or GitHub App manager.
- For `perseid connect`: write access to the SDKs repository, to open the pull request adding
  `source`. With `--auth app` or `--auth token`, admin rights on the API repository, to store
  the credential.
- An account on each registry you publish to: npm, PyPI, crates.io, Maven Central, NuGet.

### Credentials

Every perseid workflow takes the first credential it finds, so the same workflow files serve every
setup:

| Credential | Set up by | Stored as | Notes |
|---|---|---|---|
| A GitHub App of your own | [`perseid app`](#perseid-app) | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | Each run mints a token for an hour. You keep the key |
| A [fine-grained token](#tokens) | You | `SDK_GITHUB_TOKEN` secret | It expires and acts as you |
| The [perseid App](https://github.com/apps/perseid-sdks) | [`perseid sync`](#perseid-sync) | Nothing | Each run trades its GitHub OIDC token for a token of the App, for an hour |

| Workflow | Runs in | Writes to |
|---|---|---|
| `sdks.yml`, `sdk-release.yml` | the repository holding `perseid.toml`, each SDK repository | those repositories: contents, pull requests |
| `sdk-ci.yml` | the repository holding `perseid.toml`, each SDK repository | its own commit statuses |
| `perseid-push.yml` | the API repository | the SDKs repository: contents |
| the report job of `sdk-release.yml`, with [targets](#targets) waiting for releases | each SDK repository | the repository holding `perseid.toml`: a `repository_dispatch` event |
| the publish job of `sdk-release.yml` | the SDK repository, `release` environment | the registries, with [trusted publishing](#publishing) or a registry token |

No credential writes workflow files: `perseid init` and `perseid sync` write them with your own
credentials, so a token taken from a run can't add a workflow reading a repository's secrets.

The perseid App gives a run a token when:

- its repository and the target repositories are in one installation of the App, on selected
  repositories: installing the App on them lets the workflows of each write to the others;
- or the target's `perseid.toml`, on its default branch, names the run's repository as its
  `source`, by id: that repository gets Contents write on the target only, and needs no App.

Whoever can push to one of those repositories can get those tokens, as with a stored secret,
which any writer reads from a workflow on any branch. Install the App on the repositories of one
perseid setup only, and where the plan allows it, require a review on the SDK repositories'
`release` environment. The broker is [perseid-gh](https://github.com/meteroid-oss/perseid-gh).

### `perseid init`

`init` works in your clone only. It finds the spec (a tracked `openapi` or `swagger` file, JSON or
YAML) and asks which SDKs to generate, where they live, the API name and the license, offering
the spec's. It writes:

- `perseid.toml`, with a `[metadata]` table (what the spec doesn't tell is commented out) and a
  table per SDK naming its package;
- `.github/workflows/sdks.yml`, which regenerates the SDKs;
- for the SDKs kept in this repository, `sdk-ci.yml`, `sdk-release.yml` and the release-please
  files.

Commit and push them: workflows run from the default branch. SDKs in their own repositories get
their CI and release workflows from `perseid sync`, and their release-please files in their first
pull request.

- Run `init` again after editing `perseid.toml`. It rewrites the workflows it wrote, except one
  whose first line, ``# Written by `perseid init` ``, you removed.
- Without a terminal, pass `--sdks`, and optionally `--repo`, `--spec`, `--name`, `--base-url`
  and `--license` (an SPDX expression; the spec's by default).
- `--from stainless.yml` imports a Stainless config without asking, and lists what it can't
  carry over. See [migrating from Stainless](migrating-from-stainless.md).
- Without a spec in the repository, it asks where the spec is: at a URL, in a file, or in another
  repository. For the last, `spec` defaults to `openapi.json`, where `perseid connect` pushes it.
  `perseid generate --spec <path|url> --out /tmp/sdks` previews the SDKs meanwhile.
- It ends with the next steps: SDK repositories to create, `perseid sync`, the
  `perseid connect` to run, and what each registry needs before the first release.

### `perseid sync`

Run it in the repository holding `perseid.toml`, after `init` and after adding an SDK. It compares
GitHub with `perseid.toml`, prints the plan and applies it once you agree:

1. The perseid App, on the repository holding `perseid.toml`, every SDK repository and the
   repositories of the [targets](#targets). It opens the App's installation page with them
   selected, then checks the installation covers them.
   Skipped when they have `SDK_GITHUB_TOKEN`, or your own App (`SDK_APP_ID`), which `perseid app`
   manages.
2. `sdk-ci.yml` and `sdk-release.yml` in each SDK repository, committed with your credentials to
   the default branch, or through one pull request when the branch takes no direct push. One whose
   first line you removed stays yours.

| Flag | Effect |
|---|---|
| `--dry-run` | Prints the plan, exits with 2 when changes are pending |
| `--yes` | Applies without asking |
| `--no-browser` | Prints URLs instead of opening them |

SDK pull requests never write workflows: when `sdk-release.yml` is missing or outdated, the run
warns to run `perseid sync`. Both workflows use actions at `@v0`, so new perseid releases don't
change them. Pin it if you prefer, and let Dependabot's `github-actions` updates bump the pin.

### `perseid generate`

| Flag | Effect |
|---|---|
| none | Writes the SDKs where `perseid.toml` says, and prints where each one went |
| `--out <dir>` | Writes every SDK to `<dir>/<language>`, without cloning anything |
| `--check` | Fails when the SDKs differ from what the spec gives. With `--out`, compares against that directory |
| `--pr` | Commits the SDKs and opens pull requests, see [the Action](#github-action), then those of the [targets](#targets) |
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

| Credential | Tokens |
|---|---|
| The perseid App, installed by `perseid sync` | Tokens traded for the run's OIDC token, never stored |
| A GitHub App, set up by `perseid app` | Tokens minted on each run from the key you store |
| `SDK_GITHUB_TOKEN` secret | A [fine-grained token](https://github.com/settings/personal-access-tokens/new) with Contents and Pull requests read and write, on the repository holding `perseid.toml` and every SDK repository |

- Add `SDK_GITHUB_TOKEN` to every SDK repository too: `sdk-release.yml` uses it.
- When the token expires within 30 days, `generate --pr` warns on each run.

Without any, the Action fails with:

```
error: no token to open the SDK pull requests: run `perseid sync` to install the perseid App (the job
needs `permissions: id-token: write`), or `perseid app` for an App of your own, or add the
SDK_GITHUB_TOKEN secret (a fine-grained token with Contents and Pull requests read and write on the
SDK repositories)
```

### `perseid app`

`perseid app` is the alternative to the perseid App, for those who keep the key themselves. It
creates a GitHub App named `<name>-sdk-bot` in your browser, on the account that owns the SDK
repositories. It has Contents and Pull requests write, and no webhook.

It then installs the App on the repository holding `perseid.toml` and the SDK repositories, and
stores the `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret on each. It prints the plan
and asks first. If you decline, it prints the steps to do it by hand:

1. Create a GitHub App (organization settings, Developer settings, GitHub Apps), webhook off,
   with repository permissions Contents and Pull requests set to read and write.
2. Install it on the repository holding `perseid.toml` and the SDK repositories.
3. On each, store its App ID as the `SDK_APP_ID` variable and a private key as the
   `SDK_APP_PRIVATE_KEY` secret.

| Flag | Effect |
|---|---|
| `--dry-run` | Prints the plan, exits with 2 when changes are pending |
| `--yes` | Applies without asking |
| `--no-browser` | Prints URLs instead of opening them |

Like `perseid sync`, it commits `sdk-ci.yml` and `sdk-release.yml` to the SDK repositories
lacking them. Run
`perseid app` again after adding an SDK repository: it installs the App there too. GitHub
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
  each update branch (`perseid/update`, or `perseid/update-<language>`), and each needs
  `on: workflow_dispatch`.

Dispatched runs do not show in the pull request's checks or count as required status checks
([GitHub docs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)).
`sdk-ci.yml` posts the `sdk-ci` commit status when dispatched: list it in `ci-workflows` and
require that context. Your own workflows can do the same:

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

| Flag | Effect |
|---|---|
| `--spec <path>` | Another spec file |
| `--build "<command>"` | A command writing the spec in CI, when it is not committed |
| `--on change\|release\|tag` | When to push the spec. Proposes `release` when the repository publishes GitHub releases, else `change` |
| `--tags <glob>` | The tags of `--on tag`, `v*` by default |
| `--auth perseid\|app\|token` | How `perseid-push.yml` authenticates: the App of the SDKs repository when `perseid app` set one up, else the perseid App |
| `--private` | Leaves the API repository's name out of what the SDKs repository records |
| `--dry-run`, `--yes`, `--no-browser` | As for `perseid app` |

It plans two changes:

1. The [credential](#credentials) of the API repository:
   - `perseid`: nothing in the API repository. It opens a pull request on the SDKs repository
     adding, at the top level of its `perseid.toml`,
     `source = { repo = "acme/api", id = <its id>, target_id = <the SDKs repository's id> }`.
     Once merged, the perseid App lets the API repository push there, Contents write only. The
     ids keep a repository recreated under the same name, or a copy of the file elsewhere, from
     getting anything. `--private` leaves `repo` out.
   - `app`: the `SDK_APP_ID` variable, copied from the SDKs repository, and the
     `SDK_APP_PRIVATE_KEY` secret, a new key of the App. GitHub shows a key only once, so perseid
     opens the App's settings page, where you generate one. It takes the newest download, or the
     path you give, checks the key belongs to the App, stores it and offers to delete the file.
   - `token`: the `SDK_GITHUB_TOKEN` secret, a fine-grained token with Contents read and write on
     the SDKs repository, which you paste (with `--yes`, from the `SDK_GITHUB_TOKEN` environment
     variable). perseid checks it can see the SDKs repository.
2. `.github/workflows/perseid-push.yml`, written in your clone for you to commit.

- perseid asks before storing anything or opening the pull request. If you decline, it prints
  how to do it yourself.
- Running `connect` again keeps the settings you do not pass, and changes nothing once in sync.

### `perseid status`

`status` changes nothing. It compares `perseid.toml` with GitHub and reports how the automation
fares, each problem with its fix.

| Repository | Checks |
|---|---|
| SDKs repository | Secrets, workflows not yet refreshed or pushed, the last spec pushed (commit, release, age), open update pull requests, the last `sdks.yml` run, the App installation |
| API repository | The credential `perseid-push.yml` needs (the `source` of the SDKs repository, or the App's key or token), the last spec the SDKs repository received, the last `perseid-push.yml` run |

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
  id-token: write

concurrency: sdks

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
      - if: hashFiles('openapi.json') != ''
        uses: meteroid-oss/perseid@v0.6
        with:
          token: ${{ secrets.SDK_GITHUB_TOKEN }}
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
   repositories, with Contents and Pull requests write. Otherwise it uses `token`, and without
   one, asks the perseid App's broker for a token of the same repositories.
4. It runs `perseid generate --pr`, which commits the generated files on top of the current
   branch to `perseid/update`. It opens or updates a pull request in each repository holding
   SDKs, titled as described in [releases](#releases).

   release-please tells which SDKs a commit changed from its files, of which GitHub lists the
   first 3000. So when an update of several SDKs in one repository changes more than 2500 files
   (a first generation, or a perseid upgrade rewriting them all), each SDK gets its own pull
   request, from `perseid/update-<language>`, carrying the spec and release files along. Updates
   stay split until those pull requests are merged, then share one again. An open
   `perseid/update` pull request they replace is closed, as is the pull request of an SDK the
   base branch already holds as generated. Each merges on its own: where branch protection
   requires branches to be up to date, merge them one at a time or through a merge queue.

| Input | Default | Description |
|---|---|---|
| `command` | `generate --pr` | Arguments passed to perseid. `generate --check` fails on drift, for pull request checks |
| `token` | | Contents and Pull requests write on every target repository, such as `SDK_GITHUB_TOKEN`. Unused with `app-id`. Without either, the perseid App's |
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
          GH_TOKEN: ${{ secrets.SDK_GITHUB_TOKEN }}
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

## SDK tests

`.github/workflows/sdk-ci.yml`, in each repository holding SDKs, runs
`meteroid-oss/perseid/test@v0` on each SDK, for pull requests, pushes to the default branch and
manual runs. The action builds the SDK and runs its tests, the generated ones and yours:

| SDK | Runs |
|---|---|
| Rust | `cargo test` |
| TypeScript | `npm ci` (or `npm install`), `tsc --noEmit`, `npm test` |
| Python | `pip install -e .` in a venv, `unittest` on `tests/` |
| Go | `go vet ./...`, `go test ./...` |
| Java | `gradle build` (`./gradlew` when present) |
| C# | `dotnet test` |

- When the SDKs live in folders next to other code, it runs only on changes under them.
- A dispatched run posts the `sdk-ci` commit status, so the release PRs and SDK pull requests
  pushed with the default token can require it (see
  [the default `GITHUB_TOKEN`](#the-default-github_token)).
- Make its `test` jobs required status checks to gate merges, and `--auto-merge`, on the tests.
- Like `sdk-release.yml`, `init` and `sync` rewrite it unless you delete its first line.

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
   `feat(api):` for other API changes, `fix(api):` otherwise. Its changelog entries are nested
   in the commit as conventional commits, and every change oasdiff finds is listed, folded, in
   the description.
2. Merging it, or any `fix:` or `feat:` commit touching an SDK, opens a release PR that bumps the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK and publishes it.

Sizing the change:

- The previous spec is `--base-spec` if given, else the spec before the pushed commits
  (`GITHUB_EVENT_BEFORE`, set by the Action), else the previous commit.
- Without a previous spec, or without oasdiff, the pull request asks for a minor release.
- `--bump major|minor|patch` skips the comparison.
- oasdiff compares specs, not SDKs: an endpoint added next to another can move it into a new
  [child resource](configuration.md#resources), which breaks its calls. Pin such methods with
  `[resources]`.
- An open pull request keeps its largest bump, and the API changes it already lists.
- SDKs generated for the first time get no API changes: everything in them is new.
- `--relax-enum-additions`, on by default, counts enum values added to responses as minor
  changes. Generated SDKs accept unknown enum values, which makes this safe.

The SDK changelogs name each breaking change (up to 10), then sum up the endpoints added,
removed after deprecation and updated, naming up to 5 of each:

```markdown
### ⚠ BREAKING CHANGES

* **api:** `DELETE /pets/{id}`: api path removed without deprecation

### Features

* **api:** add `POST /pets`
* **api:** update `GET /pets`, `GET /owners` and 3 more
* **api:** update SDKs to Pets 2
```

release-please reads them from the merged commit. Squash merges take them from the commit or
the description; a squash message set to the pull request title alone drops them.

Tags look like `rust/v0.4.0`, or `v0.4.0` alone in its repository. Go tags carry the module's
folder, such as `api/go/v0.4.0`, as the module proxy expects.

### Publishing

| Registry | Credential |
|---|---|
| npm, PyPI, crates.io | Trusted publishing (OIDC) |
| NuGet | Trusted publishing (OIDC), with a `NUGET_USER` variable naming the policy's owner, or a `NUGET_API_KEY` secret |
| Maven Central | A Central Portal token and a GPG key |
| Go | The module proxy, nothing to set up |

Versions already on the registry are skipped, so re-running a failed job is safe. `perseid init`
prints what each registry needs before the first release.

`sdk-release.yml` has two jobs, skipped in forks, and a third with [targets](#targets) waiting
for releases:

- `release` runs `meteroid-oss/perseid/release` on pushes to the default branch. It runs
  release-please and outputs the paths of the released packages.
- `publish` runs `meteroid-oss/perseid/publish` once per released path, in the `release`
  environment. Registries trust this file name and this environment, so keep both.
- `report` runs `meteroid-oss/perseid/report` once every path is published: it tells the
  repository holding `perseid.toml`, so its targets follow.

Notes:

- Running `sdk-release.yml` by hand publishes one package again from the selected tag or branch.
  Its `path` input names a package of `release-please-config.json` (`.` for the root).
- `perseid init` rewrites the file when it differs from what it writes. Delete its first line to
  keep your own version.
- The publish job installs npm dependencies without running their scripts, which would otherwise
  see its OIDC token.

`meteroid-oss/perseid/release` takes:

| Input | `sdk-release.yml` passes | Description |
|---|---|---|
| `app-id`, `app-private-key` | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | A GitHub App whose token, minted for this repository with Contents and Pull requests write, runs release-please |
| `release-token` | `SDK_GITHUB_TOKEN` secret | The token for release-please without an App |
| `token` | | Otherwise, the default `GITHUB_TOKEN` |
| `ci-workflows` | `SDK_CI_WORKFLOWS` variable, else `sdk-ci.yml` | Workflow files dispatched on the release PR branch when it was pushed with the default token |
| `path` | the `path` of a manual run | A package to publish again, skipping release-please |

It outputs `paths` (JSON array of the released package paths), `tags` (their release tags, by
path) and `releases` (release-please's outputs, as JSON). With the default token, release PRs get
no CI unless dispatched, and auto-merged release PRs publish nothing.

### Auto-merge

`--auto-merge` (`auto-merge: true`) enables GitHub auto-merge (squash) on the SDK pull requests.
It also labels them `perseid:auto-release`, so the release action auto-merges the release PR they
lead to. `auto_merge = true` in `perseid.toml` makes `perseid init` write it into `sdks.yml`. It
needs:

- "Allow auto-merge" in the repository settings;
- required status checks, through branch protection or rulesets. Without any, GitHub merges at
  once;
- checks that run on the pull requests: `SDK_GITHUB_TOKEN` or the App, or `ci-workflows` plus a
  required commit status (see [the default `GITHUB_TOKEN`](#the-default-github_token));
- `SDK_GITHUB_TOKEN` or the App for both `sdks.yml` and `sdk-release.yml`. A merge made with the
  default `GITHUB_TOKEN` triggers no workflow, so nothing would be released.

## Targets

A `[targets.<name>]` table ([keys](configuration.md#targets)) makes perseid write into the folder
`path` of the repository `repo`, through a pull request from `perseid/targets/<name>`. A `docs`
target writes the spec and the [docs data](customizing.md#docs-data) of every SDK there, for a
docs site that shows the API in the reader's SDK language:

```toml
[targets.docs]
repo = "acme/docs"
path = "api"     # the default
after = "sdks"   # the default
```

```
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-typescript, acme/api-python
                         ▲                       │ released
                         └──── perseid-released ─┘
acme/api-sdks ──PR──▶ acme/docs (api/)
```

perseid owns `path`: each pull request replaces the folder whole, deleting the files it no longer
writes, and changes nothing outside it. The repository needs no setup beyond the App, or token,
of `sdks.yml` reaching it: `perseid sync` installs the perseid App there, and `perseid status`
reports a target repository it can't reach.

A [pack](customizing.md#packs) target generates a program wrapping one SDK, such as a CLI over the
Rust SDK, in a repository of its own, once that SDK is released:

```toml
[targets.cli]
pack = "packs/cli"   # the pack's folder, relative to perseid.toml
wraps = "rust"
repo = "acme/acme-cli"
```

Its templates see the API as the Rust SDK's templates do, and the version of the Rust SDK the
program depends on: the released one. Its pull requests replace the files it generated, deleting
those it no longer generates, and write its scaffold once. The repository releases it on its own,
with what the pack's scaffold sets up: perseid writes no release workflow there, but `perseid sync`
gives it the release credentials of the SDK repositories (your App's `SDK_APP_ID` and
`SDK_APP_PRIVATE_KEY`, or a check that `SDK_GITHUB_TOKEN` is set). Since CI tokens
can't push workflows, the pull request leaves out those of the scaffold, and its description lists
them: write them with `perseid targets cli --out <dir>` and commit them by hand. In the Action,
`pack` must be a folder of the checkout, such as a submodule: perseid doesn't fetch packs yet.

### `after`

- `after = "generate"`: `generate --pr` opens or updates the target's pull request right after
  the SDK pull requests. `version` is `null` in `docs-data.json` for an SDK that run changes.
- `after = "sdks"`: the pull request opens once every SDK generated from the current spec is
  released, and `docs-data.json` holds the `version` of each.
- `after = "<language>"`, such as `"rust"`: the same, waiting for that SDK only. A pack target
  waits for the SDK it wraps by default, and can't wait for another alone.

With `after = "sdks"`:

1. `sdks.yml` opens the SDK pull requests of a new spec, as without targets.
2. Each SDK repository's `sdk-release.yml` releases and publishes the SDK once its pull request
   is merged, then its `report` job sends a `perseid-released` `repository_dispatch` event to the
   repository holding `perseid.toml`, with `{repo, sha, releases: [{path, tag, version}]}`.
3. `sdks.yml` runs on that event as on a spec change: `generate --pr`, which changes nothing in
   SDKs already up to date, then the targets. Once every SDK is released, the target's pull
   request opens.

A spec change that leaves every SDK as it is opens the target's pull request in the run of the
spec change. Each run decides anew, so a missed event only delays the pull request until the
next run, and a run with nothing new changes nothing.

`perseid init` adds the `repository_dispatch` trigger to `sdks.yml`, and `perseid init` and
`perseid sync` add the `report` job to `sdk-release.yml`, once a target waits for releases
(`after` other than `"generate"`). Run
both after adding one.

### When an SDK is released

`generate` records a digest of the files it generates in `.perseid/generation.json`, in each
SDK's folder, rewritten only along with them. An SDK is released when, in its repository:

- no `perseid/update` or `perseid/update-<language>` pull request is open;
- its `.perseid/generation.json` on the default branch is the one at its latest release tag, the
  tag of the version `.release-please-manifest.json` holds, as `release-please-config.json`
  names it.

Since every run regenerates from the current spec first, an SDK the spec change doesn't touch
stays released, without a pull request or a release. A digest of the spec would need a pull
request and a release of every SDK on each spec change to record it. SDKs generated before
perseid recorded it count as released until their next change.

### The pull request

- Its title is a conventional commit sized like an SDK pull request's, comparing the spec with the
  `openapi.json` already in `path`: `feat(api): update the API reference to Acme 1.2.0`. A pack
  compares it with its `.perseid/openapi.json`: `feat(api): update to Acme 1.2.0`.
- Its description lists the files changed, the API changes, the version of each SDK (of the SDK
  it wraps, for a pack) and where the spec comes from.
- `auto-merge: true` enables auto-merge on it as on the SDK pull requests.

### Credentials of the report

`meteroid-oss/perseid/report` sends the event with the first credential it finds:

| Credential | Notes |
|---|---|
| `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | A token of your App, minted for the repository holding `perseid.toml`, where it must be installed |
| `SDK_GITHUB_TOKEN` secret | It needs Contents write on the repository holding `perseid.toml` |
| The perseid App | Installed on both repositories |
| The default token | When the SDKs live in the repository holding `perseid.toml` |

Without one reaching that repository, the job leaves a notice and succeeds: the targets wait for
the next run of `sdks.yml`.

### `perseid targets`

`perseid targets` writes the targets on their own. Names after the command limit it to those.

| Flag | Effect |
|---|---|
| `--out <dir>` | Writes each target to `<dir>/<name>`, without cloning its repository. `version` is `null`, and a pack depends on the version of the SDK where `generate` writes it |
| `--check` | With `--out`, fails when the targets there differ from what the spec gives, without writing |
| `--no-format` | Skips the formatters of packs |
| `--pr` | Opens or updates the pull request of each target whose `after` holds, as `generate --pr` does after the SDK pull requests |
| `--spec <path\|url>` | Reads another spec |
| `--bump`, `--auto-merge`, `--relax-enum-additions` | As for `generate --pr` |

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
