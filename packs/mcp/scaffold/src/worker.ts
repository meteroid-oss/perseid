// The server as a Cloudflare Worker: `npm run deploy`. Yours to extend. Its settings are in
// wrangler.jsonc; the SDK sources `search` quotes are bundled from src/generated/sources.json,
// which `npm run build:worker` writes.
import { mcpWorker } from "perseid-mcp/cloudflare";

import { api } from "./generated/api.js";
import sources from "./generated/sources.json";

export default mcpWorker(api, { sources });
