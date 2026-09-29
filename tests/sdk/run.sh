#!/usr/bin/env bash
# Generates the petstore SDK with webhooks enabled and runs the language's extension tests.
# usage: tests/sdk/run.sh <rust|typescript|python|go|java>
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
  rust) cargo test --features webhooks ;;
  typescript) npm install --no-audit --no-fund && npx tsc && node --test test/*.test.mjs ;;
  python)
    uv venv -q && uv pip install -q -e .
    uv run python -m unittest discover -s tests -v ;;
  go) go vet ./... && go test ./... ;;
  java)
    cat >> build.gradle <<'GRADLE'
dependencies {
    testImplementation 'org.junit.jupiter:junit-jupiter:5.11.4'
    testRuntimeOnly 'org.junit.platform:junit-platform-launcher:1.11.4'
    testCompileOnly 'org.projectlombok:lombok:1.18.36'
}
test { useJUnitPlatform() }
GRADLE
    gradle test --no-daemon ;;
esac
