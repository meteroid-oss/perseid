// The command line of the server on Node: stdio for a local client, Streamable HTTP for a hosted
// one, scripts in Deno.
import { readFileSync } from "node:fs";
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createRequire } from "node:module";
import { availableParallelism } from "node:os";
import { dirname, join } from "node:path";
import { lookup } from "node:dns/promises";
import { isIP } from "node:net";
import { PassThrough, Readable, Transform, Writable } from "node:stream";
import { parseArgs } from "node:util";

export { denoSandbox } from "./sandbox/deno.js";
export { quickjsThreadsSandbox } from "./sandbox/threads.js";
export { sqliteStore } from "./oauth/sqlite.js";

import { serveStdio, StdioServerTransport } from "@modelcontextprotocol/server/stdio";

import { apiKeyAuth, createHandler, type Auth } from "./http.js";
import { oauthBroker, privateAddress, privateHost } from "./oauth/broker.js";
import { resourceServer } from "./oauth/resource.js";
import { sqliteStore } from "./oauth/sqlite.js";
import { memoryStore } from "./oauth/store.js";
import { DEFAULT_LIMITS, limitConcurrency, type Sandbox } from "./sandbox/index.js";
import { denoSandbox } from "./sandbox/deno.js";
import type { Sources } from "./search.js";
import { createServer, DEFAULT_MAX_CHARS, type Api, type Options } from "./server.js";

const USAGE = (api: Api) => `Usage: ${api.server} [--port N] [options]

Serves the ${api.name} API to MCP clients in code mode: a search tool over its TypeScript SDK,
and an execute tool running scripts against it in a sandbox.

Without --port (or PORT), on stdin and stdout, with ${api.envPrefix}_API_KEY. With it, over
Streamable HTTP on /mcp, each request with its own API key: Authorization: Bearer <key>, or
X-API-Key; --key-from-env lets requests without one use ${api.envPrefix}_API_KEY.

With --oauth, the server is its own OAuth authorization server, for one-click connectors: users
paste their API key once on its consent page, kept encrypted under MCP_OAUTH_SECRET (32
characters or more) in --store. With --authorization-server, the API's own OAuth server issues
the tokens, which the server checks with MCP_INTROSPECTION_URL, MCP_CLIENT_ID and
MCP_CLIENT_SECRET when set, and calls the API with.

Options:
  --port N            serve HTTP on this port (default: PORT, else stdio)
  --host H            the address to listen on (default 0.0.0.0)
  --base-url URL      the API's base URL (default: ${api.envPrefix}_BASE_URL, else the SDK's)
  --read-only         run only GET operations
  --key-from-env      HTTP: use ${api.envPrefix}_API_KEY for requests without a key
  --sandbox NAME      where scripts run: deno (default), quickjs, or remote: a perseid-mcp
                      runner at MCP_RUNNER_URL, with MCP_RUNNER_TOKEN
  --oauth             HTTP: be the OAuth server, a consent page collecting API keys
  --store FILE        --oauth: a SQLite file keeping grants (default: memory, lost on restart)
  --public-url URL    the server's public URL, the OAuth issuer and resource: required with
                      --oauth and --authorization-server
  --keys-url URL      --oauth: where users find their API key, linked from the consent page
  --authorization-server URL
                      HTTP: the API's OAuth server issuing tokens (repeatable)
  --timeout MS        a script's time limit (default ${DEFAULT_LIMITS.timeoutMs})
  --memory MB         a script's memory limit (default ${DEFAULT_LIMITS.memoryMb})
  --max-calls N       API calls a script may make (default ${DEFAULT_LIMITS.calls})
  --max-chars N       results longer than this fail (default ${DEFAULT_MAX_CHARS})
`;

export interface NodeOptions {
  /** The SDK's sources `search` quotes: by default, its package as resolved from here. */
  sources?: Sources;
  /** Where scripts run: Deno by default. */
  sandbox?: Sandbox;
  /** How HTTP requests authenticate, given the parsed options: by default an API key each, or
   * what --oauth and --authorization-server ask for. */
  auth?: (api: Api, fallback: string | undefined) => Auth;
  /** --oauth: checks a key on the consent page, by calling the API with it. */
  validate?: (apiKey: string) => Promise<string | undefined>;
}

function required(name: string, by: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${by} needs ${name}`);
  return value;
}

export async function main(api: Api, argv: string[], node: NodeOptions = {}): Promise<void> {
  const { values } = parseArgs({
    args: argv,
    options: {
      port: { type: "string" },
      host: { type: "string", default: "0.0.0.0" },
      "base-url": { type: "string" },
      "read-only": { type: "boolean", default: false },
      "key-from-env": { type: "boolean", default: false },
      sandbox: { type: "string" },
      oauth: { type: "boolean", default: false },
      store: { type: "string" },
      "public-url": { type: "string" },
      "keys-url": { type: "string" },
      "authorization-server": { type: "string", multiple: true },
      timeout: { type: "string" },
      memory: { type: "string" },
      "max-calls": { type: "string" },
      "max-chars": { type: "string" },
      help: { type: "boolean", short: "h", default: false },
      version: { type: "boolean", short: "V", default: false },
    },
  });
  if (values.help) return void process.stdout.write(USAGE(api));
  if (values.version) return void process.stdout.write(`${api.server} ${api.version}\n`);
  const number = (name: string, text: string | undefined, fallback: number) => {
    if (text === undefined) return fallback;
    const value = Number(text);
    if (!Number.isInteger(value) || value <= 0) throw new Error(`--${name}: expected a positive integer, got ${text}`);
    return value;
  };
  const env = (name: string) => process.env[`${api.envPrefix}_${name}`] || undefined;
  const options: Options = {
    sandbox: node.sandbox ?? (await sandbox(values.sandbox ?? process.env.MCP_SANDBOX)),
    sources: node.sources ?? packageSources(api.corpus.package),
    baseURL: values["base-url"] ?? env("BASE_URL"),
    readOnly: values["read-only"],
    limits: {
      timeoutMs: number("timeout", values.timeout, DEFAULT_LIMITS.timeoutMs),
      memoryMb: number("memory", values.memory, DEFAULT_LIMITS.memoryMb),
      calls: number("max-calls", values["max-calls"], DEFAULT_LIMITS.calls),
    },
    maxChars: number("max-chars", values["max-chars"], DEFAULT_MAX_CHARS),
  };
  const port = values.port ?? (process.env.PORT || undefined);
  if (port === undefined) {
    serveStdio(() => createServer(api, { ...options, apiKey: env("API_KEY") }), { transport: drainingStdio() });
    return;
  }
  const fallback = values["key-from-env"] ? env("API_KEY") : undefined;
  const publicUrl = values["public-url"];
  if ((values.oauth || values["authorization-server"]?.length) && !publicUrl) {
    throw new Error("--oauth and --authorization-server need --public-url: the issuer and resource never come from request headers");
  }
  const auth = node.auth
    ? node.auth(api, fallback)
    : values.oauth
      ? oauthBroker(api, {
          store: values.store ? await sqliteStore(values.store) : memoryStore(),
          secret: required("MCP_OAUTH_SECRET", "--oauth"),
          url: publicUrl,
          keysUrl: values["keys-url"],
          validate: node.validate,
          fetch: guardedFetch,
        })
      : values["authorization-server"]?.length
        ? resourceServer({
            authorizationServers: values["authorization-server"],
            url: publicUrl,
            introspection: process.env.MCP_INTROSPECTION_URL
              ? {
                  url: process.env.MCP_INTROSPECTION_URL,
                  clientId: required("MCP_CLIENT_ID", "MCP_INTROSPECTION_URL"),
                  clientSecret: required("MCP_CLIENT_SECRET", "MCP_INTROSPECTION_URL"),
                }
              : undefined,
          })
        : apiKeyAuth(api, fallback);
  if (values.oauth && !values.store) {
    process.stderr.write("--oauth without --store: grants are kept in memory, and lost on restart\n");
  }
  const handler = createHandler(api, { ...options, auth });
  const http = createHttpServer(listener(handler));
  http.listen(number("port", port, 0), values.host, () => {
    process.stderr.write(`${api.server} ${api.version}: MCP on http://${values.host}:${port}/mcp\n`);
  });
}

/** The sandbox named on the command line, running as many scripts at once as there are CPUs
 * (Deno: twice that, its processes mostly waiting on calls). */
async function sandbox(name: string | undefined): Promise<Sandbox> {
  switch (name ?? "deno") {
    case "deno":
      return limitConcurrency(denoSandbox(), availableParallelism() * 2);
    case "quickjs":
      return (await import("./sandbox/threads.js")).quickjsThreadsSandbox();
    case "remote": {
      const url = process.env.MCP_RUNNER_URL;
      const token = process.env.MCP_RUNNER_TOKEN;
      if (!url || !token) throw new Error("--sandbox remote needs MCP_RUNNER_URL and MCP_RUNNER_TOKEN");
      return limitConcurrency((await import("./sandbox/remote.js")).remoteSandbox({ url, token }), availableParallelism() * 4);
    }
    default:
      throw new Error(`--sandbox: expected deno, quickjs or remote, got ${name}`);
  }
}

/** Stdio whose input ends only once every request read has its answer written: a client that
 * pipes its requests then closes stdin still gets every answer. */
function drainingStdio(): StdioServerTransport {
  const pending = new Set<string>();
  let ended = false;
  const input = new PassThrough();
  const settle = () => {
    if (ended && pending.size === 0) input.end();
  };
  const ids = (chunk: Buffer | string, each: (id: string) => void) => {
    for (const line of String(chunk).split("\n")) {
      try {
        const message = JSON.parse(line);
        if (message && message.id !== undefined && message.id !== null) each(JSON.stringify(message.id));
      } catch {}
    }
  };
  process.stdin.on("data", (chunk) => {
    ids(chunk, (id) => pending.add(id));
    input.write(chunk);
  });
  process.stdin.on("end", () => {
    ended = true;
    settle();
  });
  const output = new Writable({
    write(chunk, encoding, callback) {
      process.stdout.write(chunk, encoding, callback);
      ids(chunk, (id) => pending.delete(id));
      settle();
    },
  });
  return new StdioServerTransport(input, output);
}

/** A Node request listener serving a Web standard handler. */
export function listener(handler: (request: Request) => Promise<Response>) {
  return (incoming: IncomingMessage, outgoing: ServerResponse) => {
    serve(handler, incoming, outgoing).catch((error) => {
      if (!outgoing.headersSent) {
        outgoing.writeHead(500, { "content-type": "application/json" });
        outgoing.end(JSON.stringify({ error: String(error?.message ?? error) }));
      } else {
        outgoing.destroy();
      }
    });
  };
}

async function serve(
  handler: (request: Request) => Promise<Response>,
  incoming: IncomingMessage,
  outgoing: ServerResponse,
): Promise<void> {
  // The origin only names the request: the OAuth issuer and resource come from --public-url.
  const url = new URL(incoming.url ?? "/", "http://localhost");
  const headers = new Headers();
  for (const [name, value] of Object.entries(incoming.headers)) {
    if (Array.isArray(value)) for (const v of value) headers.append(name, v);
    else if (value !== undefined) headers.set(name, value);
  }
  const declared = Number(incoming.headers["content-length"] ?? 0);
  if (declared > MAX_BODY) {
    outgoing.writeHead(413, { "content-type": "application/json" }).end(JSON.stringify({ error: "the request body is too large" }));
    incoming.resume();
    return;
  }
  const body =
    incoming.method === "GET" || incoming.method === "HEAD" ? undefined : (Readable.toWeb(capped(incoming, MAX_BODY)) as ReadableStream);
  const controller = new AbortController();
  outgoing.on("close", () => controller.abort());
  const request = new Request(url, { method: incoming.method, headers, body, signal: controller.signal, duplex: "half" } as RequestInit);
  const response = await handler(request);
  outgoing.writeHead(response.status, Object.fromEntries(response.headers));
  if (!response.body) return void outgoing.end();
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) outgoing.write(chunk);
  outgoing.end();
}

/** The largest request body the server reads: the MCP SDK's own bound. */
const MAX_BODY = 4 * 1024 * 1024;

/** `incoming`, failing past `limit` bytes. */
function capped(incoming: IncomingMessage, limit: number): Readable {
  let size = 0;
  return incoming.pipe(
    new Transform({
      transform(chunk: Buffer, _encoding, callback) {
        size += chunk.length;
        callback(size > limit ? new Error("the request body is too large") : null, chunk);
      },
    }),
  );
}

/** `fetch` refusing hosts that resolve to private addresses: what the OAuth broker fetches
 * Client ID Metadata Documents with. The address is checked before the request, not at its
 * connection, so a name that resolves differently the second time (DNS rebinding) gets through:
 * the broker's 5-second timeout and 16 KB cap bound what that yields. */
export const guardedFetch: typeof fetch = async (input, init) => {
  const { hostname } = new URL(input instanceof Request ? input.url : String(input));
  const name = hostname.replace(/^\[|\]$/g, "");
  if (privateHost(name)) throw new Error(`${hostname} is not a public host`);
  const addresses = isIP(name) ? [{ address: name }] : await lookup(name, { all: true });
  if (!addresses.length || addresses.some(({ address }) => privateAddress(address))) {
    throw new Error(`${hostname} resolves to a private address`);
  }
  return fetch(input, init);
};

/** The sources of the npm package `pkg`, resolved from `from` (the server's `import.meta.url`),
 * else from here. */
export function packageSources(pkg: string, from?: string): Sources {
  let root: string | undefined;
  for (const base of [from, import.meta.url]) {
    if (!base || root) continue;
    try {
      root = dirname(createRequire(base).resolve(`${pkg}/package.json`));
    } catch {}
  }
  return (file) => {
    if (!root) return undefined;
    try {
      return readFileSync(join(root, file), "utf8");
    } catch {
      return undefined;
    }
  };
}
