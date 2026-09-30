#!/bin/sh
# Usage: npm/stage.sh VERSION DIST OUT
# Writes publishable packages to OUT, taking binaries from the perseid-<target>.tar.gz release archives in DIST.
set -eu
version=$1 dist=$2 out=$3
root=$(cd "$(dirname "$0")/.." && pwd)

for pair in linux-x64:x86_64-unknown-linux-musl linux-arm64:aarch64-unknown-linux-musl \
  darwin-x64:x86_64-apple-darwin darwin-arm64:aarch64-apple-darwin; do
  name=perseid-${pair%%:*}
  mkdir -p "$out/$name/bin"
  tar -xzf "$dist/perseid-${pair#*:}.tar.gz" -C "$out/$name"
  mv "$out/$name/perseid" "$out/$name/bin/perseid"
  jq --arg v "$version" '.version = $v' "$root/npm/$name/package.json" >"$out/$name/package.json"
done

mkdir -p "$out/perseid"
cp -R "$root/npm/perseid/bin" "$root/README.md" "$root"/LICENSE* "$root/NOTICE" "$out/perseid/"
jq --arg v "$version" '.version = $v | .optionalDependencies[] = $v' \
  "$root/npm/perseid/package.json" >"$out/perseid/package.json"
