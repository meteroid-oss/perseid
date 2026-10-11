// Streamable HTTP on Web standard `Request` and `Response`: the same handler serves Node (through
// `perseid-mcp/node`) and Cloudflare Workers. Stateless, as MCP is since 2026-07-28; 2025 clients
// are served statelessly too.
import {
  createMcpHandler,
  isLegacyRequest,
  WebStandardStreamableHTTPServerTransport,
  type AuthInfo,
} from "@modelcontextprotocol/server";

import { createServer, type Api, type Options } from "./server.js";

/** How a request proves who it acts for: the API key the server calls the API with, or the
 * response refusing it. */
export interface Auth {
  /** The routes the auth serves itself (OAuth metadata, authorize, token), if `request` is one. */
  route?(request: Request): Promise<Response | undefined>;
  authenticate(request: Request): Promise<{ apiKey: string } | Response>;
}

export interface HttpOptions extends Omit<Options, "apiKey"> {
  /** By default, `apiKeyAuth()`: each request brings its API key. */
  auth?: Auth;
}

/** The server's HTTP handler: `/mcp`, `/health`, and the routes of its auth. */
export function createHandler(api: Api, options: HttpOptions): (request: Request) => Promise<Response> {
  const auth = options.auth ?? apiKeyAuth(api);
  const serve = (apiKey: string | undefined) => createServer(api, { ...options, apiKey });
  const modern = createMcpHandler((context) => serve(context.authInfo?.token), { legacy: "reject" });
  // 2025 clients: a server per request too, answering in JSON rather than an event stream.
  const legacy = async (request: Request, authInfo: AuthInfo) => {
    const server = serve(authInfo.token);
    const transport = new WebStandardStreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
    await server.connect(transport);
    try {
      return await transport.handleRequest(request, { authInfo });
    } finally {
      void transport.close();
      void server.close();
    }
  };
  return async (request) => {
    const url = new URL(request.url);
    if (url.pathname === "/health") return json(200, { status: "ok", version: api.version });
    // OAuth's forms and registrations are small: bodies past 64 KB are refused unread.
    const declared = Number(request.headers.get("content-length") ?? 0);
    if (declared > (url.pathname === "/mcp" ? 4 * 1024 * 1024 : 64 * 1024)) {
      return json(413, { error: "the request body is too large" });
    }
    if (url.pathname !== "/mcp" && request.body) {
      const read = await small(request);
      if (!read) return json(413, { error: "the request body is too large" });
      request = read;
    }
    const routed = await auth.route?.(request);
    if (routed) return routed;
    if (url.pathname !== "/mcp") return json(404, { error: "not found: the MCP endpoint is /mcp" });
    const credentials = await auth.authenticate(request);
    if (credentials instanceof Response) return credentials;
    const authInfo: AuthInfo = { token: credentials.apiKey, clientId: "", scopes: [] };
    return (await isLegacyRequest(request)) ? legacy(request, authInfo) : modern.fetch(request, { authInfo });
  };
}

/** `request` with its body read into memory, or nothing if it passes 64 KB. */
async function small(request: Request): Promise<Request | undefined> {
  const reader = request.body!.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > 64 * 1024) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    body.set(chunk, at);
    at += chunk.length;
  }
  return new Request(request.url, { method: request.method, headers: request.headers, body });
}

/** Each request brings its API key, `Authorization: Bearer <key>` or `X-API-Key`; `fallback`
 * serves requests without one, for a server only its owner reaches. */
export function apiKeyAuth(api: Api, fallback?: string): Auth {
  return {
    async authenticate(request) {
      const apiKey = bearer(request) ?? (request.headers.get("x-api-key")?.trim() || undefined) ?? fallback;
      if (apiKey) return { apiKey };
      return json(
        401,
        {
          jsonrpc: "2.0",
          id: null,
          error: { code: -32001, message: `missing API key: send Authorization: Bearer <${api.envPrefix}_API_KEY>, or X-API-Key` },
        },
        { "www-authenticate": 'Bearer realm="mcp"' },
      );
    },
  };
}

/** The token of `Authorization: Bearer <token>`. */
export function bearer(request: Request): string | undefined {
  return /^Bearer\s+(.+)$/i.exec(request.headers.get("authorization") ?? "")?.[1]?.trim() || undefined;
}

export function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...headers },
  });
}
