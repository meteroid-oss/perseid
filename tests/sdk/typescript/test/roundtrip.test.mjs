// Schema-derived round trips: for every model of the torture fixture and of every edge fixture
// with models, `perseid samples` writes valid JSON instances; each one is decoded with the
// generated serializer, encoded again and compared with the original, in the default
// `int64 = "number"` mode and in `int64 = "bigint"` mode.
//
// The only normalizations are the documented ones: a date-time is compared as an instant (the SDK
// has millisecond precision and writes `Z`), and in `number` mode integers are compared as
// numbers, since an int64 beyond 2^53 cannot be exact there. Everything else must be equal,
// including every digit of an int64 in `bigint` mode.
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { after, describe, it } from "node:test";
import { fixtures, generate, pool, readExact, writeExact } from "./_sdk.mjs";

const SPECS = readdirSync(fixtures)
  .filter((file) => file === "torture.yaml" || /^edge-.*\.yaml$/.test(file))
  .sort();
const MODES = ["number", "bigint"];

const DATE_TIME =
  /^(\d{4}-\d{2}-\d{2})[Tt ](\d{2}:\d{2})(?::(\d{2}))?(?:\.(\d+))?([Zz]|[+-]\d{2}(?::?\d{2})?)?$/;

/** A date-time string as `{ instant }` in milliseconds; anything else is left alone. */
function instant(text) {
  const match = DATE_TIME.exec(text);
  if (match === null) {
    return text;
  }
  const [, date, time, seconds = "00", fraction = "", offset = "Z"] = match;
  const millis = fraction.slice(0, 3).padEnd(3, "0");
  let zone = "Z";
  if (offset !== "Z" && offset !== "z") {
    const digits = offset.slice(1).replace(":", "");
    zone = `${offset[0]}${digits.slice(0, 2)}:${digits.slice(2, 4) || "00"}`;
  }
  const stamp = Date.parse(`${date}T${time}:${seconds}.${millis}${zone}`);
  return Number.isNaN(stamp) ? text : { instant: stamp };
}

/** The value with the documented normalizations applied, to compare with `deepStrictEqual`. */
function normalize(value, mode) {
  if (typeof value === "string") {
    return instant(value);
  }
  if (typeof value === "bigint") {
    return mode === "number" ? Number(value) : value;
  }
  if (typeof value === "number") {
    // `5.0` and `5` are the same JSON number.
    return mode === "bigint" && Number.isInteger(value) ? BigInt(value) : value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => normalize(item, mode));
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, normalize(item, mode)]));
  }
  return value;
}

/** Decodes the JSON text like the client does, then encodes it again. */
function roundTrip(sdk, typeName, text) {
  const serializer = sdk[`${typeName}Serializer`];
  const decoded = sdk.parseJson(text);
  if (serializer === undefined) {
    // Aliases and unions of plain JSON values have no serializer: the value is its JSON.
    return sdk.stringifyJson(decoded);
  }
  return sdk.stringifyJson(serializer.serialize(serializer.parse(decoded)));
}

const variants = SPECS.flatMap((spec) => MODES.map((mode) => ({ spec, mode })));
const built = await pool(
  variants.map(({ spec, mode }) => async () => {
    const name = `Rt${spec.replace(/\W/g, "")}${mode}`.replace(/yaml/, "");
    return generate(spec, { name, int64: mode, samples: true });
  }),
  3
);
after(() => built.forEach((result) => result.cleanup()));

variants.forEach(({ spec, mode }, index) => {
  const { models, sdk } = built[index];
  describe(`${spec} (int64 = ${mode})`, () => {
    if (sdk === undefined) {
      it("has no models", () => assert.equal(Object.keys(models).length, 0));
      return;
    }
    for (const [schema, model] of Object.entries(models)) {
      const typeName = model.names.typescript;
      it(`round-trips ${schema} (${typeName})`, () => {
        assert.ok(model.samples.length > 0, "the model has samples");
        for (const sample of model.samples) {
          const text = writeExact(sample.json);
          const out = roundTrip(sdk, typeName, text);
          assert.deepStrictEqual(
            normalize(readExact(out), mode),
            normalize(sample.json, mode),
            `${typeName} sample ${sample.name}: ${text} came back as ${out}`
          );
        }
      });
    }
  });
});

describe("the samples cover every kind of model", () => {
  it("has structs, enums, unions and aliases in the torture fixture", () => {
    const index = SPECS.indexOf("torture.yaml");
    assert.ok(index >= 0, "torture.yaml is a fixture");
    const { models } = built[variants.findIndex(({ spec }) => spec === "torture.yaml")];
    const kinds = new Set(Object.values(models).map((model) => model.kind));
    for (const kind of ["struct", "enum", "union", "alias"]) {
      assert.ok(kinds.has(kind), `a model of kind ${kind}`);
    }
  });
});
