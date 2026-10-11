// For APIs with their own OAuth server: the MCP server is only a resource server. It points
// clients at that server, checks each token by introspection (RFC 7662), and calls the API with
// the token itself.
import type { Auth } from "../http.js";
import { json } from "../http.js";
import { sha256 } from "./store.js";

export interface ResourceOptions {
  /** The authorization servers clients get tokens from. */
  authorizationServers: string[];
  /** Scopes to advertise; tokens are not checked for them. */
  scopes?: string[];
  /** The server's public URL: by default the origin of each request. */
  url?: string;
  /** Checks a token: its introspection endpoint, with this server's credentials. Without it, the
   * API checks tokens itself, on every call. */
  introspection?: { url: string; clientId: string; clientSecret: string };
  /** Seconds an introspection result is reused for (60): a revoked token works that long. */
  cacheTtl?: number;
  fetch?: typeof fetch;
}

export function resourceServer(options: ResourceOptions): Auth {
  const send = options.fetch ?? fetch;
  const origin = (request: Request) => (options.url ?? new URL(request.url).origin).replace(/\/+$/, "");
  const cache = new Map<string, { active: boolean; until: number }>();
  const ttl = (options.cacheTtl ?? 60) * 1000;

  async function active(token: string, resource: string): Promise<boolean> {
    const { introspection } = options;
    if (!introspection) return true;
    const key = await sha256(token);
    const hit = cache.get(key);
    if (hit && hit.until > Date.now()) return hit.active;
    const response = await send(introspection.url, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        accept: "application/json",
        authorization: `Basic ${btoa(`${encodeURIComponent(introspection.clientId)}:${encodeURIComponent(introspection.clientSecret)}`)}`,
      },
      body: new URLSearchParams({ token, token_type_hint: "access_token" }),
    });
    if (!response.ok) throw new Error(`token introspection answered ${response.status}`);
    const result = (await response.json()) as { active?: boolean; aud?: string | string[]; exp?: number };
    const audiences = result.aud === undefined ? [] : Array.isArray(result.aud) ? result.aud : [result.aud];
    // A token bound to another resource is not for this server (RFC 8707).
    const ok = result.active === true && (!audiences.length || audiences.some((a) => a.replace(/\/+$/, "") === resource));
    const until = Math.min(Date.now() + ttl, result.exp ? result.exp * 1000 : Infinity);
    if (cache.size > 10_000) cache.clear();
    cache.set(key, { active: ok, until });
    return ok;
  }

  const unauthorized = (request: Request, description: string) =>
    json(
      401,
      { jsonrpc: "2.0", id: null, error: { code: -32001, message: description } },
      {
        "www-authenticate": `Bearer resource_metadata="${origin(request)}/.well-known/oauth-protected-resource", error="invalid_token", error_description="${description}"`,
      },
    );

  return {
    async route(request) {
      const path = new URL(request.url).pathname.replace(/\/+$/, "");
      if (path !== "/.well-known/oauth-protected-resource" && path !== "/.well-known/oauth-protected-resource/mcp") return undefined;
      return json(
        200,
        {
          resource: `${origin(request)}/mcp`,
          authorization_servers: options.authorizationServers,
          scopes_supported: options.scopes,
          bearer_methods_supported: ["header"],
        },
        { "access-control-allow-origin": "*" },
      );
    },

    async authenticate(request) {
      const token = /^Bearer\s+(.+)$/i.exec(request.headers.get("authorization") ?? "")?.[1]?.trim();
      if (!token) return unauthorized(request, "authorization required: connect through OAuth");
      if (!(await active(token, `${origin(request)}/mcp`))) return unauthorized(request, "the access token is inactive or not for this server");
      return { apiKey: token };
    },
  };
}
