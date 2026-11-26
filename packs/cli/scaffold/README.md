# @@NPM_PACKAGE@@

The @@CLIENT_NAME@@ API from the command line.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-cli-installer.sh | sh
```

On Windows: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-cli-installer.ps1 | iex"`.
With Homebrew, `brew install <owner>/tap/@@NPM_PACKAGE@@`; with npm, `npm install -g @@NPM_PACKAGE@@`
(once they are set up: see [Releasing](#releasing)).

```sh
export @@ENV_PREFIX@@_API_KEY=...        # or `@@NPM_PACKAGE@@ auth login`, which stores it in a profile
@@NPM_PACKAGE@@ --help
@@NPM_PACKAGE@@ <resource> list --max-items 10 --format table
@@NPM_PACKAGE@@ <resource> create --data @body.json --<field> value
@@NPM_PACKAGE@@ completion zsh > ~/.zfunc/_@@NPM_PACKAGE@@
```

Every command takes `--format json|jsonl|yaml|table`, `--jq`, `--profile`, `--api-key`,
`--base-url`, `--retries`, `--timeout` and `--dry-run`. Off a terminal, output is JSON and
errors are one line of JSON on stderr; the exit code says what failed: 2 usage, 3 API 4xx, 4 API
5xx, 5 network, 6 configuration. `@@NPM_PACKAGE@@ agents` is the guide for scripts and agents;
`@@NPM_PACKAGE@@ auth status` shows the credential in use, `@@NPM_PACKAGE@@ auth token` prints its token.

## For AI agents: MCP

`@@NPM_PACKAGE@@ mcp` is a Model Context Protocol server on stdio, a tool per operation, with the
credentials of the command line.

In Claude Desktop, open the bundle of your platform: it asks for the API key, and ships its own
binary. [macOS, Apple silicon](https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-darwin-arm64.mcpb),
[macOS, Intel](https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-darwin-x64.mcpb),
[Windows](https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-win32-x64.mcpb),
[Linux](https://github.com/@@PACK_REPO@@/releases/latest/download/@@NPM_PACKAGE@@-linux-x64.mcpb).

With Claude Code:

```sh
claude mcp add @@NPM_PACKAGE@@ -e @@ENV_PREFIX@@_API_KEY=... -- @@NPM_PACKAGE@@ mcp
```

In another MCP client's configuration:

```json
{"mcpServers": {"@@NPM_PACKAGE@@": {"command": "@@NPM_PACKAGE@@", "args": ["mcp"], "env": {"@@ENV_PREFIX@@_API_KEY": "..."}}}}
```

Results are structured, and a `jq` argument on each tool keeps only the fields needed; lists
return `{items, has_more, next}`. Schemas suit the client (`--client` to choose).
`--read-only` serves only the operations that read, `--tools 'customers_*'` those matching,
`--exclude-tools '*_delete'` leaves some out, and `--list` prints the tools.

`@@NPM_PACKAGE@@ mcp --port 8080` serves the same tools over HTTP at `http://127.0.0.1:8080/mcp`,
each request with its own API key; `--key-from-env` lets requests without one use the
credentials of the command line, for a server only you reach. With Claude Code:

```sh
claude mcp add --transport http @@NPM_PACKAGE@@ http://127.0.0.1:8080/mcp --header "Authorization: Bearer ..."
```

## Layout

| Path | |
|---|---|
| `generated/` | The commands of every operation, the runtime they share, and its `Cargo.toml`: perseid regenerates it from the spec, through pull requests. Its files say `@generated`; don't edit them |
| `src/main.rs` | The binary: it mounts the generated commands and yours, and picks how to log in. Yours |
| `dist-workspace.toml`, `.github/workflows/release.yml`, `publish-*.yml` | Releases, by [dist](https://github.com/axodotdev/cargo-dist) |
| `release-please-config.json`, `.release-please-manifest.json` | Versions and changelog, by [release-please](https://github.com/googleapis/release-please) |

perseid writes the other files once, then leaves them to you.

## Your own commands

A command is a clap `Command` and an async function given the runtime's `Ctx`.
`generated::client(&mut ctx)` is the SDK's client, with the credential and base URL of the
command line, environment or profile. In `src/listen.rs`:

```rust
use generated::{
    clap::{Arg, ArgMatches, Command},
    runtime::{app::Ctx, CliError},
};

pub fn command() -> Command {
    Command::new("listen")
        .about("Forward events to a local URL")
        .arg(Arg::new("forward-to").long("forward-to").value_name("URL").required(true))
}

pub async fn run(ctx: &mut Ctx, matches: &ArgMatches) -> Result<(), CliError> {
    let client = generated::client(ctx).await?;
    let url = matches.get_one::<String>("forward-to").expect("required");
    ctx.debug(format_args!("forwarding to {url} with {client:?}"));
    // Call the SDK with `client`, then print with `ctx.print(&value)` or stream with `ctx.items()`.
    Ok(())
}
```

Then in `src/main.rs`, `mod listen;`, `.subcommand(listen::command())` in `command()`, and in
`dispatch()`, before the generated commands:

```rust
if let Some(("listen", matches)) = matches.subcommand() {
    return listen::run(&mut ctx, matches).await;
}
```

`cargo test` fails if a name is taken twice.

## Logging in

`generated::AUTH` follows the spec's security schemes: an API key, prompted for or read from
stdin by `auth login`, and `auth login --web` and `--device` for an OAuth2 scheme with the
authorization code and device flows, once perseid.toml gives the OAuth client to use:

```toml
[context]
cli_client_id = "@@NPM_PACKAGE@@-cli"
```

`APP` in `src/main.rs` can log in any other way: `&generated::runtime::auth::ApiKey` for API
keys only, or a type of yours implementing `generated::runtime::auth::Auth`.

`auth login` keeps the API key and tokens in the OS keyring (Keychain, Credential Manager,
Secret Service), else in the config file, as with `--no-keyring` or `@@ENV_PREFIX@@_NO_KEYRING=1`.
Removing `features = ["keyring"]` from `Cargo.toml` keeps them in the file, and the binary
smaller.

## Releasing

Merging the pull request release-please keeps open drafts the GitHub Release and its tag, then
the release workflow builds the binaries of the tag and its installers, uploads them and
publishes the release, then publishes the Homebrew formula and the npm package. Without what
either needs, its job says so in the run's summary and succeeds, and the release goes on
without that channel.

The release workflows use perseid's credentials, the first they find:

| Credential | Set up by | Stored as |
|---|---|---|
| A GitHub App of your own | `perseid app` | `SDK_APP_ID` variable, `SDK_APP_PRIVATE_KEY` secret, on this repository too |
| A fine-grained token | You | `SDK_GITHUB_TOKEN` secret |
| The [perseid App](https://github.com/apps/perseid-sdks) | `perseid sync`, or installing it | Nothing: each run trades its OIDC token for one |

With an App, install it on this repository and on the Homebrew tap, choosing "Only select
repositories" (the perseid App reaches the tap under the same owner only). release-please then
opens its pull requests and drafts releases as the App, so CI runs on them. With none of them it
falls back to the workflow token, which needs "Allow GitHub Actions to create and approve pull
requests" in Settings > Actions > General; `ci.yml` is then dispatched on the release pull
request, so require its `ci` commit status rather than its check.

Homebrew, once:

1. Create the tap, a public repository named `homebrew-tap` under the same owner, with a first
   commit (a README will do). Under another name or owner, set the repository variable
   `HOMEBREW_TAP` to its `owner/name`.
2. Install the App on it. With a token instead, give it Contents write on the tap, or save
   another one as the `HOMEBREW_TAP_TOKEN` secret.

`brew install <owner>/tap/@@NPM_PACKAGE@@` then installs each release.

npm, once:

1. Check that the package name `@@NPM_PACKAGE@@` is free on npmjs.com. If not, set
   `npm-scope = "@your-org"` in `dist-workspace.toml` and run `dist generate`.
2. The first publish needs a token: create a granular access token with read and write on
   packages, and save it as the `NPM_TOKEN` secret of this repository.
3. Once the package exists, in its settings on npmjs.com, add a trusted publisher: GitHub
   Actions, this repository, workflow `release.yml`, no environment. Then delete `NPM_TOKEN`
   and its token: each release publishes with the run's OIDC token, with provenance.

`npm install -g @@NPM_PACKAGE@@` then installs each release; the package downloads the binary of
the platform from the GitHub Release.

Changing `installers` or any setting of `dist-workspace.toml` needs `dist generate` (dist
0.32.0, as `cargo-dist-version` says), which rewrites `release.yml`; `dist plan` shows what a
release builds. The publish jobs are `publish-homebrew.yml` and `publish-npm.yml`, dist's own
steps behind a check of their credential.

## Building against an unreleased SDK

Uncomment the `[patch.crates-io]` of `Cargo.toml`, pointing at a checkout of the SDK.
