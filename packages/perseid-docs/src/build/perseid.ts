import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { DataResource, DocsData, SdkData } from "./data.js";

const run = promisify(execFile);

export const LANGUAGES = ["typescript", "python", "go", "java", "csharp", "rust"] as const;

const TEMPLATES = join(dirname(fileURLToPath(import.meta.url)), "../../perseid/templates");

/** perseid's API model, as `perseid inspect` prints it: the parts the docs read. */
interface PerseidResource {
  name: string;
  operations: { id: string }[];
  subresources?: Record<string, PerseidResource>;
}

export interface PerseidOptions {
  /** The perseid binary. */
  bin?: string;
  /** `perseid.toml`; without one, the SDKs are named after the spec. */
  config?: string;
  /** Over the config's `spec`. */
  spec?: string;
  /** Over the config's `sdks`. */
  languages?: string[];
  name?: string;
}

/** `perseid docs-data` on a copy of the project. Before perseid had it: `perseid generate` with
 * templates writing `perseid-docs.json` in place of each SDK's `api.md`, next to that SDK's own
 * ejected `api_reference` template, so signatures come from the installed perseid. */
export async function runPerseid(options: PerseidOptions): Promise<DocsData> {
  const bin = options.bin ?? "perseid";
  const work = await mkdtemp(join(tmpdir(), "perseid-docs-"));
  try {
    const config: Record<string, unknown> = options.config
      ? parseToml(await readFile(options.config, "utf8"))
      : {};
    const root = options.config ? dirname(resolve(options.config)) : process.cwd();
    const spec = options.spec ?? (config.spec as string | undefined) ?? "openapi.json";
    config.spec = /^https?:\/\//.test(spec) || isAbsolute(spec) ? spec : resolve(root, spec);
    if (options.name) config.name = options.name;
    config.name ??= "Client";
    const languages = (options.languages ?? (config.sdks as string[] | undefined) ?? [...LANGUAGES]).filter(
      (l) => existsSync(join(TEMPLATES, l)),
    );
    config.sdks = languages;
    const overrides = join(work, "overrides");
    if (typeof config.overrides === "string" && existsSync(resolve(root, config.overrides))) {
      await cp(resolve(root, config.overrides), overrides, { recursive: true });
    }
    config.overrides = overrides;
    config.tests = false;
    config.round_trips = false;
    delete config.repo;
    for (const language of LANGUAGES) {
      if (!languages.includes(language)) delete config[language];
      else if (typeof config[language] === "object") delete (config[language] as Record<string, unknown>).repo;
    }
    const toml = join(work, "perseid.toml");
    await writeFile(toml, stringifyToml(config));

    if (await hasDocsData(bin, toml, work)) {
      const { stdout } = await run(bin, ["-c", toml, "docs-data"], { cwd: work, maxBuffer: 256 << 20 });
      const data = JSON.parse(stdout) as DocsData;
      const ordered = languages.filter((l) => data.languages[l]).map((l) => [l, data.languages[l]!] as const);
      return { ...data, languages: Object.fromEntries(ordered) };
    }
    if (await nestedLiterals(bin, work)) {
      config.context = { ...(config.context as object | undefined), perseid_docs_nested: true };
      await writeFile(toml, stringifyToml(config));
    }
    for (const language of languages) {
      const dir = join(overrides, "templates", language);
      await rm(join(dir, "api_reference.md.jinja"), { force: true });
      await run(bin, ["-c", toml, "eject", language, "templates/api_reference.md.jinja"], { cwd: work });
      await rename(join(dir, "api_reference.md.jinja"), join(dir, "perseid_builtin_reference.jinja"));
      for (const source of ["_common", language]) {
        for (const file of await readdir(join(TEMPLATES, source))) {
          await cp(join(TEMPLATES, source, file), join(dir, file));
        }
      }
    }
    const out = join(work, "out");
    await run(bin, ["-c", toml, "generate", "--out", out, "--no-format"], {
      cwd: work,
      maxBuffer: 64 << 20,
    });
    const { stdout } = await run(bin, ["-c", toml, "inspect"], { cwd: work, maxBuffer: 256 << 20 });
    const sdks: Record<string, SdkData> = {};
    for (const language of languages) {
      const file = join(out, language, "perseid-docs.json");
      sdks[language] = JSON.parse(await readFile(file, "utf8")) as SdkData;
    }
    const toData = (r: PerseidResource): DataResource => ({
      name: r.name,
      operations: r.operations.map((o) => o.id),
      subresources: Object.values(r.subresources ?? {}).map(toData),
    });
    return {
      name: String(config.name),
      base_url: (config.base_url as string | undefined) ?? null,
      resources: (JSON.parse(stdout) as { resources: PerseidResource[] }).resources.map(toData),
      languages: sdks,
    };
  } finally {
    await rm(work, { recursive: true, force: true });
  }
}

/** Whether the installed perseid writes the docs data itself (`perseid docs-data`). */
async function hasDocsData(bin: string, toml: string, work: string): Promise<boolean> {
  try {
    await run(bin, ["-c", toml, "docs-data", "--help"], { cwd: work });
    return true;
  } catch {
    return false;
  }
}

/** Whether the installed perseid's `docs.jinja` writes list and struct literals (#105). */
async function nestedLiterals(bin: string, work: string): Promise<boolean> {
  const probe = join(work, "probe");
  try {
    await mkdir(probe, { recursive: true });
    await writeFile(join(probe, "perseid.toml"), stringifyToml({ name: "Probe", sdks: ["typescript"], overrides: "o" }));
    await run(bin, ["-c", join(probe, "perseid.toml"), "eject", "typescript", "templates/docs.jinja"], { cwd: probe });
    return (await readFile(join(probe, "o/templates/typescript/docs.jinja"), "utf8")).includes('arg.kind == "list"');
  } catch {
    return false;
  } finally {
    await rm(probe, { recursive: true, force: true });
  }
}
