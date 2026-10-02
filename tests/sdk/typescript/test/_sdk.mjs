// Generates and builds an SDK from a fixture spec, for the tests that need more than petstore.
// Not a test itself: the test glob is `*.test.mjs`. Run from the directory of the petstore SDK,
// whose `node_modules` the generated SDKs share.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

const run = promisify(execFile);

export const fixtures = process.env.FIXTURES;
assert.ok(fixtures, "set FIXTURES to the perseid tests/fixtures directory");

const tsc = resolve("node_modules/typescript/bin/tsc");

/**
 * Generates the SDK of `fixtures/<spec>` named `name` with the given `int64` mode, and when
 * `samples` is set writes its `samples.json`. `build` compiles it and imports it as `sdk`; a spec
 * without models, where `samples.json` is empty, is left unbuilt.
 */
export async function generate(spec, { name, int64 = "number", samples = false, build = true }) {
  const dir = mkdtempSync(join(tmpdir(), `perseid-${name.toLowerCase()}-`));
  const cleanup = () => rmSync(dir, { recursive: true, force: true });
  try {
    copyFileSync(join(fixtures, spec), join(dir, "openapi.yaml"));
    writeFileSync(
      join(dir, "perseid.toml"),
      `spec = "openapi.yaml"\nsdks = ["typescript"]\nname = "${name}"\nbase_url = "https://${name.toLowerCase()}.test/v1"\n[typescript]\nint64 = "${int64}"\n`
    );
    await run("perseid", ["generate"], { cwd: dir });
    let models = {};
    if (samples) {
      await run("perseid", ["samples", "--out", "samples.json"], { cwd: dir });
      models = readExact(readFileSync(join(dir, "samples.json"), "utf8"));
    }
    if (!build || (samples && Object.keys(models).length === 0)) {
      return { dir, models, sdk: undefined, cleanup };
    }
    const sdkDir = join(dir, "typescript");
    symlinkSync(resolve("node_modules"), join(sdkDir, "node_modules"));
    await run(process.execPath, [tsc, "-p", "."], { cwd: sdkDir });
    const sdk = await import(pathToFileURL(join(sdkDir, "dist/esm/index.js")).href);
    return { dir, models, sdk, cleanup };
  } catch (error) {
    cleanup();
    throw error;
  }
}

/** Runs `tasks` (functions returning promises), at most `limit` at a time, in order. */
export async function pool(tasks, limit) {
  const results = new Array(tasks.length);
  let next = 0;
  const worker = async () => {
    while (next < tasks.length) {
      const index = next++;
      results[index] = await tasks[index]();
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, tasks.length) }, worker));
  return results;
}

const TOKEN = /"(?:[^"\\]|\\.)*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;
const MARK = "\u0000int:";

/** `JSON.parse` that keeps every digit of an integer, as a `bigint`; floats stay numbers. */
export function readExact(text) {
  const marked = text.replace(TOKEN, (token) =>
    /^-?\d+$/.test(token) ? `"\\u0000int:${token}"` : token
  );
  const revive = (value) => {
    if (typeof value === "string") {
      return value.startsWith(MARK) ? BigInt(value.slice(MARK.length)) : value;
    }
    if (Array.isArray(value)) {
      return value.map(revive);
    }
    if (value !== null && typeof value === "object") {
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, revive(item)]));
    }
    return value;
  };
  return revive(JSON.parse(marked));
}

/** The JSON text of a value read by `readExact`, its `bigint`s as integer literals. */
export function writeExact(value) {
  const json = JSON.stringify(value, (_key, item) => (typeof item === "bigint" ? `${MARK}${item}` : item));
  return json.replace(/"\\u0000int:(-?\d+)"/g, "$1");
}
