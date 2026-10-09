// Generates and builds an SDK from a fixture spec, for the tests that need more than petstore.
// Not a test itself: the test glob is `*.test.mjs`. Run from the directory of the petstore SDK,
// whose `node_modules` the generated SDKs share.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

export const run = promisify(execFile);

export const fixtures = process.env.FIXTURES;
assert.ok(fixtures, "set FIXTURES to the perseid tests/fixtures directory");

export const tsc = resolve("node_modules/typescript/bin/tsc");

/**
 * Generates the SDK of `fixtures/<spec>`, or of the spec `text`, named `name` with the given
 * `int64` mode, with its round trips when `roundTrips` is set, automatic idempotency keys when
 * `idempotencyKeys` is set and `[typescript]` keys of `settings`. `build` compiles it and
 * imports it as `sdk`.
 */
export async function generate(spec, { name, text, int64 = "number", roundTrips = false, idempotencyKeys = false, build = true, settings = "" }) {
  const dir = mkdtempSync(join(tmpdir(), `perseid-${name.toLowerCase()}-`));
  const cleanup = () => rmSync(dir, { recursive: true, force: true });
  try {
    if (text === undefined) {
      copyFileSync(join(fixtures, spec), join(dir, "openapi.yaml"));
    } else {
      writeFileSync(join(dir, "openapi.yaml"), text);
    }
    writeFileSync(
      join(dir, "perseid.toml"),
      `spec = "openapi.yaml"\nsdks = ["typescript"]\nname = "${name}"\nbase_url = "https://${name.toLowerCase()}.test/v1"\nround_trips = ${roundTrips}\nidempotency_keys = ${idempotencyKeys}\n[typescript]\nint64 = "${int64}"\n${settings}`
    );
    await run("perseid", ["generate"], { cwd: dir });
    const sdkDir = join(dir, "typescript");
    symlinkSync(resolve("node_modules"), join(sdkDir, "node_modules"));
    if (!build) {
      return { dir, sdkDir, sdk: undefined, cleanup };
    }
    await run(process.execPath, [tsc, "-p", "."], { cwd: sdkDir });
    const sdk = await import(pathToFileURL(join(sdkDir, "dist/esm/index.js")).href);
    return { dir, sdkDir, sdk, cleanup };
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
