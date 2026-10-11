// The behaviour every sandbox backend shares, run against a fake client.
import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { execute, type Limits, type Sandbox } from "../index.js";
import { denoSandbox } from "../sandbox/deno.js";
import { quickjsSandbox } from "../sandbox/quickjs.js";
import { quickjsThreadsSandbox } from "../sandbox/threads.js";
import { remoteSandbox } from "../sandbox/remote.js";
import { serveRunner } from "../runner.js";

const TOKEN = "a-runner-token-of-at-least-32-characters";
const runner = serveRunner({ port: 0, host: "127.0.0.1", token: TOKEN, sandbox: quickjsThreadsSandbox({ threads: 2 }) });
runner.unref();
await new Promise((resolve) => runner.once("listening", resolve));
const RUNNER = `http://127.0.0.1:${(runner.address() as { port: number }).port}`;

export const BACKENDS: [string, () => Sandbox][] = [
  ["deno", () => denoSandbox()],
  ["quickjs", () => quickjsSandbox()],
  ["quickjs in threads", () => quickjsThreadsSandbox({ threads: 2 })],
  ["remote runner", () => remoteSandbox({ url: RUNNER, token: TOKEN })],
];

test("the runner refuses requests without its token", async () => {
  const response = await fetch(`${RUNNER}/run`, { method: "POST", body: "{}" });
  assert.equal(response.status, 401);
});

const LIMITS: Limits = { timeoutMs: 10_000, memoryMb: 128, calls: 10 };

class Page {
  constructor(
    public items: unknown[],
    private more: boolean,
  ) {}
  hasNextPage() {
    return this.more;
  }
}

/** A client of things: `retrieve`, `list` (a page, and every item with `for await`), `fail`. */
export function fakeClient() {
  const calls: unknown[][] = [];
  const things = {
    retrieve: async (id: string) => {
      calls.push(["retrieve", id]);
      return { id, name: `thing ${id}` };
    },
    list(options: { limit?: number } = {}) {
      calls.push(["list", options]);
      const all = Array.from({ length: 250 }, (_, i) => ({ id: `t${i}` }));
      const first = Promise.resolve(new Page(all.slice(0, options.limit ?? 10), true));
      return Object.assign(first, {
        async *[Symbol.asyncIterator]() {
          yield* all;
        },
      });
    },
    fail: async () => {
      throw Object.assign(new Error("404 Not Found"), { status: 404, body: '{"error":"no such thing"}' });
    },
    _client: { post: async () => "private" },
  };
  return { client: { things }, calls };
}

for (const [name, make] of BACKENDS) {
  describe(`the ${name} sandbox`, () => {
    const sandbox = make();
    const run = (code: string, limits: Partial<Limits> = {}) => {
      const { client, calls } = fakeClient();
      return execute(code, client, sandbox, { ...LIMITS, ...limits }).then((outcome) => ({ ...outcome, made: calls }));
    };

    test("returns what the script returns, and what it logs", async () => {
      const outcome = await run(`console.log("hello", { a: 1 }); return { ok: true };`);
      assert.deepEqual(outcome.value, { ok: true });
      assert.deepEqual(outcome.logs, ['hello {\n  "a": 1\n}']);
      assert.equal(outcome.error, undefined);
    });

    test("strips TypeScript", async () => {
      const outcome = await run(`const n: number = 2; interface X { a: string } return n as number * 2;`);
      assert.equal(outcome.value, 4);
    });

    test("calls the client on the host", async () => {
      const outcome = await run(`const t = await client.things.retrieve("t1"); return t.name;`);
      assert.equal(outcome.value, "thing t1");
      assert.deepEqual(outcome.made, [["retrieve", "t1"]]);
      assert.equal(outcome.calls, 1);
    });

    test("gives a list's first page, and every item with for await", async () => {
      const page = await run(`const p = await client.things.list({ limit: 3 }); return p;`);
      assert.deepEqual(page.value, { items: [{ id: "t0" }, { id: "t1" }, { id: "t2" }], more: true, hasNextPage: true });
      const all = await run(`let n = 0; for await (const t of client.things.list()) n++; return n;`);
      assert.equal(all.value, 250);
      assert.equal(all.calls, 4, "the list, then three batches, each counted before it is fetched");
    });

    test("throws the API's errors with their status and body", async () => {
      const outcome = await run(`try { await client.things.fail(); } catch (e) { return [e.status, e.body, e.message]; }`);
      assert.deepEqual(outcome.value, [404, { error: "no such thing" }, "404 Not Found"]);
      const uncaught = await run(`await client.things.fail();`);
      assert.equal(uncaught.error, "404 Not Found (status 404)");
    });

    test("refuses private members and unknown methods", async () => {
      const outcome = await run(`try { await client.things._client.post(); } catch (e) { return e.message; }`);
      assert.match(String(outcome.value), /is not a method/);
      const unknown = await run(`return await client.nothing.here();`);
      assert.match(String(unknown.error), /client\.nothing\.here is not a method/);
    });

    test("stops at the call limit", async () => {
      const outcome = await run(`for (let i = 0; i < 5; i++) await client.things.retrieve("t" + i); return 1;`, { calls: 3 });
      assert.match(String(outcome.error), /more than 3 API calls/);
      assert.equal(outcome.made.length, 3);
    });

    test("stops at the time limit", async () => {
      const outcome = await run(`while (true) {}`, { timeoutMs: 500 });
      assert.match(String(outcome.error), /longer than 500 ms/);
    });

    test("has no network, files or environment", async () => {
      const net = await run(`try { await fetch("https://example.com"); return "reached"; } catch (e) { return "refused"; }`);
      assert.equal(net.value, "refused");
      const env = await run(`try { return typeof Deno === "undefined" ? "none" : Deno.env.get("HOME") ?? "none"; } catch { return "refused"; }`);
      assert.notEqual(env.value, process.env.HOME);
      const files = await run(`try { return typeof Deno === "undefined" ? "none" : Deno.readTextFileSync("/etc/passwd"); } catch { return "refused"; }`);
      assert.ok(files.value === "refused" || files.value === "none");
    });

    test("stops at the memory limit", async () => {
      const outcome = await run(`const a = []; while (true) a.push("x".repeat(1024));`, { memoryMb: 32, timeoutMs: 8000 });
      assert.ok(outcome.error, "a script filling memory fails");
    });

    test("keeps nothing from one script to the next", async () => {
      await run(`globalThis.leak = 42; return 1;`);
      const outcome = await run(`return typeof globalThis.leak;`);
      assert.equal(outcome.value, "undefined");
    });

    test("keeps what a script logs within a budget, and refuses huge messages", async () => {
      const flood = await run(`const big = "x".repeat(5000); for (let i = 0; i < 2000; i++) console.log(big); return 1;`);
      assert.equal(flood.value, 1);
      assert.ok(flood.logs.join("").length <= 70 * 1024);
      assert.match(flood.logs.at(-1)!, /more logs dropped/);
      const huge = await run(`return "x".repeat(3 * 1024 * 1024);`, { memoryMb: 128 });
      assert.match(String(huge.error), /longer than/);
    });

    test("counts batches of a list before fetching them, one at a time", async () => {
      const outcome = await run(
        `const pages = []; for (let i = 0; i < 20; i++) pages.push((async () => { let n = 0; for await (const t of client.things.list()) n++; return n; })()); return (await Promise.allSettled(pages)).length;`,
        { calls: 5 },
      );
      assert.ok(outcome.calls <= 5, `${outcome.calls} calls`);
    });

    test("keeps the host's event loop free while a script computes", async () => {
      if (name === "quickjs") return; // in process by design: a Worker's own isolate, on Cloudflare
      const started = Date.now();
      const spin = run(`while (true) {}`, { timeoutMs: 1500 });
      const tick = await new Promise<number>((resolve) => setTimeout(() => resolve(Date.now() - started), 50));
      assert.ok(tick < 500, `a 50 ms timer fired after ${tick} ms`);
      assert.match(String((await spin).error), /longer than 1500 ms/);
    });

    test("reports a script that does not compile", async () => {
      const outcome = await run(`return (;`);
      assert.match(String(outcome.error), /does not compile/);
    });
  });
}
