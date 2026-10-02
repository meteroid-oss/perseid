#!/bin/bash
# usage: run.sh spec.yaml [langs comma]   -> out/<specname>/
F=/tmp/claude-0/-home-user-perseid/dfe4da85-da76-5610-9dab-d14c668ed481/scratchpad/fuzz
P=/home/user/perseid/target/release/perseid
spec=$(realpath "$1"); langs=${2:-rust,typescript,python,go,java}
name=$(basename "$spec" .yaml)
out=$F/out/$name; rm -rf "$out"; mkdir -p "$out"; cd "$out"
export CARGO_TARGET_DIR=$F/cargo-target
$P init --sdks "$langs" --spec "$spec" > init.log 2>&1 || { echo "$name: INIT FAIL"; tail -5 init.log; exit 1; }
[ -f "${spec%.yaml}.toml" ] && cat "${spec%.yaml}.toml" >> perseid.toml
res=""
for l in ${langs//,/ }; do
  if ! $P generate $l > gen-$l.log 2>&1; then res="$res $l:GENFAIL"; continue; fi
  grep -i -E 'warn|skip' gen-$l.log | grep -v -E 'google-java-format|Go module path|biome|ruff' | sed "s/^/$name gen-$l: /"
  case $l in
    rust) (cd rust && cargo check --quiet) > rust.log 2>&1;;
    typescript) (cd typescript && npm install --no-audit --no-fund >/dev/null 2>&1 && npx tsc --noEmit) > typescript.log 2>&1;;
    python) (cd python && pkg=$(sed -n 's/^name = "\(.*\)"/\1/p' pyproject.toml | tr - _) && uv venv -q && uv pip install -q -e . mypy && uv run python -c "import $pkg" && uv run mypy --strict "$pkg") > python.log 2>&1;;
    go) (cd go && go vet ./...) > go.log 2>&1;;
    java) (cd java && gradle build --no-daemon -q 2>&1 | grep -v JAVA_TOOL_OPTIONS; exit ${PIPESTATUS[0]}) > java.log 2>&1;;
  esac
  rc=$?; [ $rc = 0 ] && res="$res $l:ok" || res="$res $l:FAIL"
done
echo "$name:$res"
