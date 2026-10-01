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
perseid init "$lang"
sed -i '/^name = /a webhooks = true' perseid.toml
perseid generate "$lang"
cp -r "$here/$lang/." "$lang/"
cd "$lang"

case "$lang" in
  rust)
    cargo test --features webhooks
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    cd "$work/torture" && perseid init rust && perseid generate rust
    mkdir -p rust/tests && cp "$here/rust/torture/"*.rs rust/tests/ && cd rust && cargo test ;;
  typescript)
    npm install --no-audit --no-fund && npm run build
    FIXTURES="$here/../fixtures" node --test test/*.test.mjs ;;
  python)
    uv venv -q && uv pip install -q -e .
    FIXTURES="$here/../fixtures" .venv/bin/python -m unittest discover -s tests -v ;;
  go)
    go vet ./... && go test ./...
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    cd "$work/torture" && perseid init go && perseid generate go
    cp "$here/go/_torture/"*_test.go go/ && cd go && go vet ./... && go test ./... ;;
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
    rm -rf _torture && gradle_test
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    cd "$work/torture" && perseid init java && perseid generate java
    cp -r "$here/java/_torture/." java/ && cd java && gradle_test ;;
  csharp)
    rm -rf _torture && dotnet test Tests
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    cd "$work/torture" && perseid init csharp && perseid generate csharp
    cp -r "$here/csharp/_torture/." csharp/ && cd csharp && dotnet test Tests ;;
esac
