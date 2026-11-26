#!/usr/bin/env bash
# Renders the pack against tests/acme with $PERSEID, builds, lints and tests the CLI, runs e2e.py,
# then `dist generate --check` and `dist plan` with $DIST or an installed `dist`. WORK keeps the
# rendered files. The build is --locked to tests/acme's Cargo.lock files: UPDATE_LOCKS=1 rewrites them.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
pack=$(dirname "$here")
perseid=${PERSEID:-perseid}
work=${WORK:-$(mktemp -d)}
rm -rf "$work/rust" "$work/cli"

"$perseid" generate -c "$here/acme/perseid.toml" rust --out "$work"
"$perseid" targets -c "$here/acme/perseid.toml" cli --out "$work"
cli=$work/cli
python3 - "$pack/runtime-crate/Cargo.toml" "$cli/generated/Cargo.toml" <<'PY'
import sys, tomllib
runtime, generated = (tomllib.load(open(path, "rb")) for path in sys.argv[1:])
for key in ["features", "dev-dependencies", "target"]:
    assert runtime[key] == generated[key], f"[{key}] of the generated Cargo.toml is not the runtime's"
extra = {"sdk", "chrono", "uuid", "rust_decimal"}
deps = {k: v for k, v in generated["dependencies"].items() if k not in extra}
assert runtime["dependencies"] == deps, "the generated Cargo.toml has other [dependencies] than the runtime"
PY
printf '\n[patch.crates-io]\nacme-rs = { path = "../rust" }\n' >>"$cli/Cargo.toml"

locked=(--locked)
if [[ -n ${UPDATE_LOCKS:-} ]]; then
  locked=()
else
  cp "$here/acme/Cargo.lock" "$cli/Cargo.lock"
  cp "$here/acme/generated.Cargo.lock" "$cli/generated/Cargo.lock"
fi

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$pack/target}
patch=(--config "patch.crates-io.acme-rs.path=\"$work/rust\"")
cargo fmt --check --manifest-path "$cli/Cargo.toml"
cargo clippy --manifest-path "$cli/Cargo.toml" "${locked[@]}" --all-targets -- -D warnings
cargo clippy --manifest-path "$cli/generated/Cargo.toml" "${patch[@]}" "${locked[@]}" --all-targets -- -D warnings
cargo clippy --manifest-path "$cli/generated/Cargo.toml" "${patch[@]}" "${locked[@]}" --no-default-features -- -D warnings
cargo clippy --manifest-path "$cli/generated/Cargo.toml" "${patch[@]}" "${locked[@]}" --all-features -- -D warnings
cargo build --manifest-path "$cli/Cargo.toml" "${locked[@]}"
cargo test --manifest-path "$cli/Cargo.toml" "${locked[@]}"
cargo test --manifest-path "$cli/generated/Cargo.toml" "${patch[@]}" "${locked[@]}"
if [[ -n ${UPDATE_LOCKS:-} ]]; then
  cp "$cli/Cargo.lock" "$here/acme/Cargo.lock"
  cp "$cli/generated/Cargo.lock" "$here/acme/generated.Cargo.lock"
fi
ACME=$CARGO_TARGET_DIR/debug/acme python3 "$here/e2e.py"
# The release workflow matches dist-workspace.toml, with the version of dist it pins ($DIST).
dist=${DIST:-$(command -v dist || true)}
if [[ -n $dist ]]; then
  (cd "$cli" && "$dist" generate --check && "$dist" plan >/dev/null)
fi
