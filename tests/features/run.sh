#!/bin/sh
# Runs the smoke test of one language against the mock server: run.sh <language> <sdk-dir>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lang=$1
cd "$2"
url=$(mktemp)
python3 -u "$here/mock_server.py" > "$url" &
mock=$!
trap 'kill $mock; rm -f "$url"' EXIT
while [ ! -s "$url" ]; do sleep 0.1; done
FEATURES_URL=$(cat "$url")
export FEATURES_URL
case $lang in
  typescript)
    # The int64 scenarios need exact 64-bit integers: regenerate the SDK with `int64 = "bigint"`.
    if ! grep -q '^int64 = "bigint"' ../perseid.toml; then
      if grep -q '^\[typescript\]' ../perseid.toml; then
        sed -i '/^\[typescript\]/a int64 = "bigint"' ../perseid.toml
      else
        printf '\n[typescript]\nint64 = "bigint"\n' >> ../perseid.toml
      fi
      (cd .. && perseid generate typescript)
    fi
    mkdir -p smoke && cp "$here/smoke.ts" smoke/
    npx tsc --outDir smoke-build --rootDir . --strict --target es2020 --module commonjs --lib es2020,dom --types node --skipLibCheck smoke/smoke.ts
    echo '{"type":"commonjs"}' > smoke-build/package.json
    node smoke-build/smoke/smoke.js ;;
  python) cp "$here/smoke.py" . && uv run --no-project python smoke.py ;;
  go) cp "$here/smoke_test.go" . && go test -run 'TestSmoke|TestScenario' -count=1 . ;;
  java)
    mkdir -p smoke && cp "$here/Smoke.java" smoke/
    grep -q "register('smoke'" build.gradle || cat >> build.gradle <<'GRADLE'
sourceSets { smoke { java { srcDir 'smoke' }; compileClasspath += main.output + configurations.runtimeClasspath; runtimeClasspath += output + compileClasspath } }
tasks.register('smoke', JavaExec) { classpath = sourceSets.smoke.runtimeClasspath; mainClass = 'Smoke' }
GRADLE
    gradle --no-daemon -q smoke ;;
  rust)
    mkdir -p tests && cp "$here/smoke.rs" tests/
    cargo add -q --dev tokio --features macros,rt-multi-thread
    cargo add -q --dev futures@0.3
    cargo test -q --test smoke ;;
  csharp)
    mkdir -p smoke && cp "$here/Smoke.cs" smoke/
    project=$(ls ./*/*.csproj | grep -v '^./smoke/' | head -n 1)
    cat > smoke/Smoke.csproj <<CSPROJ
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
  </PropertyGroup>
  <ItemGroup>
    <ProjectReference Include="../$project" />
  </ItemGroup>
</Project>
CSPROJ
    dotnet run --project smoke ;;
esac
