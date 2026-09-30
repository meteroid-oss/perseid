#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { version } = require("../package.json");
const checksums = require("../checksums.json");

const targets = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
};

function fail(message) {
  console.error(`perseid: ${message}`);
  process.exit(1);
}

function cacheDir() {
  if (process.env.PERSEID_CACHE) return process.env.PERSEID_CACHE;
  if (process.platform === "darwin") return path.join(os.homedir(), "Library", "Caches", "perseid");
  return path.join(process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache"), "perseid");
}

async function download(target, binary) {
  const archive = `perseid-${target}.tar.gz`;
  const url = `https://github.com/meteroid-oss/perseid/releases/download/v${version}/${archive}`;
  console.error(`perseid: downloading ${version} for ${target} (first run only)`);
  let response;
  try {
    response = await fetch(url);
  } catch (error) {
    fail(
      `cannot download ${url}: ${error.cause?.message ?? error.message}\n` +
        "Behind a proxy, set HTTPS_PROXY and NODE_USE_ENV_PROXY=1 (Node 24+), or install with:\n" +
        "  curl -fsSL https://sh.meteroid.com/perseid | sh",
    );
  }
  if (!response.ok) fail(`cannot download ${url}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const digest = crypto.createHash("sha256").update(bytes).digest("hex");
  if (digest !== checksums[archive]) fail(`checksum mismatch for ${archive}, refusing to run it`);

  const staging = fs.mkdtempSync(path.join(path.dirname(binary), ".download-"));
  try {
    fs.writeFileSync(path.join(staging, archive), bytes);
    const tar = spawnSync("tar", ["-xzf", archive, "perseid"], { cwd: staging, stdio: "inherit" });
    if (tar.status !== 0) fail(`cannot extract ${archive}`);
    fs.renameSync(path.join(staging, "perseid"), binary);
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }
}

async function main() {
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) {
    fail(
      `no prebuilt binary for ${process.platform}-${process.arch}: Linux and macOS on x64 and arm64 are supported.\n` +
        "Build from source: cargo install --git https://github.com/meteroid-oss/perseid",
    );
  }
  const binary = path.join(cacheDir(), version, "perseid");
  if (!fs.existsSync(binary)) {
    fs.mkdirSync(path.dirname(binary), { recursive: true });
    await download(target, binary);
  }

  const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) fail(`cannot run ${binary}: ${result.error.message}`);
  if (result.signal) process.kill(process.pid, result.signal);
  process.exit(result.status ?? 1);
}

main();
