# CI and releases

## Repository layouts

`perseid.toml` lives in the repository that holds the SDKs, or orchestrates them: it lists the SDKs
(`sdks`), where they live (`repo`) and the spec they are generated from (`spec`). `perseid init`
writes the workflows it calls for, which you commit, and `perseid setup-github` sets up what files
can't hold, once you agree. It never says where the spec comes from: when another repository holds the
spec, `perseid connect` run there pushes it here.

Which one to pick: the SDKs next to the API unless other teams own them, one repository per
language when each ecosystem wants its own home, a separate SDKs repository when the API repository
must not grant anything beyond sending its spec.

**Next to the API**: every SDK is a folder of the repository holding the spec.

```
acme/api ──PRs──▶ acme/api (typescript/, python/)
```

```toml
spec = "openapi.json"
sdks = ["typescript", "python"]
```

**One repository per language**, orchestrated from the repository holding `perseid.toml`: its
workflow generates every SDK and opens their pull requests with a GitHub App installed on all of
them. `{lang}` stands for the language as `sdks` names it; a `repo` without it is one repository
holding every SDK in a folder named after its language.

```
acme/api ──PRs──▶ acme/api-typescript, acme/api-python
```

```toml
sdks = ["typescript", "python"]
repo = "acme/api-{lang}"     # or "acme/api-sdks": one repository, a folder per SDK
```

**A separate SDKs repository** receiving the spec: the API repository only pushes its spec there,
over SSH with a deploy key that can write to that one repository; the SDKs repository generates the
SDKs and never gets any access to the API repository. The SDKs live in it, or in one repository per
language that it orchestrates with a GitHub App (installed on those repositories only, never on the
API repository).

```
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-sdks (typescript/, python/)
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-typescript, acme/api-python
```

```sh
# in acme/api-sdks
npx perseid init                  # perseid.toml (spec = "openapi.json", sdks, repo) and workflows
git add -A && git commit -m "ci: generate the SDKs with perseid" && git push
npx perseid setup-github          # SDK repositories and the App, once you agree
# in acme/api, once perseid.toml is on acme/api-sdks's default branch
npx perseid connect acme/api-sdks # deploy key, perseid-push.yml to commit
```

`perseid connect` checks `perseid.toml` of the SDKs repository, and writes
`.github/workflows/perseid-push.yml` in your clone, for you to review and commit: a trigger, an
optional build step and the `meteroid-oss/perseid/push` action, whose spec path and SDKs
repository are plain values you can edit; run `connect` again to change them. Pull requests opened from
a pushed spec say which commit it comes from: "Generated from acme/api@a1b2c3d", or
"acme/api@v1.4.0 (a1b2c3d)" when a release or tag pushed it (with `--private`, the commit alone,
without the API repository's name).

**A spec at a URL**: `spec = "https://api.acme.com/openapi.json"` needs nothing from the API
side. `sdks.yml` fetches it every day (and on demand), and opens pull requests when it changed;
their description names the URL and a digest of what it served.

## GitHub Action

`perseid init` writes `.github/workflows/sdks.yml`, the same with or without a GitHub App. It
pins the release line, `@v0.6`: before 1.0 a minor release may break, and `perseid init` writes
the line of the perseid that wrote the workflow, updating it when you run it again with a newer one.

```yaml
name: SDKs

on:
  push:
    branches: ["main"]
    paths: ["openapi.json","perseid.toml",".github/workflows/sdks.yml"]
  workflow_dispatch:

permissions:
  contents: write
  pull-requests: write

concurrency: sdks

jobs:
  sdks:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
      - if: hashFiles('openapi.json') != ''
        uses: meteroid-oss/perseid@v0.6
        with:
          app-id: ${{ vars.SDK_APP_ID }}
          app-private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}
```

The Action installs the perseid version matching its own ref, then:

1. `perseid tools list --github-output` reads the SDKs of `perseid.toml`, for the toolchains to set
   up (Rust, Go, .NET), and the account and repositories of the App token;
2. `perseid tools install` downloads the pinned formatters they need and oasdiff;
3. with `app-id`, it mints a token of that App, for the repository holding `perseid.toml` and the
   SDK repositories of the App's account, with Contents and Pull requests write only;
4. it runs `generate --pr`: perseid commits the generated files on top of the branch it runs on
   to `perseid/update` and opens or updates a pull request, in this repository or in the SDK
   repositories named by `repo`, sized and described as [Releases](#releases) says.

| Input | Default | |
|---|---|---|
| `command` | `generate --pr` | arguments passed to perseid: `generate --check` fails on drift, for pull request checks |
| `app-id`, `app-private-key` | | a GitHub App whose token the Action mints and uses instead of `token` |
| `token` | `github.token` | without an App, a token with Contents and Pull requests write on every target repository (see [Tokens](#tokens)) |
| `working-directory` | `.` | the directory holding `perseid.toml` |
| `version` | the Action's ref | the perseid version to run |
| `bump` | `auto` | `--bump`: `auto`, `major`, `minor` or `patch` |
| `base-spec` | | `--base-spec`: the previous spec `auto` compares with |
| `relax-enum-additions` | `true` | `--relax-enum-additions` |
| `auto-merge` | `false` | `--auto-merge` |
| `ci-workflows` | | `--dispatch`, with the default token only |

The inputs reach `perseid generate` as `PERSEID_BUMP`, `PERSEID_BASE_SPEC`,
`PERSEID_RELAX_ENUM_ADDITIONS`, `PERSEID_AUTO_MERGE` and `PERSEID_DISPATCH`, which it reads like
its flags.

`generate --pr` also works locally: it copies the generated files, the spec and a new SDK's
skeleton into a temporary worktree of `origin/<your branch>` (the default branch when yours isn't
pushed) and commits there, so your branch, index, handwritten changes and unpushed commits stay
out of the pull request. Without a git identity configured, commits are
authored by `github-actions[bot]`.

## Quick setup: `perseid init`, `perseid setup-github` and `perseid connect`

You need:

- Node 18+ for `npx perseid`, or the `curl` install;
- on the account that will own the SDK repositories: permission to create repositories and, in an
  organization, to create GitHub Apps (organization owner or GitHub App manager);
- with a separate SDKs repository: admin rights on the API repository (for its secret and
  variable) and, ideally, on the SDKs repository (for its deploy key);
- for each SDK you publish, an account on its registry (npm, PyPI, crates.io, Maven Central,
  NuGet) to set up trusted publishing at the end.

`perseid init` works in your clone only: it finds the spec of the repository (`git ls-files` for an
`openapi`/`swagger` JSON or YAML file holding an `openapi:` or `swagger:` key), asks which SDKs to
generate, where they live and the client name, then writes `perseid.toml`, prints the package each
SDK publishes, and writes the workflows: `.github/workflows/sdks.yml` and, for the SDKs living
here, `sdk-release.yml` and the release-please files. Commit and push them: the workflows run from
the default branch. Run `init` again after editing `perseid.toml` to refresh them; it rewrites the
workflows it wrote, and only warns about one whose first line you removed. Without a terminal,
pass `--sdks`, and optionally `--repo`, `--spec` and `--name`. Without a spec, `spec` is `openapi.json`, which
`perseid connect` will push; `perseid generate --spec <path|url>` previews the SDKs meanwhile.
`perseid generate` starts each SDK from its package skeleton (manifest, README, errors) the first
time, and prints where each one went. An SDK with its own `repo` is generated into a shallow clone
of it under `.perseid/repos/<owner>/<name>`, reset on each run: changes there perseid didn't generate stop `generate`
unless `--pr` is passed. Before `perseid setup-github` creates those repositories,
`perseid generate --out <dir>` previews every SDK in `<dir>/<language>` (or `<dir>/<path>`) without cloning anything,
and `--out <dir> --check` compares against that directory.

Then, in the same clone, `perseid setup-github` checks `perseid.toml`, signs in with `GH_TOKEN`,
the `gh` CLI's token or a browser login (it stores no token), compares it with GitHub and prints
the plan: `+` to add, `~` to change, `=` already in place, and warnings such as generated paths
taken by files perseid didn't write, or workflows not pushed yet. Once you agree, it:

1. creates the missing SDK repositories, as private or public as the one it runs in;
2. adds `sdk-release.yml` and the release-please files to each SDK repository: committed to a
   new, empty one, through a pull request to an existing one, merging release-please packages into
   an existing `release-please-config.json` (`release = false` in `perseid.toml` leaves them out);
3. creates a GitHub App in your browser (Contents and Pull requests write, no webhook), stores its
   `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret where they are used, and waits while you
   install it. Pull requests the App opens run your CI. With every SDK here, `--no-app` lets GitHub
   Actions open them with the default token instead, which needs no credentials but runs no CI on
   them;
4. lists the pull requests to merge, what each registry needs and, when the spec isn't in the
   repository yet, the `perseid connect` to run in the one holding it.

Declining changes nothing and prints how to make each change yourself. `perseid setup-github`
never touches the API repository. `perseid connect <owner/sdks-repo>`, run in it,
does: it finds its spec (or `--spec <path>`; `--build "<command>"` when CI writes it instead of
committing it), asks when to push it, proposing `release` when the repository publishes GitHub
releases and `change` otherwise (`--on change|release|tag`, `--tags` for `tag`), then plans:

1. a write deploy key on the SDKs repository, its private half the `PERSEID_SDKS_DEPLOY_KEY`
   secret of the API repository (generated in memory, never written to disk); without admin
   rights on the SDKs repository, it prints the public key and where an admin of it adds it;
2. `.github/workflows/perseid-push.yml`, written in your clone: commit and push it.

The key and the secret are its only changes on GitHub, made once you agree; declining prints the
`ssh-keygen` and `gh` commands to add them yourself. `--auth token` pushes with a fine-grained
token in the `PERSEID_SDKS_TOKEN` secret instead, and `--auth app` with a token of a GitHub App
installed on the SDKs repository only (`PERSEID_PUSH_APP_ID` variable,
`PERSEID_PUSH_APP_PRIVATE_KEY` secret), minted for that repository and Contents write only:
`connect` then changes nothing on GitHub and says what to add.

Running either again changes nothing once in sync, and `connect` keeps the key and the settings it
isn't given. `--dry-run` prints the plan and exits with 2 when changes are pending (0 otherwise),
`--yes` applies without asking, and `--no-browser` prints URLs instead of opening them.

`perseid status` runs the same comparison without changing anything, then checks how the
automation fares. In the SDKs repository: the last spec pushed (commit, release and age), open
`perseid/update` pull requests, the last `sdks.yml` run and the App installation. In the API
repository, without a `perseid.toml`: the key and secret `perseid-push.yml` needs, the
last spec the SDKs repository received and the last `perseid-push.yml` run. Each problem comes with
the fix. It exits with 2 when something waits on you (changes to apply, workflows to refresh with
`perseid init` or to push), 1 on errors, 0 otherwise. Without GitHub credentials it checks the
clone only.

### Spec pushes

`perseid-push.yml` runs on `--on`:

- `change` (the default without GitHub releases): on the API repository's default branch when the
  spec changes (on every push with `--build`, after running it);
- `release`: when a GitHub release is published;
- `tag`: when a tag matching `--tags` (`v*` by default) is pushed.

It can also be run by hand, from the default branch or a tag. Its action runs
`perseid push-spec <spec> --to <owner/sdks-repo>`, which takes the spec of the commit that
triggered it, clones the SDKs repository over SSH with the deploy key, pinned to GitHub's published
host keys, and commits the spec to the `spec` path of the SDKs repository's `perseid.toml` as `spec: acme/api@a1b2c3d`
(`spec: acme/api@v1.4.0 (a1b2c3d)` for a release or tag) with `.perseid/source.json` naming the
commit, and the tag as `ref`, unless:

- the SDKs repository has no `perseid.toml` yet: run `perseid init` there and push it, then run the
  workflow again;
- the pushed commit is older than the synced one: an older spec never overwrites a newer one;
- the spec didn't change.

When the pushed commit and the synced one have diverged (a release cut off the default branch's
history, or history rewritten), the run fails rather than skipping every later push: remove `sha`
from the SDKs repository's `.perseid/source.json` to accept the pushed spec. A `.perseid/source.json`
naming another repository, after the spec moved, is replaced.

Runs share the `perseid-push` concurrency group without cancelling a running push, so pushes happen
one at a time and in order. The deploy key reaches the SDKs repository only, and nothing on the SDKs
side can read or write the API repository. With `--private`, `.perseid/source.json` and the commit
messages leave out the API repository's name.

**Push the spec on release.** To ship SDKs for what you released rather than for every merged
change, run `perseid connect <owner/sdks-repo> --on release`: it rewrites `perseid-push.yml` to run
on `release: published`. Each release pushes its spec, and the SDKs repository opens its
`perseid/update` pull request naming the release ("Generated from acme/api@v1.4.0 (a1b2c3d)").
With `--on tag`, any matching tag does the same, release or not.

The rest of this page is the setup by hand.

## Tokens

Events caused by the default `GITHUB_TOKEN` start no workflow, except `workflow_dispatch` and
`repository_dispatch`; pull requests it opens or updates only get `pull_request` runs waiting for
"Approve workflows to run"
([GitHub docs](https://docs.github.com/en/actions/concepts/security/github_token)).

**SDKs in this repository**: no token needed. Give the job `contents: write`,
`pull-requests: write` and `actions: write`, and list your CI workflows in `ci-workflows` (`--dispatch`): with the
default token, the Action dispatches them on `perseid/update` after opening or updating the pull
request. Each needs `on: workflow_dispatch`. Checks of dispatched runs show on the branch and in
the Actions tab, but neither in the pull request's checks nor as required status checks, which only
count runs from `push`, `pull_request`, `pull_request_review`, `pull_request_target`, `deployment` and
`deployment_status`
([GitHub docs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)).
Approving the waiting `pull_request` runs by hand does count. To gate merges on dispatched runs,
have the workflow post a commit status and require that context:

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

The scaffolded `sdk-release.yml` does the same for release PRs when the `SDK_CI_WORKFLOWS`
repository variable lists workflow files and no other token is configured.

**SDKs in other repositories**: the default token cannot reach them. Either:

- a fine-grained personal access token limited to the SDK repositories, with Contents and Pull
  requests read and write, stored as a secret and passed as `token`;
- or, for organizations, your own GitHub App, whose installation tokens expire after an hour and are
  therefore minted in the workflow, never stored:
  1. create a GitHub App (organization settings, Developer settings, GitHub Apps), webhook off,
     with repository permissions Contents and Pull requests read and write;
  2. install it on the spec and SDK repositories;
  3. store its App ID as the `SDK_APP_ID` variable and a generated private key as the
     `SDK_APP_PRIVATE_KEY` secret;
  4. pass them to the Action, which mints the token on each run, for the repository holding
     `perseid.toml` and the SDK repositories of the App's account (`perseid tools list
     --github-output` prints them as `owner` and `repositories`), with Contents and Pull requests
     write only; without an `app-id`, it uses `token`:

```yaml
      - uses: meteroid-oss/perseid@v0.6
        with:
          app-id: ${{ vars.SDK_APP_ID }}
          app-private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}
```

Pull requests and merges made with either token start workflows like a person's would, so SDK CI,
auto-merge and releases work without dispatching.

**Releases**: the scaffolded `sdk-release.yml` mints the same way, through
`meteroid-oss/perseid/release`, when the `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret are
set, scoped to its own repository, and otherwise uses the `RELEASE_TOKEN` secret (a fine-grained
personal access token) or the default token. With the default token, release PRs get no CI unless
dispatched, and auto-merged release PRs publish nothing.

**Repository settings**: "Allow GitHub Actions to create and approve pull requests" (Settings,
Actions, General) is needed only when perseid or release-please run with the default token. It
also lets any workflow approve pull requests, which can satisfy a required review in branch
protection with no human in the loop, so keep "Workflow permissions" read-only by default and grant
write per job, as the examples do.

## Self-hosted runners and other CIs

`ghcr.io/meteroid-oss/perseid` bundles perseid, git, gh, oasdiff and every pinned formatter.

```yaml
jobs:
  sdks:
    runs-on: self-hosted
    container: ghcr.io/meteroid-oss/perseid:0
    steps:
      - uses: actions/checkout@v4
      - run: gh auth setup-git && perseid generate --pr
        env:
          GH_TOKEN: ${{ secrets.SDK_TOKEN }}
```

Or anywhere: `docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/work" ghcr.io/meteroid-oss/perseid generate --check`.
Without the image, `perseid tools install [--dir <bin>]` downloads the pinned formatters the SDKs
of `perseid.toml` need (or those of the languages it is given) and oasdiff, next to perseid by
default; `perseid tools list` prints them with their versions. Rust, Go and .NET toolchains bring
`rustfmt`, `gofmt` and the `dotnet` that installs `csharpier`.

## Releases

Every SDK keeps its own version, in its own manifest, and is released on its own. `perseid init`
writes [release-please](https://github.com/googleapis/release-please) (`release-please-config.json`)
and `.github/workflows/sdk-release.yml` to the root of each repository holding SDKs (skip them with
`release = false` in `perseid.toml`). With `perseid.toml` in a folder, the release files still go
at the root, where GitHub and release-please read them, and the release-please packages are named
by their path from the root (`api/typescript`).

`sdk-release.yml` holds two jobs, skipped in forks:

- `release` runs the `meteroid-oss/perseid/release` Action on pushes to the repository's default
  branch, which runs release-please and outputs the paths of the packages it released;
- `publish` runs `meteroid-oss/perseid/publish` once per released path, in the `release`
  environment, the only job with an OIDC token (`id-token: write`). Registries trust this file name
  and this environment, so keep both, and keep this job in `sdk-release.yml`.

Running it by hand publishes one package again, from the selected tag or branch: its `path` input
must name a package of `release-please-config.json` (`.` for the root). `perseid init` rewrites the
file when it differs from what it writes: delete its first line, ``# Written by `perseid init` ``,
to keep your own version (init then only warns). The publish job installs npm dependencies without
running their scripts, which would otherwise see its OIDC token.

`meteroid-oss/perseid/release` takes:

| Input | `sdk-release.yml` passes | Use |
|---|---|---|
| `app-id`, `app-private-key` | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret | GitHub App whose token, minted on each run for this repository with Contents and Pull requests write, runs release-please |
| `release-token` | `RELEASE_TOKEN` secret | token for release-please without an App, such as a fine-grained personal access token |
| `token` | | otherwise, default `GITHUB_TOKEN` |
| `ci-workflows` | `SDK_CI_WORKFLOWS` variable | workflow files dispatched on the release PR branch when it is pushed with the default token, which starts no workflow |
| `path` | the `path` of a manual run | package to publish again instead of running release-please |

It enables auto-merge (squash) on the release PR when the pushed commit came from a pull request
labeled `perseid:auto-release`, and outputs `paths` (JSON array of the released package paths),
`tags` (their release tags, by path) and `releases` (release-please's outputs, as JSON).

Releases go as follows:

1. `generate --pr` titles its pull request as a conventional commit sized by
   [oasdiff](https://github.com/oasdiff/oasdiff): `feat(api)!:` for breaking API changes,
   `feat(api):` for other API changes, `fix(api):` otherwise, and appends oasdiff's changelog to
   its description. It compares the spec with `--base-spec` (`base-spec` input), else with the spec
   file before the pushed commits (`GITHUB_EVENT_BEFORE`, which the Action sets), else at the
   previous commit; without any, or without oasdiff, it asks for a minor release. `--bump
   major|minor|patch` (`bump` input) skips the comparison. An open pull request keeps its largest
   bump.
2. Merging it, or any `fix:`/`feat:` commit touching an SDK, opens a release PR bumping the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK (`rust/v0.4.0`; Go tags carry the module's folder, such as
   `api/go/v0.4.0`, as the module proxy expects; `v0.4.0` alone in its repository) and
   publishes it through `meteroid-oss/perseid/publish`: trusted publishing (OIDC) for crates.io,
   npm and PyPI, a Central Portal token and GPG key for Maven Central, an API key for NuGet, the
   module proxy for Go. Versions already on the registry are skipped, so re-running a failed job is safe.

`--relax-enum-additions` (default `true`) counts enum values added to responses as minor changes.
That is only safe while the SDKs accept unknown enum values, which generated SDKs do.

`--auto-merge` (`auto-merge: true`) enables GitHub auto-merge on the generated pull requests and, through their
`perseid:auto-release` label, on the release PRs they lead to. It needs:

- "Allow auto-merge" in the repository settings;
- required status checks, through branch protection or rulesets: without any, GitHub merges at once;
- checks that run on the pull requests: a non-default token, or `ci-workflows` plus a required
  commit status (see [Tokens](#tokens));
- a non-default token for both the Action and `sdk-release.yml` (a GitHub App through `SDK_APP_ID`,
  or a personal access token): auto-merges enabled with the default `GITHUB_TOKEN` push commits
  that trigger no workflow, so nothing would be released or published.

**Perseid's own releases** are dispatched by `release-please.yml` to `release.yml`, which publishes
the binaries to GitHub and ghcr.io, and the `perseid` npm package with trusted publishing (register
`release.yml` of `meteroid-oss/perseid` as its trusted publisher once). The package holds no binary:
on first run it downloads the release archive for its version, checks it against the checksums
published with it, and caches it.

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration. Locally, missing `biome` or `ruff` run pinned through
`npx` or `uvx`. The Action runs `perseid tools install`, which reads the languages from
`perseid.toml` and downloads only the pinned native formatters they need: no JVM, Node or Python
setup (`csharpier` installs as a .NET tool).
