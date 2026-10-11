#!/usr/bin/env node
import { fileURLToPath } from "node:url";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { parseArgs } from "node:util";
import { buildReference, configSpec, loadSpec } from "./build/node.js";
import { applyOverlay, overlay } from "./build/overlay.js";
import { readFile } from "node:fs/promises";
import { renderPage, type Reference } from "./reference/index.js";
import { shikiHighlighter } from "./highlight.js";
import { importMintlify } from "./mintlify/import.js";

const USAGE = `perseid-docs import-mintlify <mintlify dir> [--out site] [--site https://docs.acme.com]
perseid-docs page <reference.json> [--out index.html]
perseid-docs build [--config perseid.toml] [--spec <path|url>] [--out reference.json]
                   [--overlay openapi.docs.json] [--merged <file>] [--base-url <url>]
                   [--languages typescript,python] [--perseid <bin>] [--no-sdk]
                   [--data docs-data.json]`;

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  if (command === "page") return page(rest);
  if (command === "import-mintlify") return importCommand(rest);
  const { values } = parseArgs({
    args: rest,
    options: {
      config: { type: "string" },
      spec: { type: "string" },
      out: { type: "string", default: "reference.json" },
      overlay: { type: "string" },
      merged: { type: "string" },
      "base-url": { type: "string" },
      languages: { type: "string" },
      perseid: { type: "string" },
      name: { type: "string" },
      "no-sdk": { type: "boolean" },
      data: { type: "string" },
    },
  });
  if (command !== "build") {
    console.error(USAGE);
    process.exit(command ? 2 : 0);
  }
  const config = values.config ?? (values["no-sdk"] ? undefined : "perseid.toml");
  const spec = values.spec ?? (await configSpec(config)) ?? "openapi.json";
  const reference = await buildReference({
    spec,
    ...(config ? { config } : {}),
    ...(values["base-url"] ? { baseUrl: values["base-url"] } : {}),
    ...(values.languages ? { languages: values.languages.split(",") } : {}),
    ...(values.perseid ? { bin: values.perseid } : {}),
    ...(values.name ? { name: values.name } : {}),
    ...(values.data ? { data: values.data } : {}),
    perseid: !values["no-sdk"],
  });
  await write(values.out!, reference);
  if (values.overlay) await write(values.overlay, overlay(reference));
  if (values.merged) await write(values.merged, applyOverlay(await loadSpec(spec), reference));
  const ops = reference.groups.reduce((n, g) => n + g.operations.length, 0);
  console.error(`${values.out}: ${reference.groups.length} groups, ${ops} operations, ${reference.languages.length} languages`);
}

async function importCommand(args: string[]) {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: { out: { type: "string", default: "site" }, site: { type: "string" } },
  });
  const result = await importMintlify(positionals[0] ?? ".", values.out!, values.site);
  for (const w of result.warnings) console.error(`warning: ${w}`);
  console.error(`${values.out}: ${result.pages} pages, ${result.assets} assets${result.openapi ? `, API reference from ${result.openapi}` : ""}`);
}

async function page(args: string[]) {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: { out: { type: "string", default: "index.html" }, head: { type: "string" } },
  });
  const reference = JSON.parse(await readFile(positionals[0] ?? "reference.json", "utf8")) as Reference;
  const css = await readFile(fileURLToPath(new URL("../styles/reference.css", import.meta.url)), "utf8");
  const script = await readFile(fileURLToPath(new URL("../dist/reference/client.js", import.meta.url)), "utf8");
  const head = values.head
    ? await readFile(values.head, "utf8")
    : `<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600&family=Geist+Mono:wght@400;500;600&display=swap">`;
  const html = renderPage(reference, { css, script, head, highlight: await shikiHighlighter() });
  await mkdir(dirname(values.out!), { recursive: true });
  await writeFile(values.out!, html);
  console.error(`${values.out}: ${(html.length / 1024).toFixed(0)} KiB`);
}

async function write(file: string, value: unknown) {
  await mkdir(dirname(file), { recursive: true });
  await writeFile(file, `${JSON.stringify(value, null, 2)}\n`);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
