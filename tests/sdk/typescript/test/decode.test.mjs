// Response bodies checked against their schema, and kept as received with `validate_responses = false`.
import assert from "node:assert/strict";
import { after, test } from "node:test";
import * as sdk from "../dist/esm/index.js";
import { generate } from "./_sdk.mjs";

const PET = { id: "1", name: "Rex", created_at: "2024-01-01T00:00:00Z" };

function answering(SDK, body) {
  const fetch = async () => new Response(JSON.stringify(body), { status: 200 });
  return new SDK.Petstore({ apiKey: "token", fetch, maxRetries: 0 });
}

async function decodeFailure(SDK, call, message) {
  await assert.rejects(call, (error) => {
    assert.ok(error instanceof SDK.APIDecodeError, `${error}`);
    assert.ok(error instanceof SDK.PetstoreError);
    assert.equal(error.message, `The response body does not match its schema: ${message}`);
    assert.equal(typeof error.body, "string");
    return true;
  });
}

test("a wrong type, a missing required property or another shape is an APIDecodeError", async () => {
  const retrieve = (body) => answering(sdk, body).pets.retrieve("1");
  await decodeFailure(sdk, retrieve({ ...PET, name: 42 }), "$.name: expected a string, got number 42");
  await decodeFailure(sdk, retrieve({ id: "1", created_at: PET.created_at }), "$.name: required, missing");
  await decodeFailure(sdk, retrieve({ ...PET, name: null }), "$.name: expected a string, got null");
  await decodeFailure(sdk, retrieve("abc"), '$: expected an object, got string "abc"');
  await decodeFailure(sdk, retrieve({ ...PET, created_at: "soon" }), '$.created_at: expected a date-time, got string "soon"');
  const list = answering(sdk, { data: [PET, { ...PET, created_at: undefined }] }).pets.list();
  await decodeFailure(sdk, list, "$.data[1].created_at: required, missing");
  await decodeFailure(sdk, answering(sdk, { data: {} }).pets.list(), "$.data: expected an array, got an object");
});

test("optional nulls, unknown enum values and unknown properties are accepted", async () => {
  const pet = await answering(sdk, { ...PET, tag: null, status: "adopted", color: "red" }).pets.retrieve("1");
  assert.equal(pet.tag, undefined);
  assert.equal(pet.status, "adopted");
  assert.equal(pet.color, "red");
  assert.ok(pet.createdAt instanceof Date);
});

test("serializers check the values they parse too, with their JSON path", () => {
  assert.throws(() => sdk.PetSerializer.parse({ ...PET, id: 1 }), {
    name: "TypeError",
    message: "$.id: expected a string, got number 1",
  });
});

const lenient = await generate("petstore.yaml", { name: "Petstore", settings: "validate_responses = false\n" });
after(lenient.cleanup);

test("validate_responses = false keeps bodies as received, other shapes still being APIDecodeErrors", async () => {
  const pet = await answering(lenient.sdk, { ...PET, name: 42 }).pets.retrieve("1");
  assert.equal(pet.name, 42);
  const missing = await answering(lenient.sdk, { id: "1", created_at: PET.created_at }).pets.retrieve("1");
  assert.equal(missing.name, undefined);
  await assert.rejects(answering(lenient.sdk, { data: [null] }).pets.list(), (error) => {
    assert.ok(error instanceof lenient.sdk.APIDecodeError, `${error}`);
    assert.ok(error.cause instanceof TypeError);
    return true;
  });
});
