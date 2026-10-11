# perseid-mcp

The runtime of MCP servers in code mode over the TypeScript SDKs perseid generates. The `mcp`
pack generates a server per API, `src/generated/api.ts` and a `main.ts`; this package serves it.

Two tools, as Stainless and Cloudflare serve large APIs:

- `search` finds the SDK's methods matching a query, with the TypeScript declarations of what
  they take and return.
- `execute` runs a script, the body of an async function given `client`, in a sandbox, and
  returns what it returns as JSON, with what it logs.

## Sandboxes

A script has no capability of its own: no network, files, environment or timers. Its `client` is
a proxy: each call goes to the host, which checks it is an operation of the SDK (never a private
member such as `_client`), counts it, and makes it with the SDK and the request's credentials.
The API key never enters the sandbox. Every HTTP request of the script's client goes through a
`fetch` of the host's: counted against the limit (pages and retries included), refused if it is
not a GET on a read-only server, stripped of method-override headers, and aborted when the
script ends. Scripts are TypeScript: the host strips the types (sucrase) before any backend
runs them.

| Backend | Where | Isolation | Per script |
|---|---|---|---|
| `denoSandbox` (`perseid-mcp/deno`) | Node | a process with no permission; its memory watched on Linux | ~45 ms |
| `quickjsThreadsSandbox` (`perseid-mcp/node`) | Node | QuickJS in WebAssembly, a fresh runtime, in worker threads: a loop never blocks the server | ~2–5 ms |
| `quickjsSandbox` (`perseid-mcp/quickjs`) | Workers, anywhere | the same, in process; on Workers, a loop is stopped by a count of interpreter checks, the clock standing still while code runs | ~2–5 ms (~15 ms on workerd) |
| `dynamicWorkersSandbox` (`perseid-mcp/cloudflare`) | Workers Paid | a Dynamic Worker per script, full V8, no network, CPU bounded by Cloudflare; billed per script | — |
| `remoteSandbox` (`perseid-mcp/node`) | Node | another container, `perseid-mcp runner`, holding no key | one HTTP exchange |

All enforce the same limits: time (30 s), memory (256 MB), API calls (100), logs (64 KB),
messages (2 MB), and run a bounded number of scripts at once (`limitConcurrency`). They pass the
same tests (`src/test/sandbox.test.ts`). `--sandbox deno|quickjs|remote` picks one on the
command line; Deno is the default.

A backend implements one method, `run(script, host, limits)`: it runs the script after
`PRELUDE` and carries its messages to `host.handle`.

## Entry points

- `perseid-mcp`: `createServer(api, options)`, the MCP server; `createHandler(api, options)`, its
  Streamable HTTP handler on Web standard `Request` and `Response` (Node, Cloudflare Workers);
  `execute`, `search`, and the sandbox interface. No Node API.
- `perseid-mcp/node`: `main(api, argv)`, the command line: stdio with `<PREFIX>_API_KEY`, or
  HTTP with `--port`, each request bringing its key (`Authorization: Bearer`, `X-API-Key`), or
  `--oauth`.
- `perseid-mcp/cloudflare`: `mcpWorker(api, { sources })`, the server as a Worker, configured by
  its bindings (`MCP_AUTH`, `MCP_SANDBOX`, `OAUTH_DB`, `LOADER`).
- `perseid-mcp sources`: the SDK sources `search` quotes, as JSON, for a Worker to bundle.
  `perseid-mcp runner`: the sandbox of `--sandbox remote`, in a container of its own.

Both the 2026-07-28 protocol (stateless, `Mcp-Method` headers, `server/discover`) and 2025
clients are served, each request by a server of its own, through the MCP SDK v2.

```ts
import { main, packageSources } from "perseid-mcp/node";
import { api } from "./generated/api.js";

await main(api, process.argv.slice(2), { sources: packageSources(api.corpus.package, import.meta.url) });
```

## Credentials over HTTP

Three ways a request proves who it acts for, each an `Auth` the HTTP handler takes:

- **An API key per request** (`apiKeyAuth`, the default): `Authorization: Bearer <key>` or
  `X-API-Key`. For agents and clients that set headers.
- **The OAuth broker** (`oauthBroker`, `--oauth`), for APIs with API keys only and one-click
  connectors (claude.ai, ChatGPT). The server is its own authorization server, as MCP's
  authorization spec describes: protected-resource and authorization-server metadata, dynamic
  registration (deprecated by MCP 2026-07-28, kept as the fallback) and Client ID Metadata
  Documents (preferred), authorization code with PKCE (S256 only),
  refresh tokens rotated on use, revocation. Its consent page asks for the user's API key once;
  `validate` can check it against the API first. Each MCP request's access token then maps to
  that key.
- **A resource server** (`resourceServer`, `--authorization-server`), for APIs with their own
  OAuth server: the server points clients at it, checks tokens by introspection (cached a
  minute, and refused when bound to another resource), and calls the API with the token itself.

The broker keeps clients, grants and tokens in a `Store`: memory, a SQLite file
(`sqliteStore`, `--store`), a Cloudflare D1 database (`d1Store`), or KV (`kvStore`, eventually
consistent: a code raced from two locations may serve twice). It stores tokens as their
SHA-256 only and API keys encrypted (AES-GCM) under a key derived from `MCP_OAUTH_SECRET`, so a
copy of the store reveals neither; changing the secret revokes every grant. A client's name on
the consent page is its own claim when it registered dynamically, and the page says so; a client
identified by a metadata document shows that document's host, and the page always names the host
the code goes to; a dynamically registered client keeps its redirects on one host. Documents are
never fetched from loopback, private, link-local or reserved addresses, by name nor, on Node, by
resolution. A refresh token used twice ends its grant; a grant ends after 90 days whatever its
refreshes. Registrations are braked per address, and bodies past 64 KB refused. With `--oauth`
or `--authorization-server`, `--public-url` is required: the issuer and the resource never come
from request headers.

```sh
MCP_OAUTH_SECRET="$(openssl rand -base64 32)" acme-mcp --port 8080 --oauth --store /data/grants.db \
  --public-url https://mcp.acme.com --keys-url https://dashboard.acme.com/api-keys
```
