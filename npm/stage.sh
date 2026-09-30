#!/bin/sh
# Usage: npm/stage.sh VERSION DIST OUT
# Writes the publishable package to OUT, with the checksums of the release archives in DIST.
set -eu
version=$1 dist=$2 out=$3
root=$(cd "$(dirname "$0")/.." && pwd)

mkdir -p "$out"
cp -R "$root/npm/perseid/bin" "$root/README.md" "$root"/LICENSE* "$root/NOTICE" "$out/"
(cd "$dist" && sha256sum perseid-*.tar.gz) |
  jq -Rn '[inputs | split("  ") | {(.[1]): .[0]}] | add' >"$out/checksums.json"
jq --arg v "$version" '.version = $v' "$root/npm/perseid/package.json" >"$out/package.json"
