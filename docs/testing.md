# Testing

perseid is tested in layers, from the generator to SDKs running against a server.

| Layer | Where | Runs |
|---|---|---|
| [Cargo tests](#cargo-tests) | `src/`, `tests/cli.rs` | `cargo test` |
| [Edge fixtures](#edge-fixtures) | `tests/fixtures/` | CI, every language |
| [Mock server scenarios](#mock-server-scenarios) | `tests/features/` | `tests/features/run.sh <language> <sdk-dir>` |
| [Schema-derived round trips](#schema-derived-round-trips) | `perseid samples` | CI, every language |
| [Runtime tests](#runtime-tests) | `tests/sdk/<language>` | `tests/sdk/run.sh <language>` |
| [Real-world specs](#real-world-specs) | `real-world.yml` | Weekly, and on pull requests touching `src/`, `templates/` or `runtime/` |

## Cargo tests

- Unit tests sit next to the code: spec normalization, naming, unions, identifiers.
- `tests/cli.rs` runs the binary on inline specs, and checks the generated files and the
  warnings and errors reported.

## Edge fixtures

`tests/fixtures/edge-*.yaml` (names, types, operations, security, unions, legacy 3.0 constructs,
OpenAPI 3.2) pile up constructs that are hard to generate:

- keywords and punctuation as names, clashing enums and parameters;
- recursive aliases, boolean schemas, nullable items;
- cookie parameters, webhook-only specs.

CI generates an SDK from each in every language, then compiles and type-checks it, alongside
`petstore`, `features`, `torture` and `realworld`.

## Mock server scenarios

`tests/features/mock_server.py` serves `features.yaml` and checks every request against the spec:
method, raw path and query, percent-encoding, headers, content type, body and credentials.

- A request that deviates fails with the reason in the response.
- Each language's smoke test (`smoke.ts`, `smoke.py`, `smoke_test.go`, `smoke.rs`, `Smoke.java`,
  `Smoke.cs`) fails on it.
- The scenarios, shared by the six smoke tests, are listed in
  [`tests/features/SCENARIOS.md`](../tests/features/SCENARIOS.md).

## Schema-derived round trips

The hidden `perseid samples --out samples.json` writes JSON instances of every model, derived
from its schema: full, minimal, nulls, one per union variant and enum value. Each carries the
type name each language generates.

Each SDK decodes and re-encodes them and compares the JSON, so models agree with the spec without
handwritten cases.

## Runtime tests

`tests/sdk/run.sh <language>` generates the petstore and torture SDKs and runs the tests of
`tests/sdk/<language>` over the runtime: retries, middleware, pagination, streaming, errors,
webhooks, the torture fixture's unions and serialization.

## Real-world specs

The Stripe, GitHub and OpenAI specs are generated and compiled in every language. Their specs
change without notice, so `real-world.yml` stays out of the required checks.

## Adding a case

- A construct that breaks generation gets a minimal inline spec in `tests/cli.rs`, or a construct
  in an edge fixture.
- Behavior on the wire gets a mock server scenario.
