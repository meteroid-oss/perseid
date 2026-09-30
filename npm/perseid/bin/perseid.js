#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");

const pkg = `perseid-${process.platform}-${process.arch}`;
let binary;
try {
  binary = require.resolve(`${pkg}/bin/perseid`);
} catch {
  console.error(
    `perseid: no prebuilt binary for ${process.platform}-${process.arch} (${pkg} is not installed).\n` +
      "Linux and macOS on x64 and arm64 are supported; Windows is not supported yet.\n" +
      "If optional dependencies were omitted (--no-optional, --omit=optional), reinstall with them, or run:\n" +
      "  curl -fsSL https://sh.meteroid.com/perseid | sh",
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`perseid: cannot run ${binary}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
