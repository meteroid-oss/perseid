// The MCP server of an API in code mode: `search` the SDK, `execute` a script against it.
import { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";

import { DEFAULT_LIMITS, execute, type Limits, type Sandbox } from "./sandbox/index.js";
import { search, type Corpus, type Sources } from "./search.js";

/** What the pack generates for an API: see `src/generated/api.ts`. */
export interface Api {
  /** The client's name, `Acme`. */
  name: string;
  /** This server's npm package, and its version: the SDK's. */
  server: string;
  version: string;
  /** `ACME`: `ACME_API_KEY`, `ACME_BASE_URL`. */
  envPrefix: string;
  /** From `[context] mcp_instructions` in perseid.toml, first in the server's own. */
  instructions: string;
  corpus: Corpus;
  /** The SDK's client; `fetch` carries every request of a script's calls. */
  client(options: { apiKey?: string; baseURL?: string; fetch?: typeof fetch }): object;
}

export interface Options {
  /** Where scripts run: `denoSandbox()` of `perseid-mcp/deno` on Node. */
  sandbox: Sandbox;
  /** The SDK's sources `search` quotes declarations from. */
  sources?: Sources;
  apiKey?: string;
  baseURL?: string;
  /** Refuse the operations that are not GET or HEAD. */
  readOnly?: boolean;
  limits?: Limits;
  /** Results longer than this many characters fail, asking for less. */
  maxChars?: number;
}

export const DEFAULT_MAX_CHARS = 60_000;

export function createServer(api: Api, options: Options): McpServer {
  const { readOnly = false, limits = DEFAULT_LIMITS, maxChars = DEFAULT_MAX_CHARS } = options;
  const server = new McpServer(
    { name: api.server, version: api.version },
    { instructions: instructions(api, readOnly) },
  );
  server.registerTool(
    "search",
    {
      title: `Search the ${api.name} SDK`,
      description:
        `The methods of the ${api.name} client that match a query, with the TypeScript ` +
        `declarations of what they take and return: \`list customers\`, \`invoice pdf\`, ` +
        `\`client.subscriptions.create\`, or a model's name.`,
      inputSchema: z.object({
        query: z.string().describe("Words of the operation or model sought"),
        limit: z.number().int().min(1).max(20).optional().describe("Methods to return, 5 by default"),
      }),
      annotations: { readOnlyHint: true, openWorldHint: false },
    },
    async ({ query, limit }) => ({
      content: [{ type: "text", text: search(api.corpus, query, limit, options.sources) }],
    }),
  );
  const methods = new Map(api.corpus.operations.map((o) => [o.accessor, o.method]));
  const guard = (accessor: string) => {
    const method = methods.get(accessor);
    if (method === undefined) return `client.${accessor} is not an operation of the SDK: search for it first`;
    if (!readOnly || method === "GET" || method === "HEAD") return undefined;
    return `client.${accessor} is a ${method}: this server is read-only`;
  };
  server.registerTool(
    "execute",
    {
      title: `Run a script against the ${api.name} API`,
      description:
        `Runs TypeScript against the ${api.name} API. \`code\` is the body of an async ` +
        `function given \`client\`, a ${api.name} SDK client; what it returns comes back as JSON, ` +
        `with what it logs. Search for the methods first. Do the work in the script (loops, ` +
        `filters, joins) and return only what is needed: ` +
        `\`const page = await client.customers.list(); return page.items.map((c) => c.name);\` ` +
        `The script has no network, files or environment: only \`client\` reaches the API.`,
      inputSchema: z.object({ code: z.string().describe("The body of `async (client) => { ... }`") }),
      annotations: { readOnlyHint: readOnly, destructiveHint: !readOnly, openWorldHint: true },
    },
    async ({ code }) => {
      const client = (fetch: typeof globalThis.fetch) => api.client({ apiKey: options.apiKey, baseURL: options.baseURL, fetch });
      const outcome = await execute(code, client, options.sandbox, limits, { guard, readOnly });
      const logs = outcome.logs.length ? outcome.logs : undefined;
      if (outcome.error !== undefined) {
        const text = JSON.stringify({ error: outcome.error, logs });
        return { content: [{ type: "text", text }], isError: true };
      }
      const text = JSON.stringify({ result: outcome.value, logs });
      if (text.length > maxChars) {
        const error = `the result is ${text.length} characters, more than the ${maxChars} this server returns: return only the fields needed, or fewer items`;
        return { content: [{ type: "text", text: JSON.stringify({ error }) }], isError: true };
      }
      return { content: [{ type: "text", text }] };
    },
  );
  return server;
}

function instructions(api: Api, readOnly: boolean): string {
  const own = api.instructions ? `${api.instructions.trim()}\n\n` : "";
  return (
    own +
    `The ${api.name} API, in code mode: \`search\` finds the methods of its TypeScript SDK and ` +
    `the types they take; \`execute\` runs a script calling them on \`client\`. Methods are ` +
    `\`client.<resource>.<method>(...)\`, path parameters first, then the body or an options ` +
    `object. A list awaited gives its first page (\`items\`, \`hasNextPage\`); \`for await\` ` +
    `walks every item. A failed call throws an Error with the API's \`status\` and \`body\`.` +
    (readOnly ? " This server is read-only: only GET operations run." : "")
  );
}
