// The generated round trips of torture and every edge fixture, with `int64 = "bigint"` and
// `"string"`: CI runs them in the default `number` mode on every fixture, and here every digit of
// an int64 must come back.
import assert from "node:assert/strict";
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { after, it } from "node:test";
import { fixtures, generate, pool, run, tsc } from "./_sdk.mjs";

const SPECS = readdirSync(fixtures)
  .filter((file) => file === "torture.yaml" || /^edge-.*\.yaml$/.test(file))
  .sort();

const MODES = ["bigint", "string"];
const variants = SPECS.flatMap((spec) => MODES.map((mode) => ({ spec, mode })));
const built = await pool(
  variants.map(({ spec, mode }) => () =>
    generate(spec, { name: `Rt${spec.replace(/\W|yaml$/g, "")}${mode}`, int64: mode, roundTrips: true, build: false })
  ),
  3
);
after(() => built.forEach((result) => result.cleanup()));

variants.forEach(({ spec, mode }, index) => {
  it(`round-trips the models of ${spec} with int64 = "${mode}"`, async () => {
    const { sdkDir } = built[index];
    if (!existsSync(join(sdkDir, "tests/api/roundTrips.test.ts"))) {
      assert.notEqual(spec, "torture.yaml", "torture has models");
      return;
    }
    await run(process.execPath, [tsc, "-p", "tsconfig.test.json"], { cwd: sdkDir });
    const test = join(sdkDir, "dist-test/tests/api/roundTrips.test.js");
    await run(process.execPath, ["--test", test], { cwd: sdkDir, maxBuffer: 1 << 26 }).catch((error) => {
      throw new Error(`${error.stdout}${error.stderr}`);
    });
  });
});
