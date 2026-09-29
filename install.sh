#!/bin/sh
# curl -fsSL https://raw.githubusercontent.com/meteroid-oss/perseid/main/install.sh | sh
# Env: PERSEID_VERSION (default: latest), PERSEID_INSTALL (default: ~/.local/bin)
set -eu

repo="meteroid-oss/perseid"
version="${PERSEID_VERSION:-latest}"
dir="${PERSEID_INSTALL:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target="x86_64-unknown-linux-musl" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  *) echo "perseid: unsupported platform $(uname -s)-$(uname -m), build from source: cargo install --git https://github.com/$repo" >&2; exit 1 ;;
esac

case "$version" in
  latest) url="https://github.com/$repo/releases/latest/download" ;;
  v*) url="https://github.com/$repo/releases/download/$version" ;;
  *) url="https://github.com/$repo/releases/download/v$version" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
archive="perseid-$target.tar.gz"
curl -fsSL "$url/$archive" -o "$tmp/$archive"
curl -fsSL "$url/SHA256SUMS" -o "$tmp/SHA256SUMS"
(
  cd "$tmp"
  grep " $archive\$" SHA256SUMS > expected
  if command -v sha256sum >/dev/null; then sha256sum -c expected >/dev/null; else shasum -a 256 -c expected >/dev/null; fi
)
tar -xzf "$tmp/$archive" -C "$tmp" perseid
mkdir -p "$dir"
mv "$tmp/perseid" "$dir/perseid"
chmod +x "$dir/perseid"

echo "installed $("$dir/perseid" --version) to $dir/perseid"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add it to your PATH: export PATH=\"$dir:\$PATH\"" ;;
esac
