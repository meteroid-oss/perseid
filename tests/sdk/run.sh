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
if [ "$lang" = go ]; then printf '\n[go]\nmodule = "github.com/petstore/petstore-go"\n' >> perseid.toml; fi
if [ "$lang" = csharp ]; then printf '\n[csharp.context]\ndependency_injection = true\n' >> perseid.toml; fi
perseid generate "$lang"
cp -r "$here/$lang/." "$lang/"
cd "$lang"

case "$lang" in
  rust)
    export CARGO_TARGET_DIR="$work/target"
    cargo test --features webhooks
    # Generates the SDK of one fixture, writes the samples of its models (`perseid samples`) and
    # the registry of their types, then runs `cargo test`: every test of tests/sdk/rust/torture
    # for the torture fixture, only the sample round trips for the others.
    rust_samples() (
      dir="$work/samples/$(basename "$1" .yaml)"
      mkdir -p "$dir" && cd "$dir"
      perseid init --sdks rust --spec "$1" > /dev/null && perseid generate rust > /dev/null
      perseid samples --out samples.json > /dev/null
      grep -q '"type_name"' samples.json || { echo "$1 has no models"; exit 0; }
      mkdir -p rust/tests
      python3 "$here/rust/torture/samples_registry.py" samples.json rust
      if [ "$(basename "$1")" = torture.yaml ]; then
        cp "$here/rust/torture/"*.rs rust/tests/ && (cd rust && cargo test)
      else
        cp "$here/rust/torture/samples.rs" rust/tests/ && (cd rust && cargo test --test samples)
      fi
    )
    for spec in "$here/../fixtures/torture.yaml" "$here"/../fixtures/edge-*.yaml; do
      rust_samples "$spec"
    done
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
    # The tests generate more SDKs themselves (features, torture, realworld and, with their
    # `perseid samples`, the sample round trips of torture and every edge fixture).
    npm install --no-audit --no-fund && npm run build
    FIXTURES="$here/../fixtures" node --test test/*.test.mjs ;;
  python)
    # The tests generate more SDKs themselves, with the `perseid` binary of the PATH: the torture
    # fixture, the features fixture (client behavior) and, with their `perseid samples`, the
    # sample round trips of torture and every edge fixture.
    uv venv -q && uv pip install -q -e .
    FIXTURES="$here/../fixtures" .venv/bin/python -m unittest discover -s tests -v ;;
  go)
    go vet ./... && go test ./...
    # Generates the SDK of one fixture, writes the samples of its models (`perseid samples`) and
    # the registry of their types, then runs `go test`: every test of tests/sdk/go/_torture for
    # the torture fixture, only the sample round trips for the others.
    go_samples() (
      dir="$work/samples/$(basename "$1" .yaml)"
      mkdir -p "$dir" && cd "$dir"
      perseid init --sdks go --spec "$1" > /dev/null && perseid generate go > /dev/null
      perseid samples --out samples.json > /dev/null
      grep -q '"type_name"' samples.json || { echo "$1 has no models"; exit 0; }
      python3 "$here/go/_samples/samples_registry.py" samples.json go
      if [ "$(basename "$1")" = torture.yaml ]; then
        cp "$here/go/_torture/"*_test.go go/ && (cd go && go vet ./... && go test ./...)
      else
        (cd go && go test -run TestSamplesRoundTrip ./...)
      fi
    )
    for spec in "$here/../fixtures/torture.yaml" "$here"/../fixtures/edge-*.yaml; do
      go_samples "$spec"
    done
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
    rm -rf _torture _samples _oauth _unions && gradle_test
    mkdir "$work/torture" && cp "$here/../fixtures/torture.yaml" "$work/torture/openapi.yaml"
    # Without servers, the client has no default base URL.
    sed -i '/^servers:/,/^paths:/{/^paths:/!d}' "$work/torture/openapi.yaml"
    # The tests of tests/sdk/java/_torture and the sample round trips of _samples: `perseid samples`
    # writes samples.json, which SamplesRoundTripTest decodes and encodes with the generated
    # classes of every model.
    (cd "$work/torture" && perseid init --sdks java && perseid generate java \
      && perseid samples --out samples.json > /dev/null \
      && cp -r "$here/java/_torture/." java/ && cp -r "$here/java/_samples/." java/ \
      && cp samples.json java/ && cd java && gradle_test)
    # Generates the SDK of one edge fixture, writes the samples of its models and runs only the
    # sample round trips on it.
    java_samples() (
      dir="$work/samples/$(basename "$1" .yaml)"
      mkdir -p "$dir" && cd "$dir"
      perseid init --sdks java --spec "$1" > /dev/null && perseid generate java > /dev/null
      perseid samples --out samples.json > /dev/null
      grep -q '"type_name"' samples.json || { echo "$1 has no models"; exit 0; }
      cp -r "$here/java/_samples/." java/ && cp samples.json java/ && cd java && gradle_test
    )
    for spec in "$here"/../fixtures/edge-*.yaml; do
      java_samples "$spec"
    done
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
    rm -rf _torture _samples _oauth _unions && dotnet test Tests
    # Generates the SDK of one fixture, writes the samples of its models (`perseid samples`) and runs
    # `dotnet test` on it: every test of tests/sdk/csharp/_torture for the torture fixture, only the
    # sample round trips for the others.
    csharp_samples() (
      name=$(basename "$1" .yaml)
      dir="$work/samples/$name"
      mkdir -p "$dir" && cd "$dir"
      perseid init --sdks csharp --spec "$1" > /dev/null
      # Without servers, the client has no default base URL.
      if [ "$name" = torture ]; then sed -i '/^base_url/d' perseid.toml; fi
      perseid generate csharp > /dev/null
      perseid samples --out samples.json > /dev/null
      grep -q '"type_name"' samples.json || { echo "$1 has no models"; exit 0; }
      sdk=$(basename "$(ls csharp/*/*.csproj | grep -v '\.Tests/' | head -n 1)" .csproj)
      if [ "$name" = torture ]; then
        cp -r "$here/csharp/_torture/." csharp/
      else
        mkdir csharp/Tests
        cat > csharp/Tests/Tests.csproj <<CSPROJ
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <IsPackable>false</IsPackable>
    <IsTestProject>true</IsTestProject>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.8.0" />
    <PackageReference Include="xunit" Version="2.5.3" />
    <PackageReference Include="xunit.runner.visualstudio" Version="2.5.3" />
  </ItemGroup>
  <ItemGroup>
    <Using Include="Xunit" />
    <ProjectReference Include="../$sdk/$sdk.csproj" />
  </ItemGroup>
</Project>
CSPROJ
      fi
      cp "$here/csharp/_samples/SamplesTests.cs" csharp/Tests/
      cd csharp && PERSEID_SAMPLES="$dir/samples.json" PERSEID_SDK="$sdk" dotnet test Tests
    )
    for spec in "$here/../fixtures/torture.yaml" "$here"/../fixtures/edge-*.yaml; do
      csharp_samples "$spec"
    done
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
