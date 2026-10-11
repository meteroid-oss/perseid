import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, isAbsolute, resolve } from "node:path";
import { parse as parseToml } from "smol-toml";
import type { Reference } from "../reference/model.js";
import type { DocsData } from "./data.js";
import { runPerseid, type PerseidOptions } from "./perseid.js";
import { assemble } from "./reference.js";
import { parseSpec, type Obj } from "./spec.js";

export interface BuildOptions extends PerseidOptions {
  spec: string;
  /** Over the config's `base_url` and the spec's first server. */
  baseUrl?: string;
  /** `false` builds from the spec alone: no SDK names nor samples, curl only. */
  perseid?: boolean;
  /** Docs data `perseid docs-data` already wrote, over running perseid. */
  data?: string;
}

/** The `spec` a `perseid.toml` names, as a URL or a path from the working directory. */
export async function configSpec(config: string | undefined): Promise<string | undefined> {
  if (!config || !existsSync(config)) return undefined;
  const spec = parseToml(await readFile(config, "utf8")).spec;
  if (typeof spec !== "string") return undefined;
  return /^https?:\/\//.test(spec) || isAbsolute(spec) ? spec : resolve(dirname(config), spec);
}

export async function loadSpec(location: string): Promise<Obj> {
  return parseSpec(await readText(location), location);
}

async function readText(location: string): Promise<string> {
  return /^https?:\/\//.test(location) ? (await fetch(location)).text() : readFile(location, "utf8");
}

export async function buildReference(options: BuildOptions): Promise<Reference> {
  const spec = await loadSpec(options.spec);
  const data = options.data
    ? (JSON.parse(await readText(options.data)) as DocsData)
    : options.perseid !== false
      ? await runPerseid(options)
      : undefined;
  return assemble(spec, data, options.baseUrl ? { baseUrl: options.baseUrl } : {});
}
