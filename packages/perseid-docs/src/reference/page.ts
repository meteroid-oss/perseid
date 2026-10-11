import { esc } from "./html.js";
import type { Reference } from "./model.js";
import {
  languageCss,
  modelId,
  renderGroup,
  renderIntro,
  renderLanguageSwitcher,
  renderModel,
  renderWebhook,
  renderSidebar,
  SEARCH_ICON,
  type RenderOptions,
} from "./render.js";

export interface PageOptions extends RenderOptions {
  /** The stylesheet, inlined. */
  css: string;
  /** The client script, inlined. */
  script: string;
  /** Extra `<head>` content: fonts, favicon, theme overrides. */
  head?: string;
  /** Rendered models; `false` leaves them out. */
  models?: boolean;
}

/** The search index the client's ⌘K dialog reads. */
export function searchIndex(ref: Reference): object[] {
  return [
    ...ref.groups.flatMap((g) =>
      g.operations.map((op) => ({
        slug: op.slug,
        title: op.title,
        group: g.title,
        method: op.method,
        path: op.path,
        sdk: Object.values(op.sdk).map((s) => s.method),
      })),
    ),
    ...(ref.webhooks ?? []).map((w) => ({ slug: w.slug, title: w.name, group: "Webhooks", method: w.method, path: w.description ?? "", sdk: [] })),
  ];
}

/** The reference's layout, for a page of its own or an element of another page: top bar,
 * sidebar, intro, groups, models, and the JSON the client script reads. */
export function renderLayout(ref: Reference, options: RenderOptions & { models?: boolean } = {}): string {
  const models = options.models === false ? [] : Object.values(ref.models);
  const index = JSON.stringify(searchIndex(ref)).replace(/</g, "\\u003c");
  const modelNav = models.length
    ? `<details class="pr-nav-group" data-group="models"><summary><a href="#models">Models</a></summary><ul>${models
        .map((m) => `<li><a class="pr-nav-op" href="#model-${esc(modelId(m.name))}"><span class="pr-nav-label">${esc(m.name)}</span></a></li>`)
        .join("")}</ul></details>`
    : "";
  const webhooks = ref.webhooks ?? [];
  const webhookNav = webhooks.length
    ? `<details class="pr-nav-group" data-group="webhooks"><summary><a href="#webhooks">Webhooks</a></summary><ul>${webhooks
        .map((w) => `<li><a class="pr-nav-op" href="#${esc(w.slug)}"><span class="pr-nav-label">${esc(w.name)}</span></a></li>`)
        .join("")}</ul></details>`
    : "";
  return `<div class="pr-root pr-page">
<header class="pr-topbar">
<a class="pr-brand" href="#introduction">${esc(ref.title)}</a>
<button class="pr-search-button" type="button">${SEARCH_ICON}<span>Search</span><kbd>⌘K</kbd></button>
<div class="pr-topbar-end">${renderLanguageSwitcher(ref)}<button class="pr-theme" type="button" aria-label="Toggle theme"></button></div>
</header>
<div class="pr-layout">
<aside class="pr-sidebar"><a class="pr-nav-top" href="#introduction">Introduction</a>${renderSidebar(ref, options)}${webhookNav}${modelNav}</aside>
<main class="pr-content">
${renderIntro(ref, options)}
${ref.groups.map((g) => renderGroup(g, ref, options)).join("\n")}
${webhooks.length ? `<section class="pr-models pr-webhooks" id="webhooks"><h2 class="pr-group-title">Webhooks</h2>${webhooks.map((w) => renderWebhook(w, ref, options)).join("\n")}</section>` : ""}
${models.length ? `<section class="pr-models" id="models"><h2 class="pr-group-title">Models</h2>${models.map((m) => renderModel(m, ref, options)).join("\n")}</section>` : ""}
</main>
</div>
</div>
<script type="application/json" id="pr-search-index">${index}</script>
<script type="application/json" id="pr-servers">${JSON.stringify(ref.servers.map((s) => s.url))}</script>`;
}

/** The whole reference as one HTML document, the way Scalar serves one. */
export function renderPage(ref: Reference, options: PageOptions): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(ref.title)} API reference</title>
<script>${PRE_PAINT}</script>
${options.head ?? ""}
<style>${options.css}
${languageCss(ref)}</style>
</head>
<body>
${renderLayout(ref, options)}
<script>${options.script}</script>
</body>
</html>
`;
}

/** Applies the reader's theme and language before the first paint. */
export const PRE_PAINT = `try{var t=localStorage.getItem("pr-theme");if(t)document.documentElement.dataset.theme=t;var l=localStorage.getItem("pr-lang");if(l)document.documentElement.dataset.prLang=l}catch(e){}`;
