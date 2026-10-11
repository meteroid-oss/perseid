import { copyFile, mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { dirname, extname, join, relative } from "node:path";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { parseSpec } from "../build/spec.js";

/** Mintlify's components that `perseid-docs/mintlify` provides. */
const COMPONENTS = [
  "Accordion", "AccordionGroup", "Card", "CardGroup", "Check", "CodeGroup", "Columns", "Danger",
  "Expandable", "Frame", "Icon", "Info", "Note", "ParamField", "ResponseField", "Step", "Steps",
  "Tab", "Tabs", "Tip", "Update", "Warning",
];

const STATIC = new Set([".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".avif", ".ico", ".mp4", ".webm", ".pdf", ".json", ".txt", ".woff", ".woff2"]);

interface MintNavGroup {
  group: string;
  pages: (string | MintNavGroup)[];
  icon?: string;
}

interface MintTab {
  tab: string;
  groups?: MintNavGroup[];
  pages?: (string | MintNavGroup)[];
  openapi?: string | string[];
  icon?: string;
}

interface MintLink {
  label?: string;
  anchor?: string;
  href: string;
  icon?: string;
}

interface MintConfig {
  navbar?: { links?: MintLink[]; primary?: { type?: string; label?: string; href: string } };
  contextual?: { options?: string[] };
  name?: string;
  colors?: { primary?: string };
  logo?: string | { light?: string; dark?: string; href?: string };
  favicon?: string;
  navigation?: { tabs?: MintTab[]; groups?: MintNavGroup[]; pages?: string[]; global?: { anchors?: MintLink[] } };
  footer?: { socials?: Record<string, string> };
  appearance?: { default?: string };
}

export interface ImportResult {
  pages: number;
  assets: number;
  /** The OpenAPI document the API reference tab named, copied to `api/openapi.json`. */
  openapi?: string;
  warnings: string[];
}

/** Writes an Astro project rendering the Mintlify docs in `from` with perseid-docs. */
export async function importMintlify(from: string, to: string, site?: string): Promise<ImportResult> {
  const config = JSON.parse(await readFile(join(from, "docs.json"), "utf8")) as MintConfig;
  const warnings: string[] = [];
  const files = await walk(from);
  const result: ImportResult = { pages: 0, assets: 0, warnings };
  const nav = config.navigation ?? {};
  const tabs: MintTab[] = nav.tabs ?? [{ tab: "Docs", groups: nav.groups ?? [], pages: nav.pages ?? [] }];
  const allPages = tabs.flatMap((t) => [...(t.groups ?? []).flatMap(pagesOf), ...(t.pages ?? []).flatMap(pagesOf)]);
  const home = allPages.includes("index") ? "index" : allPages[0];

  for (const file of files) {
    const rel = relative(from, file);
    if (rel.startsWith(".") || rel.startsWith("node_modules")) continue;
    const ext = extname(rel).toLowerCase();
    if (ext === ".mdx" || ext === ".md") {
      if (rel.startsWith("_snippets") || rel.startsWith("snippets") || /^readme\.mdx?$/i.test(rel)) continue;
      const slug = rel.slice(0, -ext.length);
      const target = slug === home ? "index" : slug;
      const converted = convertPage(await readFile(file, "utf8"), warnings, rel);
      await writeOut(join(to, "src/content/docs", `${target}.mdx`), converted);
      result.pages++;
    } else if (STATIC.has(ext) && rel !== "docs.json") {
      await mkdir(dirname(join(to, "public", rel)), { recursive: true });
      await copyFile(file, join(to, "public", rel));
      result.assets++;
    }
  }

  const link = (page: string) => (page === home ? "/" : `/${page}/`);
  const sidebarOf = (items: (string | MintNavGroup)[]): object[] =>
    items.map((item) =>
      typeof item === "string"
        ? { slug: item === home ? "index" : item }
        : { label: item.group, items: sidebarOf(item.pages) },
    );
  const topics = tabs.map((t) => {
    const reference = t.openapi !== undefined;
    if (reference) result.openapi = Array.isArray(t.openapi) ? t.openapi[0] : t.openapi;
    const items = reference
      ? '"reference"'
      : JSON.stringify(sidebarOf([...(t.groups ?? []), ...(t.pages ?? [])]), null, 2).replace(/\n/g, "\n      ");
    const first = (t.groups ?? []).flatMap(pagesOf)[0] ?? (t.pages ?? []).flatMap(pagesOf)[0];
    return `    {\n      label: ${JSON.stringify(t.tab)},\n      link: ${JSON.stringify(reference ? "/api-reference/" : link(first ?? home ?? "index"))},\n      items: ${items},\n    }`;
  });

  const logo = typeof config.logo === "string" ? { light: config.logo, dark: config.logo } : config.logo;
  if (logo?.light || logo?.dark) {
    for (const path of [logo.light, logo.dark]) {
      if (path) await copyIfExists(join(from, path), join(to, "src/assets", path));
    }
  }
  const social = Object.entries(config.footer?.socials ?? {}).map(([icon, href]) => ({ icon, label: capitalize(icon), href }));
  const astroConfig = `// @ts-check
import { defineConfig } from "astro/config";
import perseidDocs from "perseid-docs";

export default defineConfig({${site ? `\n  site: ${JSON.stringify(site)},` : ""}
  integrations: [
    perseidDocs({
      title: ${JSON.stringify(config.name ?? "Docs")},${logo?.light || logo?.dark ? `\n      logo: { light: ${JSON.stringify(`./src/assets/${(logo.light ?? logo.dark)!.replace(/^\//, "")}`)}, dark: ${JSON.stringify(`./src/assets/${(logo.dark ?? logo.light)!.replace(/^\//, "")}`)}, replacesTitle: true },` : ""}${config.favicon ? `\n      favicon: ${JSON.stringify(config.favicon)},` : ""}${social.length ? `\n      social: ${JSON.stringify(social)},` : ""}
${extras(config)}      topics: [
${topics.join(",\n")},
      ],
    }),
  ],
});
`;
  await writeOut(join(to, "astro.config.mjs"), astroConfig);
  await writeOut(
    join(to, "src/content.config.ts"),
    `import { defineCollection } from "astro:content";
import { docsLoader, docsSchema } from "perseid-docs/content";

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
};
`,
  );
  await writeOut(
    join(to, "package.json"),
    `${JSON.stringify(
      {
        name: `${(config.name ?? "docs").toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
        private: true,
        type: "module",
        scripts: {
          dev: "astro dev",
          build: "astro build",
          preview: "astro preview",
        },
        dependencies: { astro: "^7.3.6", "perseid-docs": "^0.1.2" },
      },
      null,
      2,
    )}\n`,
  );
  if (result.openapi) await copySpec(from, result.openapi, join(to, "api/openapi.json"), warnings);
  return result;
}

/** The spec the reference tab names, as `api/openapi.json`: the reference shows cURL until the
 * orchestrator adds `api/docs-data.json`. */
async function copySpec(from: string, location: string, to: string, warnings: string[]) {
  try {
    const text = /^https?:\/\//.test(location)
      ? await (await fetch(location)).text()
      : await readFile(join(from, location), "utf8");
    await writeOut(to, `${JSON.stringify(parseSpec(text, location), null, 2)}\n`);
  } catch (e) {
    warnings.push(`${location}: not copied to api/openapi.json (${(e as Error).message})`);
  }
}

/** The header links, sidebar anchors, page menu and default theme of a Mintlify config. */
function extras(config: MintConfig): string {
  const out: string[] = [];
  const links = (config.navbar?.links ?? []).filter((l) => l.href).map((l) => ({ label: l.label ?? l.anchor ?? l.href, href: l.href }));
  const primary = config.navbar?.primary?.href
    ? { label: config.navbar.primary.label ?? "Get started", href: config.navbar.primary.href }
    : undefined;
  if (links.length || primary) out.push(`navbar: ${JSON.stringify({ ...(links.length ? { links } : {}), ...(primary ? { primary } : {}) })},`);
  const anchors = (config.navigation?.global?.anchors ?? []).map((a) => ({
    label: a.anchor ?? a.label ?? a.href,
    href: a.href,
    ...(a.icon ? { icon: a.icon } : {}),
  }));
  if (anchors.length) out.push(`anchors: ${JSON.stringify(anchors)},`);
  const contextual = (config.contextual?.options ?? []).filter((o) => ["copy", "view", "chatgpt", "claude", "perplexity", "grok"].includes(o));
  if (contextual.length) out.push(`contextual: ${JSON.stringify(contextual)},`);
  if (config.appearance?.default === "dark" || config.appearance?.default === "light") {
    out.push(`appearance: ${JSON.stringify(config.appearance.default)},`);
  }
  return out.map((line) => `      ${line}\n`).join("");
}

/** Mintlify pages often write a callout as a paragraph opening on 💡 **Note**: an Aside. */
export function callouts(body: string): { body: string; used: boolean } {
  let used = false;
  const out = body.replace(/^💡\s*\*\*(Note|Tip|Warning|Important)\s*:?\*\*\s*:?\s*(.+)$/gm, (_, kind: string, text: string) => {
    used = true;
    const tag = kind === "Warning" || kind === "Important" ? "Warning" : kind === "Tip" ? "Tip" : "Note";
    return `<${tag}>\n${text.trim()}\n</${tag}>`;
  });
  return { body: out, used };
}

/** Frontmatter as Starlight reads it, and an import of the Mintlify components the page uses. */
export function convertPage(source: string, warnings: string[], file = "page"): string {
  const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(source);
  const parsed: unknown = match ? parseYaml(match[1]!) : {};
  const front = (typeof parsed === "string" ? { title: parsed } : parsed ?? {}) as Record<string, unknown>;
  const original = match ? source.slice(match[0].length) : source;
  const out: Record<string, unknown> = { title: front.title ?? titleFrom(original) ?? file };
  if (front.description) out.description = front.description;
  if (front.sidebarTitle) out.sidebar = { label: front.sidebarTitle };
  if (front.mode === "frame" || front.mode === "custom") {
    out.template = "splash";
    out.pagefind = true;
  }
  if (front.mode === "wide") out.tableOfContents = false;
  for (const key of Object.keys(front)) {
    if (!["title", "description", "sidebarTitle", "mode", "icon", "iconType", "og:title", "og:description", "ContentWidth"].includes(key)) {
      warnings.push(`${file}: frontmatter \`${key}\` has no equivalent, dropped`);
    }
  }
  const body = callouts(original).body;
  const plain = stripCode(body);
  const used = COMPONENTS.filter((name) => new RegExp(`<${name}[\\s>/]`).test(plain));
  const imports = used.length ? `import { ${used.join(", ")} } from "perseid-docs/mintlify";\n\n` : "";
  const content = body.replace(/^\s+/, "");
  const framed = front.mode === "frame" || front.mode === "custom" ? `<div class="pd-frame-page" />\n\n${content}` : content;
  return `---\n${stringifyYaml(out).trimEnd()}\n---\n\n${imports}${framed}`;
}

function titleFrom(body: string): string | undefined {
  return /^#\s+(.+)$/m.exec(body)?.[1]?.trim();
}

/** The text outside fenced and inline code. */
function stripCode(text: string): string {
  return text
    .split(/^```[\s\S]*?^```/m)
    .map((t) => t.replace(/`[^`\n]*`/g, ""))
    .join("");
}

function pagesOf(item: string | MintNavGroup): string[] {
  return typeof item === "string" ? [item] : item.pages.flatMap(pagesOf);
}

async function walk(dir: string): Promise<string[]> {
  const out: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await walk(path)));
    else out.push(path);
  }
  return out;
}

async function writeOut(path: string, content: string) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content);
}

async function copyIfExists(from: string, to: string) {
  try {
    await stat(from);
    await mkdir(dirname(to), { recursive: true });
    await copyFile(from, to);
  } catch {}
}

function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}
