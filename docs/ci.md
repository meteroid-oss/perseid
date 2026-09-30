# CI and releases

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

## Quick setup: `perseid init --github`

Run in a clone of the repository holding the spec, `perseid init --github` sets up everything
[Tokens](#tokens) describes, with a GitHub App, so that pushing a spec change opens SDK pull requests:

1. signs in with `GH_TOKEN`, the `gh` CLI's token or a browser login, and stores no token;
2. offers one repository per SDK (`acme-node`, `acme-python`...), writes their `repo` to
   `perseid.toml` and creates the missing ones, as private or public as the spec repository;
3. commits `sdk-release.yml` and the release-please files to each SDK repository, with your
   token, so the App needs no permission on workflow files;
4. creates a GitHub App in your browser (Contents and Pull requests write, no webhook) and stores
   its `SDK_APP_ID` variable and `SDK_APP_PRIVATE_KEY` secret in the spec and SDK repositories;
5. waits while you install the App on them;
6. opens a pull request adding `.github/workflows/sdks.yml`, which mints the App's token and runs
   the Action (`--push` commits it to the default branch instead);
7. lists what each registry still needs.

Re-running it resumes where it stopped and reuses the repositories, files, App and pull request
already there. `--yes` takes every default, `--no-browser` prints URLs instead of opening them, and
`--github-owner` puts the SDK repositories and the App under another account. The rest of this page
is the same setup by hand.

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

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration. Locally, missing `biome` or `ruff` run pinned through
`npx` or `uvx`. The Action reads the languages from `perseid.toml` and installs only the pinned
native formatters they need: no JVM, Node or Python setup (`csharpier` installs as a .NET tool).
