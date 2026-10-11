import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import starlight from "@astrojs/starlight";
import type { StarlightUserConfig } from "@astrojs/starlight/types";
import starlightSidebarTopics from "starlight-sidebar-topics";
import type { AstroIntegration } from "astro";
import { loadRoutes, sidebar as referenceSidebar, type ReferenceSource } from "./routes.js";

type Sidebar = NonNullable<StarlightUserConfig["sidebar"]>;

export interface Topic {
  label: string;
  link: string;
  icon?: string;
  /** The sidebar of this topic; `"reference"` for the API reference's. */
  items: Sidebar | "reference";
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

function referenceSource(root: string, options: PerseidDocsOptions): ReferenceSource | undefined {
  if (options.reference) {
    const file = resolve(root, options.reference);
    if (!existsSync(file)) throw new Error(`perseid-docs: ${file} is missing: run \`perseid-docs build\` first`);
    return file;
  }
  const api = options.api ?? (existsSync(resolve(root, "api/openapi.json")) ? "api" : undefined);
  if (!api) return undefined;
  const spec = resolve(root, typeof api === "string" ? join(api, "openapi.json") : api.spec);
  const data = typeof api === "string" ? resolve(root, api, "docs-data.json") : api.data && resolve(root, api.data);
  if (!existsSync(spec)) throw new Error(`perseid-docs: ${spec} is missing`);
  return data && existsSync(data) ? { spec, data } : { spec };
}

export default function perseidDocs(options: PerseidDocsOptions): AstroIntegration {
  const base = (options.referenceBase ?? "api-reference").replace(/^\/|\/$/g, "");
  return {
    name: "perseid-docs",
    hooks: {
      "astro:config:setup": ({ config, updateConfig, injectRoute }) => {
        const root = fileURLToPath(config.root);
        const source = referenceSource(root, options);
        const routes = source ? loadRoutes(source, base) : undefined;
        const reference = routes ? referenceSidebar(routes) : [];
        const plugins = [...(options.starlight?.plugins ?? [])];
        let sidebar: Sidebar | undefined = options.sidebar;
        if (options.topics?.length) {
          plugins.push(
            starlightSidebarTopics(
              options.topics.map((t, i) => ({
                id: t.items === "reference" ? "perseid-reference" : `topic-${i}`,
                label: t.label,
                link: t.link,
                ...(t.icon ? { icon: t.icon } : {}),
                items: (t.items === "reference" ? reference : t.items) as Sidebar,
              })) as never,
              {
                topics: {
                  "perseid-reference": [`/${base}/**`],
                  [`topic-${options.topics.findIndex((t) => t.items !== "reference")}`]: ["**"],
                },
              } as never,
            ),
          );
          sidebar = undefined;
        } else if (routes) {
          sidebar = [...(options.sidebar ?? []), { label: "API Reference", items: reference as Sidebar }];
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
                        reference: source,
                        base,
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
        if (routes) {
          injectRoute({
            pattern: `/${base}/[...slug]`,
            entrypoint: here("../../astro/ReferencePage.astro"),
            prerender: true,
          });
        }
      },
    },
  };
}
