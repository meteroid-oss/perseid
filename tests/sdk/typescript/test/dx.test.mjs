// An SDK of an inline spec shaped like OpenAI's: union variants sharing a tag, closed and open
// enums, a stream twin, error events, keepalives, multipart files and a key every operation
// requires.
import assert from "node:assert/strict";
import { createReadStream, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, describe, it } from "node:test";
import { generate, run, tsc } from "./_sdk.mjs";

const SPEC = `
openapi: 3.1.0
info: { title: Dx, version: "1" }
security: [{ key: [] }]
paths:
  /items:
    post:
      operationId: create_items
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: '#/components/schemas/Holder' } } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/Holder' } } } }
  /job:
    get:
      operationId: get_job
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/Status' } } } }
        default: { description: error, content: { application/json: { schema: { $ref: '#/components/schemas/Error' } } } }
  /completions:
    post:
      operationId: create_completion
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: '#/components/schemas/CompletionRequest' } } }
      responses:
        "200":
          description: the completion, or its chunks with \`stream\`
          content:
            application/json: { schema: { $ref: '#/components/schemas/Chunk' } }
            text/event-stream: { schema: { $ref: '#/components/schemas/Chunk' } }
  /files:
    post:
      operationId: upload_file
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, purpose]
              properties:
                file: { type: string, format: binary }
                purpose: { $ref: '#/components/schemas/Purpose' }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/Upload' } } } }
  /images:
    post:
      operationId: upload_image
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file]
              properties: { file: { type: string, format: binary } }
            encoding: { file: { contentType: image/png } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/Upload' } } } }
  /reports:
    get:
      operationId: watch_reports
      responses:
        "200": { description: reports, content: { text/event-stream: { schema: { $ref: '#/components/schemas/Report' } } } }
components:
  securitySchemes:
    key: { type: http, scheme: bearer }
  schemas:
    Holder:
      type: object
      required: [items]
      properties:
        items: { type: array, items: { $ref: '#/components/schemas/InputItem' } }
        effort: { $ref: '#/components/schemas/Effort' }
        model: { $ref: '#/components/schemas/Model' }
    InputItem:
      oneOf: [{ $ref: '#/components/schemas/Easy' }, { $ref: '#/components/schemas/Item' }]
      discriminator: { propertyName: type }
    Item:
      oneOf: [{ $ref: '#/components/schemas/Input' }, { $ref: '#/components/schemas/Output' }, { $ref: '#/components/schemas/Call' }]
      discriminator: { propertyName: type }
    Easy:
      type: object
      required: [content]
      properties: { type: { type: string, enum: [message] }, content: { type: string } }
    Input:
      type: object
      required: [role]
      properties: { type: { type: string, enum: [message] }, role: { type: string } }
    Output:
      type: object
      required: [id, type]
      properties: { type: { type: string, enum: [message] }, id: { type: string }, output_text: { type: string } }
    Call:
      type: object
      required: [type, call_id]
      properties: { type: { type: string, enum: [call] }, call_id: { type: string } }
    Effort: { type: string, enum: [low, high] }
    Model:
      anyOf: [{ type: string }, { type: string, enum: [small, large] }]
    Status:
      type: object
      required: [state]
      properties: { state: { $ref: '#/components/schemas/State' } }
    State: { type: string, enum: [queued, done] }
    Error:
      type: object
      properties: { error: { type: object, properties: { message: { type: string } } } }
    CompletionRequest:
      type: object
      required: [prompt]
      properties: { prompt: { type: string }, stream: { type: boolean } }
    Chunk:
      type: object
      required: [text]
      properties: { text: { type: string } }
    Report:
      type: object
      required: [text]
      properties: { text: { type: string }, error: { type: string } }
    Purpose: { type: string, enum: [batch, fine-tune] }
    Upload:
      type: object
      required: [id]
      properties: { id: { type: string } }
`;

const { sdk, sdkDir, cleanup } = await generate(undefined, { name: "Dx", text: SPEC });
after(cleanup);

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const events = (body) => new Response(body, { headers: { "content-type": "text/event-stream" } });

/** A client whose fetch answers `reply`, and the requests it received. */
function client(reply, options = {}) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url: String(url), headers: init.headers, body: init.body });
    return typeof reply === "function" ? reply() : reply.clone();
  };
  return { calls, dx: new sdk.Dx({ apiKey: "sk", fetch, maxRetries: 0, ...options }) };
}

async function failure(call) {
  try {
    await call();
  } catch (error) {
    return error;
  }
  return assert.fail("expected the call to fail");
}

describe("union variants sharing a tag", () => {
  it("are sent with the tag they declare, picked by their properties", async () => {
    const { calls, dx } = client(json({ items: [] }));
    await dx.items.create({
      items: [
        { content: "hi" },
        { type: "message", role: "user" },
        { type: "message", id: "o1", outputText: "x" },
        { type: "call", callId: "c1" },
      ],
    });
    assert.deepEqual(JSON.parse(calls[0].body).items, [
      { content: "hi", type: "message" },
      { role: "user", type: "message" },
      { id: "o1", output_text: "x", type: "message" },
      { call_id: "c1", type: "call" },
    ]);
  });

  it("are decoded as the variant the properties fit", async () => {
    const items = [
      { type: "message", content: "a" },
      { type: "message", role: "r" },
      { type: "message", id: "i", output_text: "t" },
      { role: "untagged" },
    ];
    const { dx } = client(json({ items }));
    const holder = await dx.items.create({ items: [] });
    assert.deepEqual(JSON.parse(JSON.stringify(holder.items)), [
      { content: "a", type: "message" },
      { role: "r", type: "message" },
      { id: "i", outputText: "t", type: "message" },
      { role: "untagged", type: "message" },
    ]);
  });
});

describe("the client", () => {
  it("is the default export too", () => {
    assert.equal(sdk.default, sdk.Dx);
  });

  it("requires a key when every operation does", () => {
    const fetch = async () => json({});
    const error = (() => {
      try {
        new sdk.Dx({ fetch });
      } catch (error) {
        return error;
      }
    })();
    assert.ok(error instanceof sdk.DxError);
    assert.match(error.message, /DX_API_KEY/);
    assert.ok(new sdk.Dx({ fetch, tokenProvider: () => "t" }));
    assert.ok(new sdk.Dx({ fetch, defaultHeaders: { Authorization: "Bearer t" } }));
  });

  it("copies itself with other options", async () => {
    const { calls, dx } = client(json({ state: "done" }));
    await dx.withOptions({ defaultHeaders: { "x-extra": "1" } }).job.retrieve();
    assert.equal(calls[0].headers["x-extra"], "1");
    assert.equal(calls[0].headers.authorization, "Bearer sk");
  });

  it("reads the message of a JSON error body, keeping the body", async () => {
    const body = { error: { message: "No such thing" } };
    const { dx } = client(json(body, 404));
    const error = await failure(() => dx.job.retrieve());
    assert.ok(error instanceof sdk.NotFoundError);
    assert.equal(error.message, "API error 404: No such thing");
    assert.deepEqual(JSON.parse(error.body), body);
    assert.equal(error.error.error.message, "No such thing");
  });

  it("keeps the values a later API version adds to a closed enum of a response", async () => {
    const { dx } = client(json({ state: "archived" }));
    assert.equal((await dx.job.retrieve()).state, "archived");
  });
});

describe("event streams", () => {
  const read = async (stream) => {
    const texts = [];
    for await (const chunk of stream) {
      texts.push(chunk.text);
    }
    return texts;
  };

  it("raise an API error for an error event or data", async () => {
    for (const body of [
      'data: {"text":"a"}\n\ndata: {"error":{"message":"overloaded"}}\n\n',
      'data: {"text":"a"}\n\nevent: error\ndata: {"message":"overloaded"}\n\n',
    ]) {
      const { dx } = client(() => events(body));
      const stream = await dx.completions.createStream({ prompt: "x" });
      const texts = [];
      const error = await failure(async () => {
        for await (const chunk of stream) {
          texts.push(chunk.text);
        }
      });
      assert.deepEqual(texts, ["a"]);
      assert.ok(error instanceof sdk.APIError, String(error));
      assert.match(error.message, /overloaded/);
    }
  });

  it("raise an API error for an error event whatever its data, or an error the model does not declare", async () => {
    for (const body of [
      "event: error\ndata: overloaded\n\n",
      'data: {"text":"b","error":{"message":"overloaded"}}\n\n',
    ]) {
      const { dx } = client(() => events(`data: {"text":"a"}\n\n${body}`));
      const texts = [];
      const error = await failure(async () => {
        for await (const chunk of await dx.completions.createStream({ prompt: "x" })) {
          texts.push(chunk.text);
        }
      });
      assert.deepEqual(texts, ["a"]);
      assert.ok(error instanceof sdk.APIError, String(error));
      assert.match(error.message, /overloaded/);
    }
  });

  it("keep the error property a model declares, but not error events", async () => {
    const { dx } = client(() => events('data: {"text":"a","error":"partial"}\n\n'));
    const reports = [];
    for await (const report of await dx.reports.list()) {
      reports.push(report);
    }
    assert.deepEqual(reports, [{ text: "a", error: "partial" }]);
    const failing = client(() => events('event: error\ndata: {"error":"overloaded"}\n\n'));
    const error = await failure(async () => {
      for await (const report of await failing.dx.reports.list()) {
        void report;
      }
    });
    assert.ok(error instanceof sdk.APIError, String(error));
  });

  it("skip keepalives that are not items", async () => {
    const body = 'event: ping\ndata: alive\n\nevent: keepalive\ndata: {}\n\nevent: ping\ndata: {"text":"a"}\n\ndata: {"text":"b"}\n\n';
    const { dx } = client(() => events(body));
    assert.deepEqual(await read(await dx.completions.createStream({ prompt: "x" })), ["a", "b"]);
  });

  it("decode the other events", async () => {
    const { dx } = client(() => events('data: {"text":"a"}\n\ndata: {"text":"b"}\n\ndata: [DONE]\n\n'));
    assert.deepEqual(await read(await dx.completions.createStream({ prompt: "x" })), ["a", "b"]);
  });
});

describe("multipart files", () => {
  const sent = async (file) => {
    const { calls, dx } = client(json({ id: "f" }));
    await dx.files.upload({ file, purpose: "batch" });
    return calls[0].body.text();
  };
  const path = join(sdkDir, "notes.jsonl");
  writeFileSync(path, "from disk");

  it("take bytes, streams, responses and paths", async () => {
    const bytes = new TextEncoder().encode("bytes");
    const cases = [
      [bytes.buffer, 'filename="file"', "bytes"],
      [new Blob(["stream"]).stream(), 'filename="file"', "stream"],
      [Object.defineProperty(new Response("fetched"), "url", { value: "https://x.test/a/data.csv" }), 'filename="data.csv"', "fetched"],
      [{ path }, 'filename="notes.jsonl"', "from disk"],
      [createReadStream(path), 'filename="notes.jsonl"', "from disk"],
      [{ data: bytes, filename: "named.bin", contentType: "text/plain" }, 'filename="named.bin"\r\nContent-Type: text/plain', "bytes"],
    ];
    for (const [file, header, content] of cases) {
      const body = await sent(file);
      assert.ok(body.includes(header), body);
      assert.ok(body.includes(`\r\n\r\n${content}\r\n`), body);
      assert.ok(body.includes('name="purpose"\r\n\r\nbatch'), body);
    }
  });

  it("type a path by the spec, else by its extension", async () => {
    assert.ok((await sent({ path })).includes("Content-Type: application/jsonl\r\n"));
    const { calls, dx } = client(json({ id: "f" }));
    await dx.images.upload({ file: { path } });
    assert.ok((await calls[0].body.text()).includes("Content-Type: image/png\r\n"));
  });
});

describe("types", () => {
  it("reject what the API does not take, and take what it sends", async () => {
    writeFileSync(
      join(sdkDir, "src", "dxProbe.ts"),
      `import Dx, { type Holder, type Status } from "./index.js";

export async function probe(dx: Dx): Promise<void> {
  await dx.items.create({ items: [{ content: "no tag" }, { type: "message", role: "user" }], effort: "low", model: "custom-model" });
  // @ts-expect-error a typo of a closed enum
  const typo: Holder = { items: [], effort: "hgh" };
  // @ts-expect-error the stream twin asks for the stream
  await dx.completions.create({ prompt: "x", stream: true });
  await dx.completions.create({ prompt: "x", stream: false });
  // @ts-expect-error a closed enum of a request
  await dx.files.upload({ file: new Blob([]), purpose: "fine_tune" });
  const later: Status = { state: "archived" };
  const state: string = (await dx.job.retrieve()).state;
  void typo; void later; void state;
}
`
    );
    await run(process.execPath, [tsc, "-p", ".", "--noEmit"], { cwd: sdkDir });
  });
});
