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
fails on drift.

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
- a GitHub App token as the Action's `token` and as the `RELEASE_TOKEN` secret, since merges made
  with the default `GITHUB_TOKEN` trigger no workflow, so nothing would be released or published.

## Formatting

Output goes through `rustfmt`, `biome`, `ruff`, `gofmt`, `google-java-format` and `csharpier`,
with your SDK's own formatter configuration. Locally, missing `biome` or `ruff` run pinned through
`npx` or `uvx`. The Action reads the languages from `perseid.toml` and installs only the pinned
native formatters they need: no JVM, Node or Python setup (`csharpier` installs as a .NET tool).
