# CI and releases

## Repository layouts

`perseid.toml` says where the spec comes from and where the SDKs live; `perseid setup` makes
GitHub match it. Which one to pick: the SDKs next to the API unless other teams own them, one
repository per language when each ecosystem wants its own home, a separate SDKs repository when
the API repository must not grant anything beyond sending its spec.

**Next to the API** (the default): every SDK is a folder of the API repository.

```
acme/api ──PRs──▶ acme/api (typescript/, python/)
```

```toml
spec = "openapi.json"
```

**One repository per language**, orchestrated from the API repository: its workflow generates
every SDK and opens their pull requests with a GitHub App installed on all of them. `{lang}`
stands for `node`, `python`, `go`, `java`, `rust` or `dotnet`; a `repo` without it is one
repository holding every SDK in a folder named after its language.

```
acme/api ──PRs──▶ acme/api-node, acme/api-python
```

```toml
spec = "openapi.json"
repo = "acme/api-{lang}"     # or "acme/api-sdks": one repository, a folder per SDK
```

**A separate SDKs repository** receiving the spec: the API repository only pushes its spec there,
over SSH with a deploy key that can write to that one repository; the SDKs repository owns its
own `perseid.toml`, generates the SDKs and never gets any access to the API repository. The SDKs
live in it, or in one repository per language that it orchestrates with a GitHub App (installed
on those repositories only, never on the API repository).

```
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-sdks (typescript/, python/)
acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-node, acme/api-python
```

```toml
# acme/api: perseid.toml
spec = "openapi.json"
push_spec = "acme/api-sdks"
generate = "npm run openapi"   # only when the spec isn't committed: how CI writes it

# acme/api-sdks: perseid.toml, which `perseid setup` writes when it creates the repository
spec = "github:acme/api/openapi.json"   # kept here as openapi.json, with .perseid/source.json
repo = "acme/api-{lang}"                # only for one repository per language
```

`perseid setup` works from either side: in the API repository it creates or completes the SDKs
repository from the SDK tables of its own `perseid.toml`; in the SDKs repository it sets up the
API side remotely, which takes admin rights on the API repository.

**A spec at a URL**: `spec = "https://api.acme.com/openapi.json"` needs nothing from the API
side. `sdks.yml` fetches it every day (and on demand), and opens pull requests when it changed;
their description names the URL and a digest of what it served.

`perseid.toml` with `spec = "github:…"` generates from the snapshot, and its pull requests say
which commit it comes from: "Generated from acme/api@a1b2c3d".

## GitHub Action

```yaml
# .github/workflows/sdks.yml in the repository that owns openapi.json
on:
  push:
    branches: [main]
    paths: [openapi.json, perseid.toml]
jobs:
  sdks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: meteroid-oss/perseid@v0
        with:
          token: ${{ secrets.SDK_TOKEN }}   # contents + pull requests write on the SDK repositories
```

The Action installs the perseid version matching its own ref and runs `generate --pr`: it commits
to `perseid/update` and opens or updates a pull request, in this repository or in the SDK
repositories named by `repo`. For pull request checks, pass `command: generate --check`, which
fails on drift. See [Tokens](#tokens) for which `token` to pass.

## Quick setup: `perseid setup`

You need:

- Node 18+ for `npx perseid`, or the `curl` install;
- on the account that will own the SDK repositories: permission to create repositories and, in an
  organization, to create GitHub Apps (organization owner or GitHub App manager);
- with a separate SDKs repository: admin rights on it and on the API repository;
- for each SDK you publish, an account on its registry (npm, PyPI, crates.io, Maven Central,
  NuGet) to set up trusted publishing at the end.

`perseid init` writes `perseid.toml`, with the [layouts](#repository-layouts) as commented
examples. Then, in the same clone, `perseid setup` signs in with `GH_TOKEN`, the `gh` CLI's token
or a browser login (it stores no token), compares `perseid.toml` with GitHub and prints the plan:
`+` to add, `~` to change, `=` already in place, and warnings such as generated paths taken by
files perseid didn't write. Once confirmed, it:

1. creates the missing repositories, as private or public as the one it runs in, and reuses
   existing ones: it only adds missing files, merges release-please packages into an existing
   `release-please-config.json`, and never rewrites your settings in an existing `perseid.toml`
   (only `spec`, for a repository that starts receiving the spec);
2. commits `sdk-release.yml` and the release-please files to each SDK repository with your
   token, so the App needs no permission on workflow files;
3. creates a GitHub App in your browser (Contents and Pull requests write, no webhook) when SDKs
   live in other repositories than the workflow's, stores its `SDK_APP_ID` variable and
   `SDK_APP_PRIVATE_KEY` secret where they are used, and waits while you install it;
4. for a separate SDKs repository: registers a write deploy key on it, stores its private half as
   the `PERSEID_SDKS_DEPLOY_KEY` secret of the API repository with the `PERSEID_SDKS_REPO`
   variable, and lets GitHub Actions open pull requests in the SDKs repository when it holds
   every SDK (its workflow then uses the default token);
5. opens a pull request adding `.github/workflows/sdks.yml` where the SDKs are generated, and
   `.github/workflows/perseid-push.yml` in the API repository (a new, empty SDKs repository gets
   its files committed directly), and stages the files it adds to your clone;
6. lists the pull requests to merge, the SDKs repository's first, and what each registry needs.

Running it again changes nothing once in sync. `--dry-run` prints the plan and exits with 2 when
changes are pending (0 otherwise), `--yes` applies without asking, and `--no-browser` prints URLs
instead of opening them.

`perseid status`, from the API repository or an SDKs repository, runs the same comparison
without changing anything, then checks how the automation fares: the last spec synced (commit and
age, and whether a newer spec commit is waiting), open `perseid/update` pull requests, the last
`sdks.yml` and `perseid-push.yml` runs and the App installation. Each problem comes with the fix;
it exits with 1 when something needs you. Without GitHub credentials it checks the clone only.

### Spec pushes

`perseid-push.yml` runs on the API repository's default branch when the spec changes (on every
push with `generate`, after running it). It clones the SDKs repository over SSH, pinned to
GitHub's published host keys, and commits the spec as `spec: acme/api@a1b2c3d` with
`.perseid/source.json` naming the commit, unless:

- the SDKs repository has no `.perseid/source.json` yet: its setup pull request isn't merged,
  run the workflow again once it is;
- the synced commit isn't an ancestor of the pushed one: an older or diverged commit never
  overwrites a newer spec (to resync after rewriting history, remove `sha` from
  `.perseid/source.json`);
- the spec didn't change.

Runs share the `perseid-push` concurrency group without cancelling each other, so pushes happen
one at a time and in order. The deploy key reaches the SDKs repository only, and nothing on the SDKs
side can read or write the API repository. The rest of this page is the setup by hand.

## Tokens

Events caused by the default `GITHUB_TOKEN` start no workflow, except `workflow_dispatch` and
`repository_dispatch`; pull requests it opens or updates only get `pull_request` runs waiting for
"Approve workflows to run"
([GitHub docs](https://docs.github.com/en/actions/concepts/security/github_token)).

**SDKs in this repository**: no token needed. Give the job `contents: write`,
`pull-requests: write` and `actions: write`, and list your CI workflows in `ci-workflows`: with the
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
  requests read and write, stored as a secret (`SDK_TOKEN` above);
- or, for organizations, your own GitHub App, whose installation tokens expire after an hour and are
  therefore minted in the workflow, never stored:
  1. create a GitHub App (organization settings, Developer settings, GitHub Apps), webhook off,
     with repository permissions Contents and Pull requests read and write;
  2. install it on the spec and SDK repositories;
  3. store its App ID as the `SDK_APP_ID` variable and a generated private key as the
     `SDK_APP_PRIVATE_KEY` secret;
  4. mint the token in the job:

```yaml
    steps:
      - uses: actions/checkout@v4
      - id: app
        if: vars.SDK_APP_ID != ''
        uses: actions/create-github-app-token@v2
        with:
          app-id: ${{ vars.SDK_APP_ID }}
          private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}
          owner: ${{ github.repository_owner }}
          repositories: acme-node,acme-python   # the SDK repositories
      - uses: meteroid-oss/perseid@v0
        with:
          token: ${{ steps.app.outputs.token || secrets.SDK_TOKEN }}
```

Pull requests and merges made with either token start workflows like a person's would, so SDK CI,
auto-merge and releases work without dispatching.

**Releases**: the scaffolded `sdk-release.yml` mints the same way when `SDK_APP_ID` (variable or
secret) and `SDK_APP_PRIVATE_KEY` are set, scoped to its own repository, and otherwise uses the
`RELEASE_TOKEN` secret (a fine-grained personal access token) or the default token. With the
default token, release PRs get no CI unless dispatched, and auto-merged release PRs publish nothing.

**Repository settings**: "Allow GitHub Actions to create and approve pull requests" (Settings,
Actions, General) is needed only when perseid or release-please run with the default token. It
also lets any workflow approve pull requests, which can satisfy a required review in branch
protection with no human in the loop, so keep "Workflow permissions" read-only by default and grant
write per job, as the examples do.

## Self-hosted runners and other CIs

`ghcr.io/meteroid-oss/perseid` bundles perseid, git, gh and every pinned formatter.

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

## Releases

Every SDK keeps its own version, in its own manifest, and is released on its own. `perseid init`
also scaffolds [release-please](https://github.com/googleapis/release-please)
(`release-please-config.json`) and `.github/workflows/sdk-release.yml` (skip them with
`--no-release`), and gives SDK repositories without a manifest the same on their first pull request:

1. The Action titles its pull request as a conventional commit sized by
   [oasdiff](https://github.com/oasdiff/oasdiff): `feat(api)!:` for breaking API changes,
   `feat(api):` for other API changes, `fix(api):` otherwise (`bump` input, `--bump` flag).
2. Merging it, or any `fix:`/`feat:` commit touching an SDK, opens a release PR bumping the
   touched SDKs and their changelogs. Before 1.0, breaking changes bump the minor version.
3. Merging the release PR tags each SDK (`rust/v0.4.0`, or `v0.4.0` alone in its repository) and
   publishes it through `meteroid-oss/perseid/publish`: trusted publishing (OIDC) for crates.io,
   npm and PyPI, a Central Portal token and GPG key for Maven Central, an API key for NuGet, the
   module proxy for Go. Versions already on the registry are skipped, so re-running a failed job is safe.

`relax-enum-additions` (default `true`) counts enum values added to responses as minor changes.
That is only safe while the SDKs accept unknown enum values, which generated SDKs do.

`auto-merge: true` enables GitHub auto-merge on the generated pull requests and, through their
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
`npx` or `uvx`. The Action reads the languages from `perseid.toml` and installs only the pinned
native formatters they need: no JVM, Node or Python setup (`csharpier` installs as a .NET tool).
