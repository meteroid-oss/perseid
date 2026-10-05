# Design: a hosted perseid App with no stored secrets

Status: proposal, not deployed.

## Problem

Every cross-repository write needs a credential that someone creates and stores:

| Workflow | Credential |
|---|---|
| The spec repository's push | `SDK_GITHUB_TOKEN`, or a key of the App `perseid app` creates |
| `sdks.yml`, opening pull requests | `SDK_GITHUB_TOKEN`, a fine-grained token that expires, or the `SDK_APP_*` key of the App `perseid app` creates |
| `sdk-release.yml`, running release-please | The same credentials |

A perseid GitHub App would cover all three. Its private key cannot be handed to users, so tokens
come from a service. That service stores no user secrets: it trusts GitHub's OIDC tokens and a
policy committed in the repository being written to.

## Flow

```
workflow (id-token: write)
  │ 1. requests a GitHub OIDC token, audience "perseid"
  ▼
broker (Cloudflare Worker)
  │ 2. verifies the JWT: issuer token.actions.githubusercontent.com, audience, expiry, signature (JWKS)
  │ 3. reads the trust policy of the target repository through the App
  │ 4. checks the token's claims against the policy
  │ 5. mints an installation token: target repository only, the policy's permissions only, 1 hour
  ▼
workflow pushes or opens pull requests with that token
```

The broker holds a single secret, the App's private key, as a Worker secret. It stores nothing
per user.

## Trust policy

The repository being written to owns its policy, in `.github/perseid-trust.yml`:

```yaml
# Who may get a token for this repository, and with which permissions.
- repository: acme/api                      # OIDC `repository` claim
  workflow: .github/workflows/perseid-push.yml
  ref: refs/heads/main                       # or refs/tags/v*
  permissions: { contents: write }
- repository: acme/api-sdks
  workflow: .github/workflows/sdks.yml
  ref: refs/heads/main
  permissions: { contents: write, pull-requests: write }
```

| Policy field | Matched against |
|---|---|
| `workflow` | The `job_workflow_ref` claim |
| `ref` | The `ref` claim, as a glob |
| `repository` | The `repository` and `repository_id` claims, so a recreated repository with the same name does not inherit trust |

- Events from forks (`pull_request` from a fork) are rejected.
- `perseid init` and `perseid connect` write the policy entries with the workflows. The user
  commits them, like every other file perseid writes.

## Commands and workflows

- `connect --auth perseid` writes `perseid-push.yml` with `permissions: id-token: write` and
  `auth: perseid` as its credential input. `push-spec` asks the broker for a token for `--to`.
  There is no key and no secret.
- `sdks.yml` and `sdk-release.yml` do the same for the SDK repositories. App tokens trigger
  workflows, so CI runs on SDK and release pull requests with no setting to change.
- `perseid app` links to `github.com/apps/perseid/installations/new` to install the perseid App
  on the repositories. `SDK_GITHUB_TOKEN` is not needed.

## App permissions

The App asks for repository Contents, Pull requests and Workflows (read and write), and Metadata
(read). Workflows is there because the first pull request in an SDK repository adds its
`sdk-release.yml`. Every token is narrowed further, to the policy's permissions and one
repository.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| A stolen App key writes to every installing repository | Worker secret only, rotated; minimal App permissions; tokens always narrowed; audit log of every mint (repository, workflow, run id) |
| A broker bug grants a token outside the policy | Small codebase, deny by default, tests from captured OIDC tokens, no wildcard repositories |
| A broker outage stops SDK generation | Personal tokens and self-hosted Apps keep working; `--auth` picks one |
| Users who refuse third-party Apps | As for an outage: every other `--auth` stays available |
| Replayed OIDC tokens | Short expiry, plus the `jti` claim cached for its lifetime |

## Cost

Each workflow run makes one request: one JWKS check (cached), one policy read and one token
mint. That fits in Cloudflare Workers' free tier for a long time.

## Prior art

Chainguard's open-source octo-sts follows the same pattern: GitHub OIDC, trust policies in the
target repository, short-lived App tokens. The broker could run octo-sts itself, or port its
policy model to a Worker.

## Open questions

- Whether the policy lives in its own file or in `perseid.toml` (`[trust]`). A separate file keeps
  the policy reviewable on its own.
- Whether GitHub Enterprise Server is supported, with a different issuer and API base per
  instance.
