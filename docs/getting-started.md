# From OpenAPI to automatic SDK update PRs

Download and extract the binary for your platform from [Perseid releases](https://github.com/meteroid-oss/perseid/releases).
No Rust, Python, Docker, or formatter installation is needed to bootstrap.

> Maintainers: the first binary release needs the [release workflow](../maintainer-workflows/README.md)
> activated first. Until that release exists, the download and generated CI are not available.

## 1. Bootstrap

For a private `acme/backend` with a committed OpenAPI file, and an empty
`acme/clients` repository checked out alongside it:

```sh
./perseid init --name acme \
  --language rust --language typescript \
  --output ../clients --repository acme/clients \
  --source-repository acme/backend --source-ref main \
  --spec apis/generated/openapi.json \
  --backend-directory ../backend \
  --base-url https://api.acme.com
```

This creates the config, SDK packages and shared support files, local override
config, and the orchestration workflows. Standard schema preparation is enabled
for OpenAPI 3.0/3.1 JSON; unsupported operations fail generation instead of being
silently omitted. It also writes the notification workflow
into the backend checkout. Templates ship inside the binary; init needs no network
access and refuses to overwrite existing files. Use any combination of `rust`,
`typescript`, `python`, `java`, and `go` (Go also needs `--go-module YOUR_IMPORT_PATH`).

## 2. Give GitHub access, then push

Set the Actions secret **`PERSEID_TOKEN`** in both repositories. For the simplest
setup, use a fine-grained token with access to these repositories and **Contents:
read/write** plus **Pull requests: read/write**. Include any external SDK repositories
it will update. The backend only needs permission to dispatch to the clients repo;
you can give it a separate, narrower token. Approve the token for your organization
if required. A GitHub App token can replace this secret for larger installations.

Commit and push the files created in **both** checkouts to `main`. The clients
repository's **Update SDKs** workflow opens the first generation PR. Review and
merge it. No generation or tool installation is needed on your machine: CI installs
only the selected languages' tools and downloads the pinned Perseid release.

## 3. Change your API

```text
Backend main: committed OpenAPI changes
    → notify clients repository
    → clients CI syncs the spec, generates/formats/checks SDKs
    → opens or updates SDK PRs
    → you review and merge
```

The backend exports and commits OpenAPI using its existing tooling. The
orchestration repository owns the snapshot, generation, and delivery. Its PR checks
use the committed snapshot without access to the private backend. You can also run
**Update SDKs** manually. The generated workflows assume the clients default branch
is `main`; edit them if yours differs.

If your spec is already in the clients repo, omit `--source-repository`,
`--source-ref`, `--backend-directory`, and `--repository`; `--spec` is then a local
path. Commit that file too. `PERSEID_TOKEN` is recommended for automatic PR checks;
the workflow can fall back to `GITHUB_TOKEN`, subject to GitHub's PR/workflow approval
settings and the repository's “Allow GitHub Actions to create and approve pull
requests” setting.

## Customizing and releasing

SDK settings live in `.perseid/overrides.toml`. Edit `perseid.toml` to put targets in
[separate repositories](orchestration.md); bootstrap each destination with
`perseid init --sdk-only`. Use `--no-workflows` for a local-only project. Local
formatted generation needs formatters on PATH, or use the bundled Docker image.

SDK updates are PRs, not automatic package releases. Each SDK owns its version;
`perseid version` records an editable bump request. Publishing requires that SDK's
registry hooks and credentials. Automatic `oasdiff` bump recommendations are not
implemented yet. See [versioning and releases](orchestration.md) for the commands,
extension points, and CI-only spec sources.
