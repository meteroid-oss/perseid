# The MCP pack

A hosted [Model Context Protocol](https://modelcontextprotocol.io) server for the API, in code
mode, over its TypeScript SDK: the server for web clients (claude.ai, ChatGPT) that have no shell
to run the CLI with. Clients with a shell get the CLI's own `mcp`, a tool per operation.

```toml
# perseid.toml
sdks = ["typescript"]

[targets.mcp]                       # the built-in pack `mcp`: this folder
wraps = "typescript"
repo = "acme/acme-mcp"
after = "typescript"                # once the TypeScript SDK is released

[context]
mcp_instructions = "Amounts are in cents."   # optional: first in the server's instructions
```

`perseid targets mcp --out <dir>` previews it.

## Tools

Two, as Stainless and Cloudflare serve large APIs, rather than one per operation:

- `search` finds the methods of the SDK whose words match a query (`list customers`,
  `invoice pdf`, `client.subscriptions.create`, a model's name). It returns their signatures and
  routes, and the TypeScript declarations of what they take and return, which the SDK's package
  ships in `src/`.
- `execute` runs a script: `code` is the body of an async function given `client`, an SDK client.
  What it returns comes back as JSON, with what it logs. A model does its loops, filters and joins
  in the script and returns only what it needs, instead of reading every response.

## The sandbox

The runtime is the npm package [`perseid-mcp`](../../packages/perseid-mcp): the generated server
depends on it, so a fix to the sandbox or the transports reaches every server as a version bump.

A script runs in [Deno](https://deno.com) with no permission at all: no network, no files, no
environment, no subprocesses. Its `client` is a proxy: each call goes to the host over stdio,
and the host makes it with the SDK and the request's credentials. Only the SDK's operations are
callable: neither its private members (`_client`) nor anything `search` does not list. The API key never enters the
sandbox, and nothing leaves it but the calls it asks for and the result it returns.

- `await client.things.list()` gives the first page, its body with `items` and `hasNextPage`;
  `for await` walks every item, pages fetched by the host 100 items at a time.
- A failed call throws an `Error` with the API's `status` and `body`.
- Limits: 30 seconds, 256 MB, 100 API calls a script (`--timeout`, `--memory`, `--max-calls`);
  results longer than 60,000 characters fail, asking for less (`--max-chars`).
- `--read-only` refuses every operation but GET and HEAD before it reaches the API.

Deno comes from the `deno` npm package, or `DENO_PATH`, or the `PATH`.

## Transports and credentials

Without `--port` (or `PORT`), on stdio with `<PREFIX>_API_KEY`, for local clients. With it, over
Streamable HTTP on `/mcp`, stateless, with one of three kinds of credentials:

- **An API key per request** (the default): `Authorization: Bearer` or `X-API-Key`; a request
  without one gets a 401 with `WWW-Authenticate`. `--key-from-env` lets requests without a key use
  `<PREFIX>_API_KEY`, for a server only its owner reaches.
- **`--oauth`**: the server is its own OAuth server, for one-click connectors (claude.ai,
  ChatGPT) on APIs with API keys only. Users paste their key once on its consent page; it is kept
  encrypted under `MCP_OAUTH_SECRET` in `--store`, a SQLite file. `--public-url` sets the
  issuer, `--keys-url` links to where users find their key.
- **`--authorization-server <url>`**: the API's own OAuth server issues the tokens; the server
  checks them by introspection (`MCP_INTROSPECTION_URL`, `MCP_CLIENT_ID`, `MCP_CLIENT_SECRET`)
  and calls the API with them.

`/health` answers for load balancers. See [`perseid-mcp`](../../packages/perseid-mcp#credentials-over-http)
for how the broker stores grants.

## The generated repository

| Path | |
|---|---|
| `src/generated/api.ts` | The SDK's client, and the index of its operations and models `search` reads |
| `package.json` | Generated: the server's version is the SDK's, and it depends on that version |
| `src/main.ts`, `Dockerfile`, `.github/` | Written once, then yours |

Merging a regeneration pull request releases it: `release.yml` publishes the package when npm
does not have its version yet (npm trusted publishing, else `NPM_TOKEN`), tags it, and pushes the
image to `ghcr.io/<repo>`, and attaches the MCP bundle (`npm run mcpb`: QuickJS for a sandbox, the API key
asked for at install) to the GitHub release, for Claude Desktop.

## Tests

`tests/run.sh` renders the pack against `tests/acme` (the CLI pack's acme spec) with `$PERSEID`,
builds the TypeScript SDK and the server (on `packages/perseid-mcp`, packed), and runs `tests/e2e.mjs` against the CLI pack's mock
API: search, scripts with pages and errors, the sandbox's walls and limits, read-only, and HTTP
credentials.
