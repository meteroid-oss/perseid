// The HTTP handler as MCP clients see it: the official client on the 2026-07-28 protocol, and a
// 2025 client's stateless requests.
import assert from "node:assert/strict";
import { test } from "node:test";

import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";

import { createHandler, type Api } from "../index.js";
import { quickjsSandbox } from "../sandbox/quickjs.js";

const api: Api = {
  name: "Acme",
  server: "acme-mcp",
  version: "1.0.0",
  envPrefix: "ACME",
  instructions: "Amounts are in cents.",
  corpus: {
    package: "acme",
    operations: [
      {
        accessor: "whoami.retrieve",
        signature: "whoami.retrieve(): Promise<string>",
        method: "GET",
        path: "/whoami",
        summary: "Who the key belongs to",
        description: "",
        deprecated: false,
        paginated: false,
        types: [],
      },
    ],
    models: {},
  },
  client: (options) => ({ whoami: { retrieve: async () => options.apiKey } }),
};

const handler = createHandler(api, { sandbox: quickjsSandbox() });
const fetchHandler = ((input: string | URL | Request, init?: RequestInit) => handler(new Request(input, init))) as typeof fetch;

test("the official client calls the tools on the current protocol, with its key", async () => {
  const client = new Client({ name: "test", version: "1.0.0" });
  await client.connect(
    new StreamableHTTPClientTransport(new URL("https://mcp.acme.test/mcp"), {
      fetch: fetchHandler,
      requestInit: { headers: { authorization: "Bearer sk_modern" } },
    }),
  );
  const { tools } = await client.listTools();
  assert.deepEqual(tools.map((t) => t.name), ["search", "execute"]);
  const result = await client.callTool({ name: "execute", arguments: { code: "return await client.whoami.retrieve();" } });
  const text = (result.content as { text: string }[])[0]!.text;
  assert.equal(JSON.parse(text).result, "sk_modern");
  await client.close();
});

test("a 2025 client's stateless request is served too", async () => {
  const response = await handler(
    new Request("https://mcp.acme.test/mcp", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        "x-api-key": "sk_legacy",
        "mcp-protocol-version": "2025-06-18",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name: "execute", arguments: { code: "return await client.whoami.retrieve();" } },
      }),
    }),
  );
  assert.equal(response.status, 200);
  const body = (await response.json()) as { result: { content: { text: string }[] } };
  assert.equal(JSON.parse(body.result.content[0]!.text).result, "sk_legacy");
});

test("answers health checks, and refuses requests without a key", async () => {
  assert.equal((await handler(new Request("https://mcp.acme.test/health"))).status, 200);
  const anonymous = await handler(new Request("https://mcp.acme.test/mcp", { method: "POST", body: "{}" }));
  assert.equal(anonymous.status, 401);
});
