import assert from "node:assert/strict";
import { test } from "node:test";
import { Petstore } from "../dist/esm/index.js";

const PET = { id: "1", name: "Rex", created_at: "2024-01-01T00:00:00Z" };

function origin(seen) {
  return async (request, _next) => {
    seen.push(request);
    return Response.json(PET);
  };
}

function cache() {
  const store = new Map();
  return async (request, next) => {
    if (request.method !== "GET") {
      return next(request);
    }
    const hit = store.get(request.url);
    if (hit) {
      return new Response(hit, { headers: { "content-type": "application/json" } });
    }
    const response = await next(request);
    if (response.ok) {
      store.set(request.url, await response.clone().text());
    }
    return response;
  };
}

test("a cache middleware answers repeated GETs without reaching the origin", async () => {
  const seen = [];
  const petstore = new Petstore({ apiKey: "token", middleware: [cache(), origin(seen)] });
  for (let i = 0; i < 3; i++) {
    assert.equal((await petstore.pets.retrieve("1")).name, "Rex");
  }
  assert.equal(seen.length, 1);
  await petstore.pets.retrieve("2");
  assert.equal(seen.length, 2);
});

test("middleware runs in order and can change the request", async () => {
  const seen = [];
  const tag = (request, next) => {
    request.headers.set("x-tag", "yes");
    return next(request);
  };
  const petstore = new Petstore({ apiKey: "token", middleware: [tag, origin(seen)] });
  await petstore.pets.retrieve("1");
  assert.equal(seen[0].headers.get("x-tag"), "yes");
  assert.equal(seen[0].headers.get("authorization"), "Bearer token");
});

test("middleware composes with a custom fetch", async () => {
  let reached = 0;
  const fetch = async () => {
    reached++;
    return Response.json(PET);
  };
  const petstore = new Petstore({ apiKey: "token", fetch, middleware: [cache()] });
  await petstore.pets.retrieve("1");
  await petstore.pets.retrieve("1");
  assert.equal(reached, 1);
});
