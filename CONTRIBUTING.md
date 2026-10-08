# Contributing

Thanks for helping. Bug reports with a spec that reproduces them are the most useful thing you
can send. For a feature or a new language, open an issue first, so we can agree on the shape
before you write it six times.

Questions go to [Discussions](https://github.com/meteroid-oss/perseid/discussions).

## Setup

perseid is one Rust binary. To build it and run its own tests, you need:

- Rust stable, with `rustfmt` and `clippy`. The minimum is 1.88.
- Optionally, oasdiff, for the release sizing tests. They are skipped without it.

```sh
cargo build
cargo run -- tools install rust --dir ~/.local/bin   # oasdiff, on your PATH
```

To build and test the generated SDKs, install perseid on your PATH, then the toolchain of each
language you touch:

```sh
cargo install --locked --path .        # perseid in ~/.cargo/bin
perseid tools install typescript python java csharp   # pinned formatters, next to perseid
```

| Language | Toolchain, as CI uses it |
|---|---|
| Rust | Rust stable |
| TypeScript | Node 20+ and npm |
| Python | [uv](https://docs.astral.sh/uv/). The mock server also needs `python3` |
| Go | Go 1.24 |
| Java | JDK 17 and Gradle |
| C# | .NET 8 and .NET 10 SDKs |

`rustfmt`, `gofmt` and `csharpier` come with their toolchains. `perseid tools list` prints the
formatters and their pinned versions. Docker is only needed to build the image.

## Tests

The layers are described in [docs/testing.md](docs/testing.md). Before you push:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

When you change `templates/`, `runtime/` or how code is generated, also build an SDK in each
language you touched. CI does this for every fixture of `tests/fixtures/`:

```sh
spec=$PWD/tests/fixtures/features.yaml   # from the repository root
mkdir -p /tmp/sdk && cd /tmp/sdk
perseid init --sdks rust,typescript,python,go,java,csharp --spec "$spec"
sed -i '/^name = /a round_trips = true' perseid.toml
perseid generate && perseid generate --check
```

Then build and test each SDK the way the `sdks` job of
[`.github/workflows/github-actions.yml`](.github/workflows/github-actions.yml) does: `cargo test`,
`npx tsc --noEmit && npm test`, `mypy --strict` and `unittest`, `go vet` and `go test`,
`gradle build`, `dotnet build -warnaserror && dotnet test`.

Two scripts cover runtime behavior. Both need perseid and the language's toolchain on the PATH.

```sh
tests/sdk/run.sh typescript                          # retries, middleware, webhooks, unions...
tests/features/run.sh typescript /tmp/sdk/typescript # an SDK of features.yaml against the mock server
```

### Files to regenerate

There are no snapshot files: `tests/cli.rs` asserts on the generated files inline. Two committed
files are generated:

| File | After changing | Regenerate with |
|---|---|---|
| `perseid.schema.json` | `src/config.rs`. `cargo test` fails while it is stale | `cargo run -- schema > perseid.schema.json` |
| `.github/cover.svg` | `.github/cover/cover.py` | `python3 .github/cover/cover.py` |

## Repository layout

| Path | Contents |
|---|---|
| `src/` | The binary. `main.rs` holds the commands |
| `src/spec/`, `src/spec.rs` | Loading the spec: bundling external `$ref`s, upgrading 3.0, normalizing |
| `src/api/` | The model templates receive: resources, operations, types, unions, pagination, security. `perseid inspect` prints it |
| `src/generate.rs`, `src/generator.rs`, `src/template.rs`, `src/template/` | Rendering: where each template writes, filters such as `ident` |
| `src/postprocessing.rs`, `src/format.rs`, `src/tools.rs` | Formatters and their pinned versions |
| `src/scaffold.rs`, `src/init.rs` | `perseid init` and the files an SDK starts from |
| `src/pr.rs`, `src/sizing.rs`, `src/changelog.rs` | `generate --pr`: pull requests, release size, changelogs |
| `src/github/` | `sync`, `app`, `connect`, `status` and the spec push |
| `templates/<lang>/` | Jinja templates, rendered on every generation |
| `runtime/<lang>/` | Runtime files copied into each SDK. `@@TOKENS@@` are replaced; `features/<name>/` is copied only when that feature is on |
| `scaffold/<lang>/` | Files written on the first generation only: README, manifest, error types |
| `scaffold/release/` | `sdk-ci.yml` and `sdk-release.yml` |
| `action.yml`, `test/`, `publish/`, `release/`, `push/` | The GitHub Actions perseid's workflows use |
| `tests/` | `cli.rs`, `github.rs`, `push.rs`; fixtures, mock server scenarios and runtime tests |
| `docs/` | User docs |

`build.rs` embeds `templates/`, `runtime/` and `scaffold/` in the binary, so a rebuild picks up
their changes.

Every file perseid writes must carry `@generated` in its first lines, or generation fails. Files
without it belong to the user.

## Adding a feature

A feature lands in all six languages in the same pull request.

1. If it needs a setting, add it to `src/config.rs` and regenerate `perseid.schema.json`. A
   setting each SDK can override goes in the `language!` table.
2. Expose what the templates need from `src/api/`. Check it with `perseid inspect`.
3. Write it in `templates/<lang>/` and `runtime/<lang>/` for each language. Opt-in runtime code
   goes under `runtime/<lang>/features/<name>/`.
4. Test it:
   - generation and warnings in `tests/cli.rs`, with a minimal inline spec;
   - a hard construct in an edge fixture, `tests/fixtures/edge-*.yaml`;
   - behavior on the wire as a mock server scenario: `features.yaml`, `mock_server.py`,
     [`SCENARIOS.md`](tests/features/SCENARIOS.md) and the six smoke tests;
   - runtime behavior in `tests/sdk/<lang>/`.
5. Document it in `docs/`: [features](docs/features.md) for behavior, the language sections of
   [languages](docs/languages.md) for syntax, [configuration](docs/configuration.md) for
   settings. Add a line to the README table if users will look for it there.

## Adding a language

Open a [new language issue](https://github.com/meteroid-oss/perseid/issues/new?template=new_language.yml)
first. A language is a large, long-lived change: it has to reach parity with the others, and
someone has to keep it there. C# is the latest one added; following its traces is the fastest
way in (`grep -rn csharp src`).

- `src/config.rs`: `LANGUAGES`, the `Language` enum, a `language!` table, the default `package`.
- `src/generate.rs`: where templates, runtime, tests and round trips go; the file extension.
- `src/api/types.rs`: the type names the templates print (`to_csharp` and its siblings).
- `src/template.rs`, `src/template/ident.rs`, `src/reserved.rs`, `src/client_name.rs`: doc
  comments, keywords, reserved names.
- `src/postprocessing.rs`, `src/format.rs`, `src/tools.rs`: the formatter, pinned.
- `src/scaffold.rs` and `scaffold/<lang>/`: the package skeleton and its release-please package.
- `src/init.rs`, `src/github/mod.rs`: the prompt, and how to set up its registry.
- `templates/<lang>/`, `runtime/<lang>/`: the SDK itself.
- `test/action.yml`, `publish/action.yml`, `action.yml`: detecting, testing and publishing it.
- `tests/sdk/<lang>/` and `tests/sdk/run.sh`, a smoke test in `tests/features/`, and the matrices
  of `.github/workflows/github-actions.yml` and `real-world.yml`.
- `README.md`, `docs/languages.md`, `.github/cover/cover.py`.

## Commits and pull requests

- Commits and pull request titles follow [Conventional Commits](https://www.conventionalcommits.org):
  `feat:`, `fix:`, `docs:`, `chore:`, with a scope when it helps (`fix(pr):`) and `!` for a
  breaking change.
- Pull requests are squash-merged, so the title becomes the commit.
  [release-please](https://github.com/googleapis/release-please) reads it to bump the version and
  write `CHANGELOG.md`. Before 1.0, `feat!:` bumps the minor version and `feat:` the patch.
- Keep a pull request to one change. Say how you tested it.

## Code style

- Match the code around you. `cargo fmt` and clippy must pass with no warnings.
- Generated code must pass each language's formatter and strictest checks: `mypy --strict`,
  `tsc`, `go vet`, `dotnet build -warnaserror`.
- Name things so that comments are rarely needed. Keep comments to three lines or fewer, and do
  not repeat what the code says.
- Docs use short, concrete sentences, like the existing pages.
