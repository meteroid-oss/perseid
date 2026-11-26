import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import starlight from "@astrojs/starlight";
import type { StarlightUserConfig } from "@astrojs/starlight/types";
import starlightSidebarTopics from "starlight-sidebar-topics";
import type { AstroIntegration } from "astro";
import { registerBaseLinks } from "./base-links.js";
import { formatHref, loadRoutes, sidebar as referenceSidebar, type ReferenceSource, type SiteReference, type UrlFormat } from "./routes.js";

type Sidebar = NonNullable<StarlightUserConfig["sidebar"]>;

export interface Topic {
  label: string;
  link: string;
  icon?: string;
  /** The sidebar of this topic: `"reference"` for the API reference's, `"reference:<id>"` for
   * the one of `references` with that `id`. */
  items: Sidebar | "reference" | `reference:${string}`;
}

/** Another API reference, such as a previous version of the API. */
export interface ReferenceOptions {
  /** Names it in `topics`: `items: "reference:v1"`. */
  id: string;
  /** The label of its sidebar group without topics: `API Reference (<id>)` by default. */
  label?: string;
  /** The docs model `perseid-docs build` writes, as `reference`. */
  reference?: string;
  /** The folder holding `openapi.json` and, optionally, `docs-data.json`, as `api`. Without docs
   * data, the reference shows cURL. */
  api?: string | { spec: string; data?: string };
  /** URL prefix: `api-reference/<id>` by default. */
  base?: string;
  /** A superseded version: a banner on each page links to the main reference. `true` writes the
   * banner, a string is its HTML. */
  legacy?: boolean | string;
}

export interface PerseidDocsOptions {
  title: string;
  description?: string;
  /** The docs model `perseid-docs build` writes. */
  reference?: string;
  /** The folder holding `openapi.json` and `docs-data.json`, as the orchestrator writes it, or
   * their paths. `api/` is read by default when `api/openapi.json` exists. */
  api?: string | { spec: string; data?: string };
  /** URL prefix of the reference: `api-reference` by default, as Mintlify's. */
  referenceBase?: string;
  /** More API references, such as the legacy version of the API. */
  references?: ReferenceOptions[];
  /** The sidebar of the guides, the reference's appended as its own group. */
  sidebar?: Sidebar;
  /** Sections with a sidebar each, switched at the top of the sidebar, as Mintlify's tabs. */
  topics?: Topic[];
  logo?: StarlightUserConfig["logo"];
  favicon?: string;
  social?: StarlightUserConfig["social"];
  customCss?: string[];
  /** Brand color of links, buttons and the selected language. */
  accent?: string;
  /** Links at the right of the header, and its call to action. */
  navbar?: { links?: { label: string; href: string }[]; primary?: { label: string; href: string } };
  /** Links at the top of the sidebar, with a Tabler icon each. */
  anchors?: { label: string; href: string; icon?: string }[];
  /** The page menu next to each title: `copy`, `view`, `chatgpt`, `claude`, `perplexity`, `grok`. */
  contextual?: string[];
  /** The theme until the reader picks one. */
  appearance?: "dark" | "light";
  /** Fonts loaded from Google Fonts: Geist and Geist Mono by default, `false` for none. */
  fonts?: string | false;
  /** Anything else Starlight takes, over what perseid-docs sets. */
  starlight?: Partial<StarlightUserConfig>;
}

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/** Packages the site imports through Starlight's generated code, resolved from perseid-docs:
 * the site depends on perseid-docs alone. */
const HIDDEN = /^(@astrojs\/starlight|starlight-sidebar-topics|@astrojs\/mdx|@astrojs\/sitemap|astro-expressive-code|@expressive-code\/|@pagefind\/|pagefind)(\/|$)/;

/** Tabler's icon set, one JSON file, resolved here: the site does not depend on it. */
function iconsDir(): string | undefined {
  try {
    return createRequire(import.meta.url).resolve("@iconify-json/tabler/icons.json");
  } catch {
    return undefined;
  }
}

function head(options: PerseidDocsOptions): NonNullable<StarlightUserConfig["head"]> {
  const out: NonNullable<StarlightUserConfig["head"]> = [];
  const fonts = options.fonts ?? "family=Geist:wght@400;500;600;700&family=Geist+Mono:wght@400;500;600";
  if (fonts) {
    out.push(
      { tag: "link", attrs: { rel: "preconnect", href: "https://fonts.googleapis.com" } },
      { tag: "link", attrs: { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: true } },
      { tag: "link", attrs: { rel: "stylesheet", href: `https://fonts.googleapis.com/css2?${fonts}&display=swap` } },
    );
  }
  if (options.appearance) {
    const theme = JSON.stringify(options.appearance);
    out.push({
      tag: "script",
      content: `try{if(!localStorage.getItem("starlight-theme"))document.documentElement.dataset.theme=${theme}}catch(e){}`,
    });
  }
  return out;
}

function referenceSource(root: string, options: Pick<ReferenceOptions, "reference" | "api">, fallback: boolean): ReferenceSource | undefined {
  if (options.reference) {
    const file = resolve(root, options.reference);
    if (!existsSync(file)) throw new Error(`perseid-docs: ${file} is missing: run \`perseid-docs build\` first`);
    return file;
  }
  const api = options.api ?? (fallback && existsSync(resolve(root, "api/openapi.json")) ? "api" : undefined);
  if (!api) return undefined;
  const spec = resolve(root, typeof api === "string" ? join(api, "openapi.json") : api.spec);
  const data = typeof api === "string" ? resolve(root, api, "docs-data.json") : api.data && resolve(root, api.data);
  if (!existsSync(spec)) throw new Error(`perseid-docs: ${spec} is missing`);
  return data && existsSync(data) ? { spec, data } : { spec };
}

const escapeHtml = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const trim = (path: string) => path.replace(/^\/+|\/+$/g, "");

function siteReferences(root: string, options: PerseidDocsOptions, format: UrlFormat): SiteReference[] {
  const main = trim(options.referenceBase ?? "api-reference");
  const out: SiteReference[] = [];
  const source = referenceSource(root, options, true);
  if (source) out.push({ id: "", label: "API Reference", source, base: main });
  for (const r of options.references ?? []) {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(r.id)) throw new Error(`perseid-docs: name references in lowercase letters, digits and dashes, not "${r.id}"`);
    const extra = referenceSource(root, r, false);
    if (!extra) throw new Error(`perseid-docs: set \`api\` or \`reference\` of the reference "${r.id}"`);
    const label = r.label ?? `API Reference (${r.id})`;
    const current = source ? ` <a href="${formatHref(main, format)}">Read the current API reference</a>.` : "";
    const banner = typeof r.legacy === "string" ? r.legacy : r.legacy ? `${escapeHtml(label)} documents a legacy version of the API.${current}` : undefined;
    out.push({ id: r.id, label, source: extra, base: trim(r.base ?? `${main}/${r.id}`), ...(banner ? { banner } : {}) });
  }
  const bases = new Set(out.map((r) => r.base));
  if (bases.size !== out.length) throw new Error("perseid-docs: give each reference its own `base`");
  return out;
}

const topicId = (id: string) => (id ? `perseid-reference-${id}` : "perseid-reference");

export default function perseidDocs(options: PerseidDocsOptions): AstroIntegration {
  return {
    name: "perseid-docs",
    hooks: {
      "astro:config:setup": ({ config, updateConfig, injectRoute, logger }) => {
        const root = fileURLToPath(config.root);
        const format: UrlFormat = { base: config.base, trailingSlash: config.trailingSlash };
        const references = siteReferences(root, options, format);
        if (!registerBaseLinks((config.markdown as unknown as { processor?: Parameters<typeof registerBaseLinks>[0] }).processor, config.base)) {
          logger.warn(`the Markdown processor isn't Sätteri or unified: links in pages won't take the base ${config.base}`);
        }
        const sidebars = new Map(references.map((r) => [r.id, referenceSidebar(loadRoutes(r.source, r.base, format)) as Sidebar]));
        const plugins = [...(options.starlight?.plugins ?? [])];
        let sidebar: Sidebar | undefined = options.sidebar;
        if (options.topics?.length) {
          const referenceOf = (items: Topic["items"]) =>
            items === "reference" ? "" : typeof items === "string" ? items.slice("reference:".length) : undefined;
          for (const t of options.topics) {
            const id = referenceOf(t.items);
            if (id !== undefined && !sidebars.has(id)) {
              throw new Error(`perseid-docs: the topic "${t.label}" shows ${id ? `the reference "${id}", which \`references\` lacks` : "the API reference, which has no `api` or `reference`"}`);
            }
          }
          // Most specific first: a reference under another's base takes its own pages.
          const patterns = Object.fromEntries(
            [...references]
              .sort((a, b) => b.base.length - a.base.length)
              .filter((r) => options.topics!.some((t) => referenceOf(t.items) === r.id))
              .map((r) => [topicId(r.id), [`/${r.base}/**`]]),
          );
          const guides = options.topics.findIndex((t) => referenceOf(t.items) === undefined);
          plugins.push(
            starlightSidebarTopics(
              options.topics.map((t, i) => {
                const id = referenceOf(t.items);
                return {
                  id: id === undefined ? `topic-${i}` : topicId(id),
                  label: t.label,
                  link: t.link,
                  ...(t.icon ? { icon: t.icon } : {}),
                  items: (id === undefined ? t.items : sidebars.get(id)) as Sidebar,
                };
              }) as never,
              { topics: { ...patterns, ...(guides >= 0 ? { [`topic-${guides}`]: ["**"] } : {}) } } as never,
            ),
          );
          sidebar = undefined;
        } else if (references.length) {
          sidebar = [...(options.sidebar ?? []), ...references.map((r) => ({ label: r.label, items: sidebars.get(r.id)! }))];
        }
        updateConfig({
          integrations: [
            starlight({
              title: options.title,
              ...(options.description ? { description: options.description } : {}),
              ...(options.logo ? { logo: options.logo } : {}),
              ...(options.favicon ? { favicon: options.favicon } : {}),
              ...(options.social ? { social: options.social } : {}),
              ...(sidebar ? { sidebar } : {}),
              customCss: [here("../../astro/theme.css"), ...(options.customCss ?? [])],
              head: [...head(options), ...(options.starlight?.head ?? [])],
              components: {
                Header: here("../../astro/components/Header.astro"),
                Sidebar: here("../../astro/components/Sidebar.astro"),
                PageTitle: here("../../astro/components/PageTitle.astro"),
                ...(options.starlight?.components ?? {}),
              },
              ...options.starlight,
              plugins,
            }),
          ],
          vite: {
            plugins: [
              {
                name: "perseid-docs-hidden-deps",
                enforce: "pre",
                async resolveId(this: { resolve: (id: string, importer: string, o: object) => Promise<unknown> }, id: string) {
                  return HIDDEN.test(id) ? this.resolve(id, here("./index.js"), { skipSelf: true }) : undefined;
                },
              },
              {
                name: "perseid-docs-config",
                resolveId: (id: string) => (id === "virtual:perseid-docs/config" ? "\0perseid-docs-config" : undefined),
                load: (id: string) =>
                  id === "\0perseid-docs-config"
                    ? `export default ${JSON.stringify({
                        references,
                        format,
                        accent: options.accent,
                        icons: iconsDir(),
                        assets: { css: here("../../styles/reference.css"), script: here("../reference/client.js") },
                        title: options.title,
                        description: options.description,
                        navbar: options.navbar,
                        anchors: options.anchors,
                        contextual: options.contextual,
                        appearance: options.appearance,
                      })};`
                    : undefined,
              },
            ] as never,
          },
        });
        injectRoute({ pattern: "/[...slug].md", entrypoint: here("../../astro/routes/markdown.ts"), prerender: true });
        injectRoute({ pattern: "/llms.txt", entrypoint: here("../../astro/routes/llms.ts"), prerender: true });
        for (const r of references) {
          injectRoute({ pattern: `/${r.base}/[...slug]`, entrypoint: here("../../astro/ReferencePage.astro"), prerender: true });
        }
      },
    },
  };
}
