#!/usr/bin/env bash
# Renders the pack against tests/acme with $PERSEID, builds its TypeScript SDK, builds the server
# against it and against perseid-mcp as packed from packages/perseid-mcp ($PERSEID_MCP to take
# another), then runs e2e.mjs against the CLI pack's mock API. WORK keeps the rendered files.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
perseid=${PERSEID:-perseid}
work=${WORK:-$(mktemp -d)}
rm -rf "$work/typescript" "$work/mcp" "$work"/perseid-mcp-*.tgz

"$perseid" generate -c "$here/acme/perseid.toml" typescript --out "$work"
"$perseid" targets -c "$here/acme/perseid.toml" mcp --out "$work"

(cd "$work/typescript" && npm install --no-audit --no-fund && npm run build)
runtime=${PERSEID_MCP:-$here/../../../packages/perseid-mcp}
(cd "$runtime" && npm run build && npm pack --pack-destination "$work" --silent >/dev/null)
cd "$work/mcp"
node -e '
const fs = require("fs");
const manifest = JSON.parse(fs.readFileSync("package.json", "utf8"));
const tarball = fs.readdirSync("..").find((f) => /^perseid-mcp-.*\.tgz$/.test(f));
manifest.dependencies.acme = "file:../typescript";
if (tarball) manifest.dependencies["perseid-mcp"] = "file:../" + tarball;
fs.writeFileSync("package.json", JSON.stringify(manifest, null, 2));'
npm install --no-audit --no-fund
npx tsc --noEmit false
SERVER="$work/mcp/dist/main.js" node --test "$here/e2e.mjs"
