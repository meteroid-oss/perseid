# Testing

perseid is tested in layers, from the generator to SDKs running against a server.

1. **Cargo tests.** Unit tests next to the code (spec normalization, naming, unions, identifiers)
   and `tests/cli.rs`, which runs the binary on inline specs and checks the generated files and
   the warnings and errors reported. `cargo test` runs them all.
2. **Edge fixtures.** `tests/fixtures/edge-*.yaml` (names, types, operations, security, legacy
   3.0 constructs, OpenAPI 3.2) pile up the constructs that broke generation: keywords and
   punctuation as names, clashing enums and parameters, recursive aliases, boolean schemas, cookie
   parameters, nullable items, webhook-only specs. CI generates an SDK from each, in every
   language, and compiles and type-checks it, alongside `petstore`, `features`, `torture` and
   `realworld`.
3. **Strict mock server scenarios.** `tests/features/mock_server.py` serves `features.yaml` and
   checks every request against the spec: method, raw path and query, percent-encoding, headers,
   content type, body and credentials. A sloppy request fails with the reason in the response, so
   each language's smoke test (`tests/features/smoke.*`) fails on it. The scenarios, shared by the
   six smoke tests, are listed in [`tests/features/SCENARIOS.md`](../tests/features/SCENARIOS.md).
4. **Schema-derived round trips.** The hidden `perseid samples --out samples.json` writes, for
   every model, JSON instances derived from the schema (full, minimal, nulls, one per union
   variant and enum value) with the type name each language generates. Each SDK decodes and
   re-encodes them and compares the JSON, so models agree with the spec without handwritten
   cases.
5. **Per-language runtime tests.** `tests/sdk/run.sh <language>` generates the petstore and
   torture SDKs and runs the tests of `tests/sdk/<language>` over the runtime: retries, middleware,
   pagination, streaming, errors, webhooks, the torture fixture's unions and serialization.
6. **Real-world specs.** The Stripe, GitHub and OpenAI specs are generated and compiled in every
   language on a weekly schedule and on pull requests that touch `src/`, `templates/` or
   `runtime/` (`real-world.yml`). Their specs change without notice, so they stay out of the
   required checks.

A bug found in the wild gets a minimal inline spec in `tests/cli.rs` or a construct in an edge
fixture, and a scenario when it is about what goes over the wire.
