// Raw responses, errors, retries and environment variables of the petstore client.
import assert from "node:assert/strict";
import { test } from "node:test";
import * as sdk from "../dist/esm/index.js";

const PET = { id: "1", name: "Rex", created_at: "2024-01-01T00:00:00Z", color: "red" };

function client(responses, options = {}) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: new URL(url), init });
    const next = responses[Math.min(calls.length - 1, responses.length - 1)];
    return typeof next === "function" ? next(init) : next.clone();
  };
  return { calls, petstore: new sdk.Petstore({ apiKey: "token", fetch, ...options }) };
}

const json = (body, status = 200, headers = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "x-request-id": "req_1", ...headers } });

test("withResponse gives the parsed body with the response, asResponse the response alone", async () => {
  const { petstore } = client([json(PET)]);
  const { data, response, requestId } = await petstore.pets.retrieve("1").withResponse();
  assert.equal(data.name, "Rex");
  assert.equal(response.status, 200);
  assert.equal(requestId, "req_1");
  const raw = await petstore.pets.retrieve("1").asResponse();
  assert.deepEqual(await raw.json(), PET);
});

test("models keep the properties the SDK does not know and send them back", async () => {
  const { petstore } = client([json(PET)]);
  const pet = await petstore.pets.retrieve("1");
  assert.equal(pet.color, "red");
  assert.equal(sdk.PetSerializer.serialize(pet).color, "red");
  assert.equal("created_at" in pet, false);
});

test("every error is a PetstoreError, API errors parse their body as JSON", async () => {
  const { petstore } = client([json({ message: "nope" }, 404)]);
  await assert.rejects(petstore.pets.retrieve("1"), (error) => {
    assert.ok(error instanceof sdk.NotFoundError && error instanceof sdk.APIError);
    assert.ok(error instanceof sdk.PetstoreError);
    assert.equal(error.name, "NotFoundError");
    assert.deepEqual(error.error, { message: "nope" });
    assert.equal(error.requestId, "req_1");
    return true;
  });
});

test("connection failures, timeouts, aborts and bad bodies are SDK errors", async () => {
  const refused = client([() => Promise.reject(new TypeError("fetch failed"))], { maxRetries: 0 });
  await assert.rejects(refused.petstore.pets.retrieve("1"), (error) => {
    assert.ok(error instanceof sdk.APIConnectionError && error instanceof sdk.PetstoreError);
    assert.ok(!(error instanceof sdk.APIConnectionTimeoutError));
    assert.equal(error.cause.message, "fetch failed");
    return true;
  });

  const hang = (init) =>
    new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason)));
  const slow = client([hang], { maxRetries: 0 });
  const alive = setInterval(() => {}, 1000);
  await assert.rejects(slow.petstore.pets.retrieve("1", { timeout: 5 }), (error) => {
    assert.ok(error instanceof sdk.APIConnectionTimeoutError && error instanceof sdk.APIConnectionError);
    assert.equal(error.name, "APIConnectionTimeoutError");
    return true;
  });
  clearInterval(alive);

  const controller = new AbortController();
  controller.abort();
  await assert.rejects(
    slow.petstore.pets.retrieve("1", { signal: controller.signal }),
    sdk.APIUserAbortError
  );

  const garbled = client([new Response("{not json", { status: 200 })]);
  await assert.rejects(garbled.petstore.pets.retrieve("1"), (error) => {
    assert.ok(error instanceof sdk.APIDecodeError);
    assert.equal(error.body, "{not json");
    return true;
  });
});

test("retries honour retry-after-ms and the per-request maxRetries", async () => {
  const { calls, petstore } = client([json({}, 503, { "retry-after-ms": "1" }), json(PET)]);
  assert.equal((await petstore.pets.retrieve("1")).id, "1");
  assert.equal(calls.length, 2);

  const once = client([json({}, 503, { "retry-after-ms": "1" })], { maxRetries: 5 });
  await assert.rejects(once.petstore.pets.retrieve("1", { maxRetries: 1 }), sdk.InternalServerError);
  assert.equal(once.calls.length, 2);
});

test("a 429 is retried after its Retry-After, and a 204 resolves to nothing", async () => {
  const { calls, petstore } = client([json({}, 429, { "retry-after": "0" }), new Response(null, { status: 204 })]);
  await petstore.pets.delete("1");
  assert.equal(calls.length, 2);
});

test("the API key and base URL come from the environment unless given", async () => {
  process.env.PETSTORE_API_KEY = "from-env";
  process.env.PETSTORE_BASE_URL = "https://env.test/v9";
  try {
    const seen = [];
    const fetch = async (url, init) => {
      seen.push({ url: new URL(url), init });
      return json(PET);
    };
    await new sdk.Petstore({ fetch }).pets.retrieve("1");
    await new sdk.Petstore({ apiKey: "arg", baseURL: "https://arg.test", fetch }).pets.retrieve("1");
    delete process.env.PETSTORE_BASE_URL;
    await new sdk.Petstore({ fetch }).pets.retrieve("1");
    assert.equal(seen[0].url.href, "https://env.test/v9/pets/1");
    assert.equal(seen[0].init.headers.authorization, "Bearer from-env");
    assert.equal(seen[1].url.href, "https://arg.test/pets/1");
    assert.equal(seen[1].init.headers.authorization, "Bearer arg");
    assert.equal(seen[2].url.origin, "https://petstore.example.com");
  } finally {
    delete process.env.PETSTORE_API_KEY;
    delete process.env.PETSTORE_BASE_URL;
  }
});
