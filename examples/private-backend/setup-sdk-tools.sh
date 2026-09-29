#!/usr/bin/env bash
# Copy to the orchestration repository as .perseid/setup-ci.sh.
# Rust + TypeScript example. Rust/rustup, Node/npm, Git, and gh must be installed.
set -euo pipefail

# Pin the same generator for backend updates, SDK checks, and releases.
PERSEID_REV=6cabbf9a186e67bead7021f7f05c1aaa3258e467
SDK_TOOLS_ROOT="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/perseid-sdk-tools"
mkdir -p "$SDK_TOOLS_ROOT/bin"

cargo install --locked --git https://github.com/meteroid-oss/perseid \
  --rev "$PERSEID_REV" --bin perseid --root "$SDK_TOOLS_ROOT"

# Match Docker's rustfmt while leaving cargo/rustc on the SDK's stable toolchain.
rustup toolchain install nightly-2025-02-27 --profile minimal --component rustfmt
cat > "$SDK_TOOLS_ROOT/bin/rustfmt" <<'FORMATTER'
#!/usr/bin/env bash
exec rustup run nightly-2025-02-27 rustfmt "$@"
FORMATTER
chmod +x "$SDK_TOOLS_ROOT/bin/rustfmt"
npm install --prefix "$SDK_TOOLS_ROOT/formatters" --ignore-scripts \
  --no-audit --no-fund @biomejs/biome@2.1.4

if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "$SDK_TOOLS_ROOT/bin" "$SDK_TOOLS_ROOT/formatters/node_modules/.bin" >> "$GITHUB_PATH"
else
  printf 'Add these directories to PATH:\n%s\n%s\n' \
    "$SDK_TOOLS_ROOT/bin" "$SDK_TOOLS_ROOT/formatters/node_modules/.bin"
fi
