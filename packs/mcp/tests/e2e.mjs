// The server generated from tests/acme, run against the CLI pack's mock API:
// `SERVER=path/to/dist/main.js node --test tests/e2e.mjs`, with `SANDBOX=quickjs` or `remote` (a
// runner process, scripts in Deno) for those backends.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const SERVER = process.env.SERVER;
const SANDBOX = process.env.SANDBOX || "deno";
const MOCK = join(dirname(fileURLToPath(import.meta.url)), "../../cli/tests/mock.py");
let api;

const freePort = () =>
  new Promise((resolve) => {
    const probe = createServer().listen(0, () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });

async function waitFor(url) {
  for (let i = 0; i < 100; i++) {
    try {
      await fetch(url);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw new Error(`${url} never answered`);
}

/** With SANDBOX=remote, the runner every server sends its scripts to. */
const RUNNER = { MCP_RUNNER_TOKEN: "e2e-runner-token-of-at-least-32-chars" };
let runner;

before(async () => {
  if (SANDBOX === "remote") {
    const port = await freePort();
    const cli = join(dirname(SERVER), "../node_modules/perseid-mcp/dist/cli.js");
    runner = spawn("node", [cli, "runner", "--port", String(port), "--host", "127.0.0.1"], {
      env: { ...process.env, ...RUNNER },
      stdio: ["ignore", "ignore", "inherit"],
    });
    RUNNER.MCP_RUNNER_URL = `http://127.0.0.1:${port}`;
    await waitFor(`${RUNNER.MCP_RUNNER_URL}/health`);
  }
  const port = await freePort();
  const mock = spawn("python3", [MOCK, String(port)], { stdio: "ignore" });
  api = { url: `http://127.0.0.1:${port}`, process: mock };
  await waitFor(`${api.url}/__requests`);
});

after(() => {
  api.process.kill();
  runner?.kill();
});

/** The server on stdio, and a function sending it a request and reading its answer. */
function stdio(args = [], env = {}) {
  const server = spawn("node", [SERVER, "--sandbox", SANDBOX, ...args], {
    env: { ...process.env, ...RUNNER, ACME_API_KEY: "sk_test", ACME_BASE_URL: api.url, PORT: "", ...env },
    stdio: ["pipe", "pipe", "inherit"],
  });
  const waiting = new Map();
  createInterface({ input: server.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    waiting.get(message.id)?.resolve(message);
  });
  server.on("exit", (code) => {
    for (const { reject } of waiting.values()) reject(new Error(`the server exited with ${code}`));
  });
  let id = 0;
  const request = (method, params = {}) => {
    const message = { jsonrpc: "2.0", id: ++id, method, params };
    server.stdin.write(JSON.stringify(message) + "\n");
    return new Promise((resolve, reject) => waiting.set(message.id, { resolve, reject }));
  };
  const close = () => server.kill();
  return { request, close };
}

async function connected(args, env) {
  const server = stdio(args, env);
  const init = await server.request("initialize", {
    protocolVersion: "2025-11-25",
    capabilities: {},
    clientInfo: { name: "e2e", version: "1" },
  });
  return { ...server, init };
}

const call = async (server, name, args) => {
  const { result } = await server.request("tools/call", { name, arguments: args });
  return { ...result, json: JSON.parse(result.content[0].text) };
};

const text = async (server, query) =>
  (await server.request("tools/call", { name: "search", arguments: { query } })).result.content[0].text;

test("search finds methods with the declarations of their types", { timeout: 60_000 }, async () => {
  const server = await connected();
  try {
    assert.match(server.init.result.instructions, /^Quantities are counts of items in stock\./);
    const { result } = await server.request("tools/list");
    assert.deepEqual(result.tools.map((t) => t.name), ["search", "execute"]);
    const found = await text(server, "list things");
    assert.match(found, /^## client\.things\.list\(options\?: ThingsListOptions\): PagePromise<ThingList, Thing>/);
    assert.match(found, /export interface ThingsListOptions \{[\s\S]*limit\?: number/);
    assert.match(found, /export interface Thing \{/);
    assert.match(await text(server, "client.things.create"), /^## client\.things\.create\(/);
    assert.match(await text(server, "Address"), /export interface Address/);
    assert.match(await text(server, "zzz"), /^Nothing matches "zzz"\. The resources are: client\./);
  } finally {
    server.close();
  }
});

test("execute runs a script against the API, pages and errors included", { timeout: 60_000 }, async () => {
  const server = await connected();
  try {
    const listed = await call(server, "execute", {
      code: `const page = await client.things.list({ limit: 2 });
const names = [];
for await (const thing of client.things.list({ limit: 2 })) names.push(thing.name);
console.log("listed", names.length);
return { first: page.items.map((t) => t.id), more: page.hasNextPage, names };`,
    });
    assert.equal(listed.isError, undefined);
    assert.deepEqual(listed.json.result.first, ["th_1", "th_2"]);
    assert.equal(listed.json.result.more, true);
    assert.equal(listed.json.result.names.length, 5);
    assert.deepEqual(listed.json.logs, ["listed 5"]);

    const missing = await call(server, "execute", { code: `return await client.things.retrieve("th_404");` });
    assert.equal(missing.isError, true);
    assert.match(missing.json.error, /404/);
    const caught = await call(server, "execute", {
      code: `try { await client.things.retrieve("th_404"); } catch (e) { return { status: e.status, body: e.body }; }`,
    });
    assert.deepEqual(caught.json.result, { status: 404, body: { message: "no thing th_404" } });
    const unknown = await call(server, "execute", { code: `return await client.things.nope();` });
    assert.match(unknown.json.error, /client\.things\.nope is not an operation of the SDK/);
    const internal = await call(server, "execute", { code: `return await client.things._client.get("/things");` });
    assert.match(internal.json.error, /is not an operation of the SDK|is not a method/);
  } finally {
    server.close();
  }
});

test("the sandbox has no network, files nor environment, and stops at its limits", { timeout: 60_000 }, async () => {
  const server = await connected(["--timeout", "2000", "--max-calls", "3"]);
  try {
    const net = await call(server, "execute", { code: `return await fetch(${JSON.stringify(api.url)});` });
    assert.match(net.json.error, /net access|'?fetch'? is not defined/);
    const env = await call(server, "execute", { code: `return Deno.env.get("ACME_API_KEY");` });
    assert.match(env.json.error, /env access|'?Deno'? is not defined/);
    const files = await call(server, "execute", { code: `return Deno.readTextFileSync("/etc/hostname");` });
    assert.match(files.json.error, /read access|'?Deno'? is not defined/);
    const spin = await call(server, "execute", { code: `while (true) {}` });
    assert.match(spin.json.error, /longer than 2000 ms/);
    const calls = await call(server, "execute", {
      code: `for (let i = 0; i < 5; i++) await client.things.retrieve("th_1"); return "done";`,
    });
    assert.match(calls.json.error, /more than 3 API calls/);
  } finally {
    server.close();
  }
});

test("a read-only server refuses writes before they reach the API", { timeout: 60_000 }, async () => {
  const before = (await (await fetch(`${api.url}/__requests`)).json()).length;
  const server = await connected(["--read-only"]);
  try {
    const listed = await server.request("tools/list");
    const execute = listed.result.tools.find((t) => t.name === "execute");
    assert.equal(execute.annotations.readOnlyHint, true);
    const created = await call(server, "execute", { code: `return await client.things.create({ name: "Lamp" });` });
    assert.match(created.json.error, /client\.things\.create is a POST: this server is read-only/);
    const around = await call(server, "execute", { code: `return await client.things._client.post("/things", { body: { name: "Lamp" } });` });
    assert.equal(around.isError, true);
    const read = await call(server, "execute", { code: `return (await client.things.retrieve("th_1")).name;` });
    assert.equal(read.json.result, "Widget");
    const sent = await (await fetch(`${api.url}/__requests`)).json();
    assert.deepEqual(sent.slice(before).map((r) => r.method), ["GET"]);
  } finally {
    server.close();
  }
});

test("over HTTP each request brings its API key", { timeout: 60_000 }, async () => {
  const port = await freePort();
  const server = spawn("node", [SERVER, "--sandbox", SANDBOX, "--port", String(port), "--host", "127.0.0.1"], {
    env: { ...process.env, ...RUNNER, ACME_API_KEY: "", ACME_BASE_URL: api.url },
    stdio: ["ignore", "ignore", "inherit"],
  });
  try {
    const url = `http://127.0.0.1:${port}`;
    await waitFor(`${url}/health`);
    assert.equal((await (await fetch(`${url}/health`)).json()).status, "ok");
    const post = (body, headers = {}) =>
      fetch(`${url}/mcp`, {
        method: "POST",
        headers: { "content-type": "application/json", accept: "application/json, text/event-stream", ...headers },
        body: JSON.stringify(body),
      });
    const anonymous = await post({ jsonrpc: "2.0", id: 1, method: "tools/list" });
    assert.equal(anonymous.status, 401);
    assert.match(anonymous.headers.get("www-authenticate"), /^Bearer/);
    const executed = await post(
      {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "execute", arguments: { code: `return (await client.auth.retrieveWhoami()).id;` } },
      },
      { authorization: "Bearer sk_test" },
    );
    assert.equal(executed.status, 200);
    const body = await executed.json();
    assert.equal(JSON.parse(body.result.content[0].text).result.length > 0, true);
    const sent = await (await fetch(`${api.url}/__requests`)).json();
    assert.equal(sent.at(-1).headers.authorization ?? sent.at(-1).headers.Authorization, "Bearer sk_test");
  } finally {
    server.kill();
  }
});

test("with --oauth, a client connects through the consent page and calls with the user's key", { timeout: 60_000 }, async () => {
  const port = await freePort();
  const store = join(await mkdtemp(join(tmpdir(), "acme-mcp-")), "grants.db");
  const server = spawn(
    "node",
    [SERVER, "--sandbox", SANDBOX, "--port", String(port), "--host", "127.0.0.1", "--oauth", "--store", store, "--public-url", `http://127.0.0.1:${port}`],
    {
      env: { ...process.env, ...RUNNER, ACME_API_KEY: "", ACME_BASE_URL: api.url, MCP_OAUTH_SECRET: "e2e-secret-of-at-least-thirty-two-chars" },
      stdio: ["ignore", "ignore", "inherit"],
    },
  );
  try {
    const url = `http://127.0.0.1:${port}`;
    await waitFor(`${url}/health`);
    const unauthorized = await fetch(`${url}/mcp`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    assert.equal(unauthorized.status, 401);
    const metadataUrl = /resource_metadata="([^"]+)"/.exec(unauthorized.headers.get("www-authenticate"))[1];
    const [server_] = (await (await fetch(metadataUrl)).json()).authorization_servers;
    const metadata = await (await fetch(`${server_}/.well-known/oauth-authorization-server`)).json();
    const redirect = "http://127.0.0.1:9999/callback";
    const { client_id } = await (
      await fetch(metadata.registration_endpoint, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ redirect_uris: [redirect], client_name: "e2e" }),
      })
    ).json();
    const verifier = randomBytes(32).toString("base64url");
    const challenge = createHash("sha256").update(verifier).digest("base64url");
    const authorize = new URL(metadata.authorization_endpoint);
    for (const [k, v] of Object.entries({ response_type: "code", client_id, redirect_uri: redirect, code_challenge: challenge, code_challenge_method: "S256", state: "s" })) {
      authorize.searchParams.set(k, v);
    }
    const page = await (await fetch(authorize)).text();
    const pending = /name="pending" value="([^"]+)"/.exec(page)[1];
    const consented = await fetch(authorize, {
      method: "POST",
      redirect: "manual",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ pending, api_key: "sk_user", action: "allow" }),
    });
    assert.equal(consented.status, 302);
    const code = new URL(consented.headers.get("location")).searchParams.get("code");
    const tokens = await (
      await fetch(metadata.token_endpoint, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({ grant_type: "authorization_code", code, redirect_uri: redirect, client_id, code_verifier: verifier }),
      })
    ).json();
    const executed = await fetch(`${url}/mcp`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        authorization: `Bearer ${tokens.access_token}`,
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name: "execute", arguments: { code: `return (await client.auth.retrieveWhoami()).id;` } },
      }),
    });
    assert.equal(executed.status, 200);
    const sent = await (await fetch(`${api.url}/__requests`)).json();
    assert.equal(sent.at(-1).headers.authorization ?? sent.at(-1).headers.Authorization, "Bearer sk_user");
  } finally {
    server.kill();
  }
});
