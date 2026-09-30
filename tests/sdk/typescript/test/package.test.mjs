import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";

const { name } = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

test("the exports map serves ESM to import and CommonJS to require", async () => {
  const esm = await import(name);
  const cjs = createRequire(import.meta.url)(name);
  for (const sdk of [esm, cjs]) {
    assert.equal(typeof sdk.Petstore, "function");
    assert.equal(typeof sdk.ApiException, "function");
    assert.equal(sdk.PetStatus.Available, "available");
    assert.equal(typeof sdk.PetSerializer.parse, "function");
  }
  assert.notEqual(esm.Petstore, cjs.Petstore);
});

test("enums are string literal unions with a constant object of their values", async () => {
  const { PetStatus } = await import(name);
  assert.deepEqual(Object.values(PetStatus), ["available", "pending", "sold"]);
});

test("models parse and serialize under their public names", async () => {
  const { PetSerializer } = await import(name);
  const pet = PetSerializer.parse({ id: "1", name: "Rex", created_at: "2024-01-01T00:00:00Z" });
  assert.equal(pet.createdAt.toISOString(), "2024-01-01T00:00:00.000Z");
  assert.deepEqual(JSON.parse(JSON.stringify(PetSerializer.serialize(pet))), {
    id: "1",
    name: "Rex",
    created_at: "2024-01-01T00:00:00.000Z",
  });
  assert.deepEqual(PetSerializer._fromJsonObject({ id: "1", name: "Rex", created_at: "2024-01-01T00:00:00Z" }), pet);
});
