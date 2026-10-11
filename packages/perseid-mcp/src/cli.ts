#!/usr/bin/env node
// `perseid-mcp sources <api.js> --out <file>`: the SDK sources `search` quotes, as JSON, for a
// Worker to bundle: it has no files to read them from at run time.
// `perseid-mcp runner`: runs scripts for servers started with `--sandbox remote`.
// `perseid-mcp mcpb <api.js> --out manifest.json`: the manifest of the server's MCP bundle.
import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { availableParallelism } from "node:os";

import { packageSources } from "./node.js";
import { serveRunner } from "./runner.js";
import { limitConcurrency } from "./sandbox/index.js";
import type { Api } from "./server.js";

const USAGE = `Usage: perseid-mcp sources <dist/generated/api.js> --out <file.json>
       perseid-mcp runner [--port 8090] [--host 0.0.0.0] [--sandbox deno|quickjs]
       perseid-mcp mcpb <dist/generated/api.js> --out <manifest.json>

sources: writes the TypeScript sources of the SDK that the API's index refers to, by path, as
JSON: what search quotes declarations from, for a Cloudflare Worker to bundle.

runner: runs the scripts of servers started with --sandbox remote, in another container that
holds no API key: their calls go back to the server. Requests must bring MCP_RUNNER_TOKEN.

mcpb: writes the manifest.json of the server's MCP bundle (.mcpb), for Claude Desktop and the
clients installing bundles: the server on Node, scripts in QuickJS (no Deno to ship), the API
key asked for at install.
`;

async function main(argv: string[]): Promise<void> {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      out: { type: "string" },
      port: { type: "string", default: "8090" },
      host: { type: "string", default: "0.0.0.0" },
      sandbox: { type: "string", default: "deno" },
      help: { type: "boolean", short: "h" },
    },
  });
  const [command, module] = positionals;
  if (command === "runner" && !values.help) {
    const token = process.env.MCP_RUNNER_TOKEN;
    if (!token || token.length < 32) throw new Error("runner needs MCP_RUNNER_TOKEN, 32 characters or more");
    const sandbox =
      values.sandbox === "quickjs"
        ? (await import("./sandbox/threads.js")).quickjsThreadsSandbox()
        : limitConcurrency((await import("./sandbox/deno.js")).denoSandbox(), availableParallelism() * 2);
    serveRunner({ port: Number(values.port), host: values.host!, token, sandbox });
    return;
  }
  if (values.help || (command !== "sources" && command !== "mcpb") || !module || !values.out) {
    process.stdout.write(USAGE);
    process.exitCode = values.help ? 0 : 2;
    return;
  }
  const url = pathToFileURL(resolve(module)).href;
  const { api } = (await import(url)) as { api: Api };
  if (command === "mcpb") {
    writeFileSync(values.out, JSON.stringify(manifest(api), null, 2) + "\n");
    process.stderr.write(`the MCP bundle manifest of ${api.server} ${api.version} in ${values.out}\n`);
    return;
  }
  const read = packageSources(api.corpus.package, url);
  const files = new Set<string>([
    ...api.corpus.operations.flatMap((operation) => operation.types.map(([, file]) => file)),
    ...Object.values(api.corpus.models),
  ]);
  const sources: Record<string, string> = {};
  for (const file of [...files].sort()) {
    const text = read(file);
    if (text !== undefined) sources[file] = text;
  }
  writeFileSync(values.out, JSON.stringify(sources));
  process.stderr.write(`${Object.keys(sources).length} sources of ${api.corpus.package} in ${values.out}\n`);
}

/** The MCP bundle manifest (spec 0.3) of the server of `api`. */
function manifest(api: Api) {
  return {
    manifest_version: "0.3",
    name: api.server,
    display_name: api.name,
    version: api.version,
    description: `The ${api.name} API in code mode: search its TypeScript SDK, and run scripts against it in a sandbox.`,
    author: { name: api.name },
    server: {
      type: "node",
      entry_point: "dist/main.js",
      mcp_config: {
        command: "node",
        args: ["${__dirname}/dist/main.js", "--sandbox", "quickjs"],
        env: {
          [`${api.envPrefix}_API_KEY`]: "${user_config.api_key}",
          [`${api.envPrefix}_BASE_URL`]: "${user_config.base_url}",
        },
      },
    },
    tools: [
      { name: "search", description: `Search the ${api.name} SDK's methods and the types they take and return` },
      { name: "execute", description: `Run a script against the ${api.name} API, its calls made with your key` },
    ],
    user_config: {
      api_key: { type: "string", title: `${api.name} API key`, description: `Your ${api.name} API key`, sensitive: true, required: true },
      base_url: { type: "string", title: "Base URL", description: "The API's base URL, if not the SDK's default", required: false, default: "" },
    },
    compatibility: { platforms: ["darwin", "win32", "linux"], runtimes: { node: ">=20.0.0" } },
  };
}

main(process.argv.slice(2)).catch((error: Error) => {
  process.stderr.write(`${error.message}\n`);
  process.exit(1);
});
