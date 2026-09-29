#!/usr/bin/env bash
# Generated for ubuntu-24.04 by the same release as the config and workflows.
set -euo pipefail
PERSEID_VERSION=@@VERSION@@
PERSEID_TOOLS="${RUNNER_TEMP:?GitHub Actions runner required}/perseid-tools"
mkdir -p "$PERSEID_TOOLS/bin"
archive=perseid-x86_64-unknown-linux-musl.tar.gz
release="https://github.com/meteroid-oss/perseid/releases/download/v$PERSEID_VERSION"
curl --fail --silent --show-error --location "$release/$archive" -o "$PERSEID_TOOLS/$archive"
curl --fail --silent --show-error --location "$release/SHA256SUMS" -o "$PERSEID_TOOLS/SHA256SUMS"
(
  cd "$PERSEID_TOOLS"
  # Require exactly the requested archive's digest, then verify before extraction.
  awk -v file="$archive" '$2 == file { print; found++ } END { if (found != 1) exit 1 }' SHA256SUMS > selected.sha256
  sha256sum --check selected.sha256
  tar -xzf "$archive" -C bin perseid
)
printf '%s\n' "$PERSEID_TOOLS/bin" >> "$GITHUB_PATH"
@@FORMATTERS@@
