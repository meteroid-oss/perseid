#!/usr/bin/env node
// The server's entry point: the generated API, served by perseid-mcp. Yours to extend.
import { main, packageSources } from "perseid-mcp/node";

import { api } from "./generated/api.js";

main(api, process.argv.slice(2), { sources: packageSources(api.corpus.package, import.meta.url) }).catch((error: Error) => {
  process.stderr.write(`${error.message}\n`);
  process.exit(2);
});
