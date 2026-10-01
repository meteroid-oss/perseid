# Design: a hosted perseid App with no stored secrets

Status: proposal. Nothing here is deployed.

## Why

Today every cross-repository write needs a credential that someone creates and stores:

- the spec repository pushes with a deploy key, a personal token or its own App key;
- `sdks.yml` opens pull requests in other repositories with the `SDK_APP_*` App key;
- in a single repository, the default `GITHUB_TOKEN` opens pull requests on which CI doesn't run,
  and needs the "Allow GitHub Actions to create and approve pull requests" setting.

A perseid GitHub App would cover all three cases. Users can't be handed its private key, though, so
tokens have to come from a service. That service never stores user secrets: it trusts GitHub's
OIDC tokens, and a policy committed in the repository being written to.

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

The broker holds a single secret, the App's private key, stored as a Worker secret. It stores
nothing per user.

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

The workflow is matched on the `job_workflow_ref` claim, the ref on `ref` (glob), and the
repository on `repository_id` as well as `repository`, so a deleted and recreated repository with
the same name doesn't inherit trust. Events from forks (`pull_request` from a fork) are rejected.
`perseid init` and `perseid connect` write the policy entries along with the workflows. Like every
other file perseid writes, the user commits it.

## Commands and workflows

- `connect --auth perseid` writes `perseid-push.yml` with `permissions: id-token: write`, and with
  `auth: perseid` in place of a credential input. `push-spec` asks the broker for a token for
  `--to`. GitHub needs nothing else: there is no key and no secret.
- `sdks.yml` and `sdk-release.yml` do the same for the SDK repositories. App tokens trigger
  workflows, so CI runs on SDK and release pull requests with no setting to change.
- `setup-github` shrinks to "install the perseid App on these repositories". That is a link to
  `github.com/apps/perseid/installations/new`.

## App permissions

The App asks for repository Contents (read and write), Pull requests (read and write) and
Metadata (read). It needs no Workflows permission: perseid never pushes workflow files, the user
commits them. Every token is narrowed further, to the policy's permissions and one repository.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| A stolen App key writes to every installing repository | Worker secret only, rotated; App permissions minimal; tokens always narrowed; audit log of every mint (repository, workflow, run id) |
| A broker bug grants a token outside the policy | Small codebase, deny by default, tests from captured OIDC tokens, no wildcard repositories |
| A broker outage stops SDK generation | Deploy keys, personal tokens and self-hosted Apps keep working; `--auth` picks one |
| Users who refuse third-party Apps | Same as the outage: every other `--auth` stays available |
| Replayed OIDC tokens | Short expiry, plus the `jti` claim cached for its lifetime |

## Cost

Each workflow run makes one request, and each request is one JWKS check (cached), one policy read
and one token mint. That fits in Cloudflare Workers' free tier for a long time.

## Prior art

Chainguard's open-source octo-sts follows the same pattern: GitHub OIDC, trust policies in the
target repository, short-lived App tokens. The broker could run octo-sts itself, or port its policy
model to a Worker.

## Not decided

- Whether the policy lives in its own file or in `perseid.toml` (`[trust]`). A separate file keeps
  the policy reviewable on its own.
- Whether GitHub Enterprise Server is supported (a different issuer and API base for each instance).
