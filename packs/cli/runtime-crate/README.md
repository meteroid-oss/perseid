# perseid-cli-runtime

The API-independent runtime of the CLIs perseid generates, as a crate: its modules are the files
of [`../runtime`](../runtime), which perseid vendors into each CLI, so they are tested as they
ship. [The pack's README](../README.md) describes the contract for generated code and the CLI's
behavior; this file covers the crate.

| Module | |
|---|---|
| `app` | `App`, the global options and built-in commands (`auth`, `completion`, `man`, `schema`, `agents`, `mcp`) added to the root, `Globals`, `Host`, `Ctx` (`ctx.token()`), `run`/`run_with` |
| `args` | path, query and header parameters: positional or flags checked by clap, then decoded into the SDK's types; uploads and `--output` |
| `auth` | the `Auth` hook, `ApiKey`, `Methods`, `auth login`/`logout`/`status`/`token`/`profiles`, opening the browser |
| `oauth` | `DeviceCode` and `Browser` logins, refresh and revocation (feature `oauth`) |
| `config` | `config.toml` profiles and their `Credential`, the precedence flag > env > profile > default, 0600 writes, its lock |
| `keyring` | the secrets of profiles in the OS keyring (feature `keyring`), and their way back |
| `retry` | `--retries`, `--no-retry`, `--timeout` and their variables, for the SDK's builder; `--debug`'s log of attempts |
| `mcp` | `mcp`: the Model Context Protocol server, a tool per operation of `schema`, each call a command run in process |
| `body` | `--data` and field flags into a JSON body, checked against the schema, decoded into the SDK's type with errors naming the flag |
| `schema` | the generated JSON Schema of the commands: `schema`'s index and descriptions, body validation with suggestions |
| `dry_run` | `--dry-run`'s `Recorder`, the request the SDK's `HttpClient` would send, as JSON |
| `output` | `--format` and `--jq` on one response, `Items` for paginated lists and event streams, tables |
| `jq` | jq filters through jaq, on `serde_json::Value`, with `@csv` and `@tsv` |
| `error` | `CliError`, its constructors for the SDK's errors, JSON and human rendering, exit codes |

The runtime is vendored, not depended on: perseid copies `../runtime/*.rs` into the generated
crate's `src/runtime/`, `mod.rs` included, and this crate builds the same files with
`[lib] path = "../runtime/mod.rs"`. The modules refer to each other through `super::`, so they
work at any depth, and no macro of ours is involved. Each file carries the `@generated` marker
perseid requires of the files it writes. Generated crates carry the dependencies below, which
`../tests/run.sh` checks against the pack's `Cargo.toml.jinja`.

## Dependencies

Sixteen direct dependencies, 88 crates in all, without the `oauth` feature. With it (the
default) 24 and 149, but every added crate is one the Rust SDK already depends on, at the same
version: a generated CLI pays nothing for it, nor for `hyper`. A release build of the example
is about 10 MB, 6.5 MB without `oauth`: the difference is the HTTP and TLS stack the SDK brings
anyway.

| Crate | Why |
|---|---|
| `clap` 4 (`string`) | The command tree, with the builder API: derive would cost compile time on large APIs (see the roadmap). `string` lets help text carry the env prefix. No `env` feature: the precedence is resolved in `config`, against an injected environment. |
| `clap_complete` | `completion <shell>`. |
| `clap_mangen` | `man`, one page per command. |
| `serde`, `serde_json` (`preserve_order`) | Responses go through `serde_json::Value`, which the SDK's models serialize to. `preserve_order` keeps the models' field order: stable, and the order of the spec rather than alphabetical. |
| `serde_path_to_error` | The property path of a decoding error (`address.city`), to name the flag that set it. serde_json alone gives a line and column. |
| `serde-saphyr` (`serialize` only) | YAML output. `serde_yaml` is deprecated and its forks `serde_yaml_ng` and `serde_norway` have had no release since 2024 and still carry a C-to-Rust port of libyaml. serde-saphyr is maintained (1.3, September 2026), pure safe Rust, and serialize-only it pulls four small crates. It quotes YAML 1.1 booleans like `n` and `yes`. |
| `comfy-table` | Tables: Unicode widths and wrapping to the terminal's width. Its default `tty` feature (crossterm without its event loop) gives the terminal width and bold headers. |
| `jaq-core` 3, `jaq-std` 3, `jaq-json` 2 (`serde`) | `--jq` in process, no `jq` binary needed. The current releases (jaq 3; jaq-json is versioned on its own). `jaq-std`'s defaults keep `test`, `@base64d` and the date functions, which filters on API objects use. |
| `toml` 1 | `config.toml`, read and written with serde; only the `parse`, `display` and `serde` features. |
| `dirs` | The platform's config directory, when `XDG_CONFIG_HOME` is unset. |
| `rpassword` | `auth login` prompts for the key without echo. |
| `hyper` 1 (no feature) | The request and body types of `--dry-run`, whose recorder the SDK's `HttpClient` calls: the SDK's own. `oauth` adds `client` and `http1`. |
| `tokio` (`rt`) | `run` drives the generated `async fn dispatch` on a current-thread runtime. The SDK brings the rest of Tokio it needs; `oauth` adds `net`, `time` and `io-util`. |

Behind `oauth`, the stack of the SDK's own connector rather than reqwest, which the SDK does not
use and which would add its own TLS provider (aws-lc-rs by default in 0.13):

| Crate | Why |
|---|---|
| `hyper` (`client`, `http1`), `hyper-util` (`client-legacy`), `http-body-util` | Two form POSTs per login or refresh, over HTTP/1.1. |
| `hyper-rustls` 0.27, `rustls` 0.23 (`ring`, native roots) | HTTPS to the authorization server, configured like the SDK's connector. |
| `url` | The authorization URL, the redirect's query, form encoding. |
| `sha2` 0.10, `base64` 0.22, `getrandom` 0.2 | PKCE: S256 challenges, base64url, random verifiers and `state`. |

Behind `keyring`, off by default and on in the scaffold's binary: the platform's store only,
each in Rust, so no system library is needed to build or run. On Linux the zbus stack adds 68
crates (75 without `oauth`); the Keychain adds 2, the Credential Manager 5.

| Crate | Why |
|---|---|
| `keyring-core` 1 | The entries' API, and the mock store the tests use. `keyring` 4 is now a sample CLI over it, and keyring 3's stores live in crates of their own. |
| `apple-native-keyring-store` (`keychain`), macOS | The login Keychain, through `security-framework`. `protected` (the data protection keychain) needs a signed app. |
| `windows-native-keyring-store`, Windows | The Credential Manager, through `windows-sys`. |
| `zbus-secret-service-keyring-store` (`rt-async-io-crypto-rust`), Linux and BSDs | The Secret Service (GNOME Keyring, KWallet) over D-Bus, with zbus and RustCrypto: no `libdbus` nor OpenSSL, unlike `dbus-secret-service-keyring-store`. Its blocking calls run zbus on async-io's own thread, which a Tokio runtime cannot conflict with; the `rt-tokio` variant would block inside it. |

`mcp` needs no crate: JSON-RPC over lines is `serde_json`, base64 a few lines.

Neither `anyhow` nor `thiserror`: `CliError` is one struct with a kind, written by hand in a
few lines, and generated code builds it through its constructors. Terminal detection is
`std::io::IsTerminal`, the config file's lock std's `File::lock` (Rust 1.89, the MSRV).

Dev only: `tempfile`, and `toml` to read the config file in tests.

## Tests

`cargo test` runs the unit tests of each module and `tests/acme.rs`, which drives
`examples/acme.rs` (the hand-written generated CLI over a fake SDK) in process: streaming and
`--max-items`, page flags and the first page on a terminal, formats and `<PREFIX>_FORMAT`,
`--jq` and its failures keeping the response, bodies from flags, `--data` and stdin, checked
against the schema, `schema` and `agents`, API, network, usage and config errors with their
exit codes, golden `--help`, `auth login`, `auth profiles`, `auth logout`, completion, man
pages and closed pipes. The OAuth tests run a fake authorization server on 127.0.0.1 and play
the browser: device code (pending, then approved; denied), browser login with PKCE and `state`
(and a forged `state`, and an idle connection opened first), a custom login minting an API key,
refresh on expiry saved to the profile, once for concurrent runs, unexpired tokens kept when
they cannot be refreshed, URLs relative to `--base-url`, revocation on logout, `auth status`
and `auth token`, `--api-key-stdin`, the device login's JSON off a terminal. `--retries`,
`--timeout` and their variables reaching the SDK's builder; `mcp` sessions, with and without
`initialize`: tools and their schemas, calls, invalid bodies as `isError`, unknown tools and
versions, `--read-only` and `--tools`. With `--features keyring`, `keyring-core`'s mock store
holds the secrets of `auth login`, refreshes and `auth logout`. Nothing touches the network or
the OS keyring.
