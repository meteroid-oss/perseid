// The unions of the SDK generated from tests/fixtures/edge-unions.yaml: open enums, unions whose
// variants share a JSON type, undiscriminated object unions and union bodies, with the runtime
// helper that decodes them (`unionTry`), against a fake fetch.
import assert from "node:assert/strict";
import { join } from "node:path";
import { after, describe, it } from "node:test";
import { pathToFileURL } from "node:url";
import { generate } from "./_sdk.mjs";

const { dir, sdk, cleanup } = await generate("edge-unions.yaml", { name: "UnionsSdk" });
after(cleanup);
const { unionTry } = await import(
  pathToFileURL(join(dir, "typescript", "dist/esm/unions.js")).href
);

const dateTime = await import(
  pathToFileURL(join(dir, "typescript", "dist/esm/datetime.js")).href
);

/** `value` as JSON: parsing leaves `undefined` in the absent optional fields. */
const plain = (value) => JSON.parse(JSON.stringify(value));

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** A client answering every request with `reply`, and the requests it received. */
function client(reply) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: String(url), method: init.method, body: init.body && JSON.parse(init.body) });
    return json(reply);
  };
  return { calls, api: new sdk.UnionsSdk({ fetch, tokenProvider: () => "tok", maxRetries: 0 }) };
}

/** The method of `resource` whose name starts with `prefix`, bound. */
function method(resource, prefix) {
  const names = Object.getOwnPropertyNames(Object.getPrototypeOf(resource));
  const name = names.find((n) => n.toLowerCase().startsWith(prefix));
  assert.ok(name, `no ${prefix} method among ${names}`);
  return resource[name].bind(resource);
}

const COMPLETION = {
  id: "c1",
  model: "my-finetune",
  created_at: "2024-01-02T03:04:05Z",
  choices: [{ index: 0, finish_reason: "vendor-specific" }],
};

describe("unionTry", () => {
  const date = (v) => new Date(v);
  const dateOrString = [
    { type: "string", parse: (v) => date(v) },
    { type: "string", parse: (v) => v },
  ];

  it("falls through to the next variant when decoding fails", () => {
    assert.ok(unionTry("2024-01-02T03:04:05Z", dateOrString, undefined, false) instanceof Date);
    assert.equal(unionTry("yesterday", dateOrString, undefined, false), "yesterday");
  });

  it("keeps a string that only new Date() would accept as a string", () => {
    const { parseDateTime } = dateTime;
    const candidates = [
      { type: "string", parse: (v) => parseDateTime(v) },
      { type: "string", parse: (v) => v },
    ];
    assert.equal(unionTry("Version 2", candidates, undefined, false), "Version 2");
    assert.equal(unionTry("1", candidates, undefined, false), "1");
    assert.ok(unionTry("2024-01-02T03:04:05Z", candidates, undefined, false) instanceof Date);
  });

  it("tells arrays apart by their first element", () => {
    const tag = (name) => ({ parse: (v) => ({ name, v }) });
    const candidates = [
      { type: "string", ...tag("string") },
      { type: "array", items: { type: "string" }, ...tag("strings") },
      { type: "array", items: { type: "integer" }, ...tag("integers") },
      { type: "array", items: { type: "array", items: { type: "integer" } }, ...tag("matrix") },
    ];
    const pick = (value) => unionTry(value, candidates, undefined, false)?.name;
    assert.equal(pick("hi"), "string");
    assert.equal(pick(["a"]), "strings");
    assert.equal(pick([1, 2]), "integers");
    assert.equal(pick([[1], [2]]), "matrix");
    assert.equal(pick([]), "strings");
    assert.equal(pick([[]]), "matrix");
    assert.equal(pick(1.5), undefined);
    assert.equal(unionTry(null, candidates, undefined, false), null);
  });

  it("keeps an integer apart from a fraction, and reads bigint as an integer", () => {
    const candidates = [
      { type: "integer", parse: () => "integer" },
      { type: "number", parse: () => "number" },
    ];
    assert.equal(unionTry(3, candidates, undefined, false), "integer");
    assert.equal(unionTry(3.5, candidates, undefined, false), "number");
    assert.equal(unionTry(2n ** 60n, candidates, undefined, false), "integer");
  });

  it("matches the empty string variant only for the empty string", () => {
    const candidates = [
      { type: "string", empty: true, parse: () => "empty" },
      { type: "string", parse: () => "string" },
    ];
    assert.equal(unionTry("", candidates, undefined, false), "empty");
    assert.equal(unionTry("x", candidates, undefined, false), "string");
  });

  it("picks the best matching object", () => {
    const candidates = [
      { type: "object", required: ["mode", "tools"], known: ["mode", "tools"], parse: () => "allowed" },
      { type: "object", required: ["type"], known: ["type"], parse: () => "hosted" },
      { type: "object", required: ["name"], known: ["name", "arguments"], parse: () => "function" },
      { type: "object", required: [], known: [], parse: () => "other" },
    ];
    const pick = (value) => unionTry(value, candidates, "best-match", false);
    assert.equal(pick({ mode: "auto", tools: [] }), "allowed");
    assert.equal(pick({ type: "web_search" }), "hosted");
    assert.equal(pick({ name: "f", arguments: "{}" }), "function");
    assert.equal(pick({ unrelated: 1 }), "other");
    // Ties go to the first variant.
    assert.equal(pick({ type: "x", name: "f" }), "hosted");
  });

  it("evaluates the rules of an object union in order", () => {
    const candidates = [
      { type: "object", when: [["type", "a"], ["id"]], parse: () => "a" },
      { type: "object", when: [["type", "b"]], parse: () => "b" },
      { type: "object", parse: () => "catch-all" },
    ];
    const pick = (value) => unionTry(value, candidates, "rules", false);
    assert.equal(pick({ type: "a", id: 1 }), "a");
    assert.equal(pick({ type: "a" }), "catch-all");
    assert.equal(pick({ type: "b" }), "b");
    assert.equal(pick({}), "catch-all");
  });

  it("sends a Date as a string", () => {
    const candidates = [
      { type: "string", parse: (v) => v },
      { type: "array", items: { type: "string" }, parse: (v) => v },
    ];
    const when = new Date(0);
    assert.equal(unionTry(when, candidates, undefined, true), when);
    assert.deepEqual(unionTry([when], candidates, undefined, true), [when]);
  });

  it("keeps a value no variant fits", () => {
    const candidates = [{ type: "string", parse: (v) => v.toUpperCase() }];
    assert.deepEqual(unionTry({ a: 1 }, candidates, undefined, false), { a: 1 });
    assert.throws(() => candidates[0].parse(1));
    assert.equal(unionTry("x", [{ type: "string", parse() { throw new Error("no"); } }], undefined, false), "x");
  });
});

describe("generated unions", () => {
  it("decodes a date-time that may be a free-form string", () => {
    const parsed = sdk.CompletionSerializer.parse({
      ...COMPLETION,
      expires_at: "2024-06-01T00:00:00Z",
    });
    assert.ok(parsed.createdAt instanceof Date);
    assert.ok(parsed.expiresAt instanceof Date);
    const free = sdk.CompletionSerializer.parse({ ...COMPLETION, created_at: "next week", expires_at: null });
    assert.equal(free.createdAt, "next week");
    assert.equal(free.expiresAt, null);
    // Dates are left to `JSON.stringify`, as in every model.
    const out = plain(sdk.CompletionSerializer.serialize(parsed));
    assert.equal(out.created_at, "2024-01-02T03:04:05.000Z");
    assert.equal(sdk.CompletionSerializer.serialize(free).created_at, "next week");
  });

  it("keeps open enum values the SDK does not list", () => {
    const parsed = sdk.CompletionSerializer.parse({ ...COMPLETION, voice: "ash", voice_open: "my-voice" });
    assert.equal(parsed.model, "my-finetune");
    assert.equal(parsed.choices[0].finishReason, "vendor-specific");
    assert.equal(parsed.voice, "ash");
    assert.equal(parsed.voiceOpen, "my-voice");
    assert.equal(sdk.CompletionSerializer.serialize(parsed).voice_open, "my-voice");
  });

  it("round-trips the prompt-like union and the numbers", () => {
    for (const prompt of ["hi", ["a", "b"], [1, 2], [[1], [2, 3]], []]) {
      const parsed = sdk.CompletionSerializer.parse({ ...COMPLETION, prompt, score: 1, ratio: 2.5, stop: null });
      assert.deepEqual(parsed.prompt, prompt);
      assert.deepEqual(sdk.CompletionSerializer.serialize(parsed).prompt, prompt);
      assert.equal(parsed.ratio, 2.5);
    }
  });

  it("sends and receives union bodies and prompts", async () => {
    for (const prompt of ["hi", ["a"], [1], [[1], [2]]]) {
      const { api, calls } = client(COMPLETION);
      const completion = await method(api.completions, "create")({ model: "any-model", prompt });
      assert.deepEqual(calls[0].body, { model: "any-model", prompt });
      assert.ok(completion.createdAt instanceof Date);
    }
  });

  it("decodes object unions without a distinguishing property", () => {
    const base = { id: "r1", model: "m" };
    for (const tool_choice of [
      "auto",
      { mode: "required", tools: [{ type: "x" }] },
      { type: "web_search" },
      { name: "f", arguments: "{}" },
    ]) {
      // `Response` would shadow the fetch global: the model is `ResponseModel`.
      const parsed = sdk.ResponseModelSerializer.parse({ ...base, tool_choice });
      assert.deepEqual(plain(parsed.toolChoice), tool_choice);
      assert.deepEqual(plain(sdk.ResponseModelSerializer.serialize(parsed).tool_choice), tool_choice);
    }
  });

  it("types a union response body", async () => {
    const verbose = { text: "t", duration: 1.5, language: "en", segments: [{ id: 1, start: 0, end: 1 }] };
    const { api } = client(verbose);
    const result = await method(api.transcriptions, "create")({ model: "m", file_id: "f" });
    assert.equal(result.text, "t");
    assert.equal(result.language, "en");
    assert.deepEqual(plain(result.segments), verbose.segments);
  });

  it("sends a union request body", async () => {
    const { api, calls } = client({ id: "g1", result: { score: 0.5 } });
    const grade = await method(api.grades, "create")({ score: 0.5, threshold: 0.2 });
    assert.deepEqual(calls[0].body, { score: 0.5, threshold: 0.2 });
    assert.deepEqual(plain(grade.result), { score: 0.5 });
  });

  it("types a field that cannot be typed without untyping its model", () => {
    const parsed = sdk.UntypableSerializer.parse({ name: "n", mixed: "auto", count: 3 });
    assert.equal(parsed.name, "n");
    assert.equal(parsed.count, 3);
    assert.equal(parsed.mixed, "auto");
  });
});
