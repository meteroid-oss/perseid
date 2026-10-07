#!/usr/bin/env bash
# Generates the petstore SDK with webhooks enabled and runs the language's extension tests.
# usage: tests/sdk/run.sh <rust|typescript|python|go|java|csharp>
set -euo pipefail
lang=${1:?usage: run.sh <language>}
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cp "$here/../fixtures/petstore.yaml" "$work/openapi.yaml"
cd "$work"
perseid init --sdks "$lang"
sed -i '/^name = /a webhooks = true' perseid.toml
if [ "$lang" = rust ]; then sed -i '/^base_url = /d' perseid.toml; fi
if [ "$lang" = go ]; then sed -i '/^\[go\]$/a module = "github.com/petstore/petstore-go"' perseid.toml; fi
if [ "$lang" = csharp ]; then printf '\n[csharp.context]\ndependency_injection = true\n' >> perseid.toml; fi
perseid generate "$lang"
cp -r "$here/$lang/." "$lang/"
cd "$lang"

case "$lang" in
  rust)
    export CARGO_TARGET_DIR="$work/target"
    cargo test --features webhooks
    # The tests of tests/sdk/rust/torture run on the SDK of tests/fixtures/torture.yaml.
    (mkdir "$work/torture" && cd "$work/torture" \
      && perseid init --sdks rust --spec "$here/../fixtures/torture.yaml" > /dev/null \
      && sed -i '/^name = /a idempotency_keys = true' perseid.toml \
      && perseid generate rust > /dev/null \
      && mkdir -p rust/tests && cp "$here/rust/torture/"*.rs rust/tests/ \
      && cd rust && cargo test)
    # The OAuth2 client credentials tests run on the SDK of tests/fixtures/oauth.yaml.
    (mkdir "$work/oauth" && cd "$work/oauth" \
      && perseid init --sdks rust --spec "$here/../fixtures/oauth.yaml" > /dev/null \
      && perseid generate rust > /dev/null \
      && mkdir -p rust/tests && cp "$here/rust/oauth/"*.rs rust/tests/ \
      && cd rust && cargo test --test oauth)
    # The decoding of unions sharing a JSON type, best-match unions and open enums run on the SDK
    # of tests/fixtures/edge-unions.yaml.
    (mkdir "$work/unions" && cd "$work/unions" \
      && perseid init --sdks rust --spec "$here/../fixtures/edge-unions.yaml" > /dev/null \
      && perseid generate rust > /dev/null \
      && mkdir -p rust/tests && cp "$here/rust/unions/"*.rs rust/tests/ \
      && cd rust && cargo test --test models --test client) ;;
  typescript)
    # The tests generate more SDKs themselves (features, torture, realworld and, with
    # `int64 = "bigint"` and `"string"`, the round trips of torture and every edge fixture).
    npm install --no-audit --no-fund && npm run build
    FIXTURES="$here/../fixtures" node --test test/*.test.mjs ;;
  python)
    # The tests generate more SDKs themselves, with the `perseid` binary of the PATH: the torture
    # fixture and the features fixture (client behavior).
    uv venv -q && uv pip install -q -e .
    FIXTURES="$here/../fixtures" .venv/bin/python -m unittest discover -s tests -v ;;
  go)
    go vet ./... && go test ./...
    # The tests of tests/sdk/go/_torture run on the SDK of tests/fixtures/torture.yaml.
    (mkdir "$work/torture" && cd "$work/torture" \
      && perseid init --sdks go --spec "$here/../fixtures/torture.yaml" > /dev/null \
      && sed -i '/^name = /a idempotency_keys = true' perseid.toml \
      && perseid generate go > /dev/null \
      && cp "$here/go/_torture/"*_test.go go/ \
      && cd go && go vet ./... && go test ./...)
    # The OAuth2 client credentials tests run on the SDK of tests/fixtures/oauth.yaml.
    (mkdir "$work/oauth" && cd "$work/oauth" \
      && perseid init --sdks go --spec "$here/../fixtures/oauth.yaml" > /dev/null \
      && perseid generate go > /dev/null \
      && cp "$here/go/_oauth/"*_test.go go/ \
      && cd go && go vet ./... && go test ./...)
    # The decoding of unions sharing a JSON type, best-match unions, union bodies, hoisted
    # variants and open enums run on the SDK of tests/fixtures/edge-unions.yaml.
    (mkdir "$work/unions" && cd "$work/unions" \
      && perseid init --sdks go --spec "$here/../fixtures/edge-unions.yaml" > /dev/null \
      && perseid generate go > /dev/null \
      && cp "$here/go/_unions/"*_test.go go/ \
      && cd go && go vet ./... && go test ./...) ;;
  java)
    gradle_test() {
      cat >> build.gradle <<'GRADLE'
dependencies {
    testImplementation 'org.junit.jupiter:junit-jupiter:5.11.4'
    testRuntimeOnly 'org.junit.platform:junit-platform-launcher:1.11.4'
}
test { useJUnitPlatform(); testLogging { events "passed", "skipped", "failed"; exceptionFormat "full" } }
GRADLE
      gradle test --no-daemon
    }
    rm -rf _torture _oauth _unions && gradle_test
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    # Without servers, the client has no default base URL.
    sed -i '/^servers:/,/^paths:/{/^paths:/!d}' "$work/torture/openapi.yaml"
    # The tests of tests/sdk/java/_torture.
    (cd "$work/torture" && perseid init --sdks java \
      && sed -i '/^name = /a idempotency_keys = true' perseid.toml && perseid generate java \
      && cp -r "$here/java/_torture/." java/ && cd java && gradle_test)
    # The OAuth2 client credentials tests run on the SDK of tests/fixtures/oauth.yaml.
    (mkdir "$work/oauth" && cd "$work/oauth" \
      && perseid init --sdks java --spec "$here/../fixtures/oauth.yaml" > /dev/null \
      && perseid generate java > /dev/null \
      && cp -r "$here/java/_oauth/." java/ && cd java && gradle_test)
    # The decoding of unions sharing a JSON type, best-match unions, union bodies and open enums
    # run on the SDK of tests/fixtures/edge-unions.yaml.
    (mkdir "$work/unions" && cd "$work/unions" \
      && perseid init --sdks java --spec "$here/../fixtures/edge-unions.yaml" > /dev/null \
      && perseid generate java > /dev/null \
      && cp -r "$here/java/_unions/." java/ && cd java && gradle_test) ;;
  csharp)
    rm -rf _torture _oauth _unions && dotnet test Tests
    # The tests of tests/sdk/csharp/_torture run on the SDK of tests/fixtures/torture.yaml, with no
    # default base URL.
    (mkdir "$work/torture" && cd "$work/torture" \
      && perseid init --sdks csharp --spec "$here/../fixtures/torture.yaml" > /dev/null \
      && sed -i '/^name = /a idempotency_keys = true' perseid.toml \
      && sed -i '/^base_url/d' perseid.toml \
      && perseid generate csharp > /dev/null \
      && cp -r "$here/csharp/_torture/." csharp/ && cd csharp && dotnet test Tests)
    # The OAuth2 client credentials tests run on the SDK of tests/fixtures/oauth.yaml.
    (mkdir "$work/oauth" && cd "$work/oauth" \
      && perseid init --sdks csharp --spec "$here/../fixtures/oauth.yaml" > /dev/null \
      && perseid generate csharp > /dev/null \
      && cp -r "$here/csharp/_oauth/." csharp/ && cd csharp && dotnet test Tests)
    # The decoding of unions sharing a JSON type, best-match unions, union bodies and open enums
    # run on the SDK of tests/fixtures/edge-unions.yaml.
    (mkdir "$work/unions" && cd "$work/unions" \
      && perseid init --sdks csharp --spec "$here/../fixtures/edge-unions.yaml" > /dev/null \
      && perseid generate csharp > /dev/null \
      && cp -r "$here/csharp/_unions/." csharp/ && cd csharp && dotnet test Tests) ;;
esac
