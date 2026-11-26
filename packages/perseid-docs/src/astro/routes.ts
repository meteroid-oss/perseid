import { readFileSync } from "node:fs";
import {
  flatten,
  languageCss,
  renderGroup,
  renderIntro,
  renderLanguageSwitcher,
  renderModel,
  renderModelIndex,
  renderWebhook,
  renderWebhookIndex,
  renderOperation,
  type Group,
  type Highlighter,
  type Operation,
  type Reference,
  type RenderOptions,
  type Webhook,
} from "../reference/index.js";
import { shikiHighlighter } from "../highlight.js";
import { assemble, kebab } from "../build/reference.js";
import { parseSpec } from "../build/spec.js";
import type { DocsData } from "../build/data.js";

/** Where the reference comes from: a `reference.json` `perseid-docs build` wrote, or the spec
 * and docs data the orchestrator writes in the docs repository's `api/` folder. */
export type ReferenceSource = string | { spec: string; data?: string };

/** How the site's URLs look: Astro's `base` and `trailingSlash`. */
export interface UrlFormat {
  base: string;
  trailingSlash: "always" | "never" | "ignore";
}

const DEFAULT_FORMAT: UrlFormat = { base: "/", trailingSlash: "ignore" };

export interface ReferenceRoutes {
  reference: Reference;
  /** URL prefix of the reference, under the site's base: `api-reference`. */
  base: string;
  format: UrlFormat;
  /** Route slug of each group, by group id. */
  groupSlugs: Map<string, string>;
  /** Route path of each operation under the base, by operation slug. */
  opSlugs: Map<string, string>;
  /** Route path of each webhook under the base, by event name. */
  webhookSlugs: Map<string, string>;
}

export type Route =
  | { kind: "intro" }
  | { kind: "group"; group: string }
  | { kind: "operation"; group: string; op: string }
  | { kind: "models" }
  | { kind: "webhooks" }
  | { kind: "webhook"; webhook: string }
  | { kind: "model"; model: string };

/** The site URL of `path`, a path under the site's base. */
export function formatHref(path: string, format: UrlFormat = DEFAULT_FORMAT): string {
  const prefix = format.base.replace(/\/+$/, "");
  const clean = path.replace(/^\/+|\/+$/g, "");
  if (!clean) return format.trailingSlash === "never" ? prefix || "/" : `${prefix}/`;
  return `${prefix}/${clean}${format.trailingSlash === "never" ? "" : "/"}`;
}

/** The site URL of the Markdown twin of `path`. */
export function markdownHref(path: string, format: UrlFormat = DEFAULT_FORMAT): string {
  return `${format.base.replace(/\/+$/, "")}/${path.replace(/^\/+|\/+$/g, "") || "index"}.md`;
}

/** Mintlify's `prepareStringToBeValidFilename`: the file name, so the URL, of its page for an
 * operation or a webhook. Characters a URL path can't hold as they are are dropped. */
export function mintlifySlug(text: string | undefined): string | undefined {
  if (!text) return undefined;
  return text
    .replaceAll(" ", "-")
    .replace(/\{.*?\}/g, "-")
    .replace(/^-/, "")
    .replace(/-$/, "")
    .replace(/[{}(),.'\n/]/g, "")
    .replaceAll(/--/g, "-")
    .toLowerCase()
    .replace(/[?#%\\"<>|^`]/g, "");
}

const cache = new Map<string, ReferenceRoutes>();

export function readReference(source: ReferenceSource): Reference {
  return typeof source === "string"
    ? (JSON.parse(readFileSync(source, "utf8")) as Reference)
    : assemble(
        parseSpec(readFileSync(source.spec, "utf8"), source.spec),
        source.data ? (JSON.parse(readFileSync(source.data, "utf8")) as DocsData) : undefined,
      );
}

/** Routes are Mintlify's, so links to a Mintlify reference keep working: `/<base>/<tag>/<summary>`
 * for an operation, `<method>-<path>` without a summary, `/<base>/webhooks/<summary>` for a
 * webhook. */
export function loadRoutes(source: ReferenceSource, base: string, format: UrlFormat = DEFAULT_FORMAT): ReferenceRoutes {
  const key = `${JSON.stringify(source)}\0${base}\0${JSON.stringify(format)}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const reference = readReference(source);
  const groupSlugs = new Map<string, string>();
  const opSlugs = new Map<string, string>();
  const webhookSlugs = new Map<string, string>();
  const taken = new Set<string>(["models", "webhooks"]);
  const claim = (path: string, first: number) => {
    let slug = path;
    for (let i = first; taken.has(slug); i++) slug = `${path}-${i}`;
    taken.add(slug);
    return slug;
  };
  const byId = new Map(reference.groups.map((g) => [g.id, g]));
  const root = (g: Group): Group => (g.parent && byId.has(g.parent) ? root(byId.get(g.parent)!) : g);
  const roots = reference.groups.filter((g) => root(g) === g);
  for (const group of roots) groupSlugs.set(group.id, claim(mintlifySlug(group.title) || group.id, 2));
  // A docs model written before summaries were kept names operations by title.
  const summaries = reference.groups.some((g) => g.operations.some((op) => op.summary !== undefined));
  // A sub-resource's operations live under its root's URL, the tag Mintlify filed them under.
  for (const group of reference.groups) {
    const folder = groupSlugs.get(root(group).id)!;
    for (const op of group.operations) {
      const summary = summaries ? op.summary : op.title;
      const name = mintlifySlug(summary) || `${op.method.toLowerCase()}-${mintlifySlug(op.path)}`;
      opSlugs.set(op.slug, claim(`${folder}/${name}`, 1));
    }
  }
  for (const group of reference.groups.filter((g) => root(g) !== g)) {
    groupSlugs.set(group.id, claim(`${groupSlugs.get(root(group).id)}/${kebab(group.title) || group.id}`, 2));
  }
  for (const webhook of reference.webhooks ?? []) {
    webhookSlugs.set(webhook.name, claim(`webhooks/${mintlifySlug(webhook.summary) || mintlifySlug(webhook.name)}`, 1));
  }
  const routes = { reference, base: base.replace(/^\/|\/$/g, ""), format, groupSlugs, opSlugs, webhookSlugs };
  cache.set(key, routes);
  return routes;
}

/** Paths under the site's base, without slashes around them. */
export const introPath = (r: ReferenceRoutes) => r.base;
export const groupPath = (r: ReferenceRoutes, group: Group) => `${r.base}/${r.groupSlugs.get(group.id)}`;
export const operationPath = (r: ReferenceRoutes, op: Operation) => `${r.base}/${r.opSlugs.get(op.slug)}`;
export const webhookPath = (r: ReferenceRoutes, webhook: Webhook) => `${r.base}/${r.webhookSlugs.get(webhook.name)}`;
export const modelPath = (r: ReferenceRoutes, name: string) => `${r.base}/models/${encodeURIComponent(name)}`;

export function groupHref(r: ReferenceRoutes, group: Group): string {
  return formatHref(groupPath(r, group), r.format);
}

export function operationHref(r: ReferenceRoutes, op: Operation, _group?: Group): string {
  return formatHref(operationPath(r, op), r.format);
}

export function webhookHref(r: ReferenceRoutes, webhook: Webhook): string {
  return formatHref(webhookPath(r, webhook), r.format);
}

export function modelHref(r: ReferenceRoutes, name: string): string {
  return formatHref(modelPath(r, name), r.format);
}

export function staticPaths(r: ReferenceRoutes): { params: { slug: string | undefined }; props: { route: Route } }[] {
  const out: { params: { slug: string | undefined }; props: { route: Route } }[] = [
    { params: { slug: undefined }, props: { route: { kind: "intro" } } },
  ];
  for (const group of r.reference.groups) {
    const g = r.groupSlugs.get(group.id)!;
    out.push({ params: { slug: g }, props: { route: { kind: "group", group: group.id } } });
    for (const op of group.operations) {
      out.push({
        params: { slug: r.opSlugs.get(op.slug)! },
        props: { route: { kind: "operation", group: group.id, op: op.slug } },
      });
    }
  }
  const webhooks = r.reference.webhooks ?? [];
  if (webhooks.length) out.push({ params: { slug: "webhooks" }, props: { route: { kind: "webhooks" } } });
  for (const w of webhooks) {
    out.push({ params: { slug: r.webhookSlugs.get(w.name)! }, props: { route: { kind: "webhook", webhook: w.name } } });
  }
  if (Object.keys(r.reference.models).length) {
    out.push({ params: { slug: "models" }, props: { route: { kind: "models" } } });
  }
  for (const name of Object.keys(r.reference.models)) {
    out.push({ params: { slug: `models/${name}` }, props: { route: { kind: "model", model: name } } });
  }
  return out;
}

const METHOD_VARIANT: Record<string, string> = {
  get: "note",
  post: "success",
  put: "caution",
  patch: "caution",
  delete: "danger",
};

/** The reference's Starlight sidebar: one collapsed group per resource, methods as badges. */
function sidebarGroup(r: ReferenceRoutes, group: Group): object {
  return {
    label: group.title,
    collapsed: true,
    items: [
      ...group.operations.map((op) => ({
        label: op.title,
        link: `/${operationPath(r, op)}`,
        badge: { text: op.method === "delete" ? "DEL" : op.method.toUpperCase(), variant: METHOD_VARIANT[op.method] ?? "default" },
        attrs: { class: `pd-op pd-op-${op.method}` },
      })),
      ...r.reference.groups.filter((g) => g.parent === group.id).map((g) => sidebarGroup(r, g)),
    ],
  };
}

export function sidebar(r: ReferenceRoutes): object[] {
  return [
    { label: "Introduction", link: `/${introPath(r)}` },
    ...r.reference.groups.filter((g) => !g.parent).map((group) => sidebarGroup(r, group)),
    ...(r.reference.webhooks?.length
      ? [{
          label: "Webhooks",
          collapsed: true,
          items: [
            { label: "All events", link: `/${r.base}/webhooks` },
            ...r.reference.webhooks.map((w) => ({ label: w.name, link: `/${webhookPath(r, w)}`, attrs: { class: "pd-op pd-webhook" } })),
          ],
        }]
      : []),
    ...(Object.keys(r.reference.models).length ? [{ label: "Models", link: `/${r.base}/models` }] : []),
  ];
}

let highlighter: Promise<Highlighter> | undefined;
let assets: { css: string; script: string } | undefined;

/** The viewer's stylesheet and script, at paths the integration resolved: this module runs bundled. */
export interface AssetPaths {
  css: string;
  script: string;
}

function referenceAssets(paths: AssetPaths) {
  assets ??= { css: readFileSync(paths.css, "utf8"), script: readFileSync(paths.script, "utf8") };
  return assets;
}

export interface RenderedRoute {
  title: string;
  description?: string;
  html: string;
  /** The stylesheet of the reference and the rules hiding unselected languages. */
  css: string;
  script: string;
  toolbar: string;
  /** The servers try-it sends requests to, as JSON. */
  servers: string;
}

export async function renderRoute(r: ReferenceRoutes, route: Route, paths: AssetPaths): Promise<RenderedRoute> {
  highlighter ??= shikiHighlighter();
  const options: RenderOptions = {
    highlight: await highlighter,
    modelHref: (name) => modelHref(r, name),
    operationHref: (op, group) => operationHref(r, op, group),
    groupHref: (group) => groupHref(r, group),
    webhookHref: (webhook) => webhookHref(r, webhook),
  };
  const { reference } = r;
  const { css, script } = referenceAssets(paths);
  const common = {
    css: `${css}\n${languageCss(reference)}`,
    script,
    toolbar: renderLanguageSwitcher(reference),
    servers: JSON.stringify(reference.servers.map((s) => s.url)),
  };
  switch (route.kind) {
    case "intro":
      return { ...common, title: `${reference.title} API`, html: renderIntro(reference, options) };
    case "group": {
      const group = reference.groups.find((g) => g.id === route.group)!;
      const html = renderGroup({ ...group, operations: [] }, reference, options).replace(
        /<div class="pr-card-body pr-endpoint-list">/,
        `<div class="pr-card-body pr-endpoint-list">${group.operations
          .map((op) => `<a class="pr-endpoint" href="${operationHref(r, op, group)}"><span class="pr-method pr-m-${op.method}">${op.method.toUpperCase()}</span><span class="pd-endpoint-title">${escapeHtml(op.title)}</span><code>${escapeHtml(op.path)}</code></a>`)
          .join("")}`,
      );
      const overview = `<div class="pd-group-ops">${group.operations
        .map((op) => `<a class="pd-group-op" href="${operationHref(r, op, group)}"><span class="pd-group-op-title">${escapeHtml(op.title)}</span>${op.description ? `<span class="pd-group-op-desc">${escapeHtml(firstSentence(op.description))}</span>` : ""}</a>`)
        .join("")}</div>`;
      const withOverview = html.replace(/(<div class="pr-group-intro">[\s\S]*?)(<\/div>\n<div class="pr-card pr-endpoints")/, `$1${overview}$2`);
      return { ...common, title: group.title, ...(group.description ? { description: firstSentence(group.description) } : {}), html: withOverview };
    }
    case "operation": {
      const group = reference.groups.find((g) => g.id === route.group)!;
      const op = group.operations.find((o) => o.slug === route.op)!;
      return {
        ...common,
        title: op.title,
        ...(op.description ? { description: firstSentence(op.description) } : {}),
        html: renderOperation(op, group, reference, options),
      };
    }
    case "webhooks":
      return { ...common, title: "Webhooks", description: `The events the ${reference.title} API sends to your endpoint.`, html: renderWebhookIndex(reference, options) };
    case "webhook": {
      const webhook = reference.webhooks!.find((w) => w.name === route.webhook)!;
      return {
        ...common,
        title: webhook.name,
        ...(webhook.description ? { description: firstSentence(webhook.description) } : {}),
        html: renderWebhook(webhook, reference, options),
      };
    }
    case "models":
      return { ...common, title: "Models", description: `The objects the ${reference.title} API takes and returns.`, html: renderModelIndex(reference, options) };
    case "model": {
      const model = reference.models[route.model]!;
      return { ...common, title: model.name, html: renderModel(model, reference, options) };
    }
  }
}

function firstSentence(text: string): string {
  return text.split(/\n\n|(?<=\.)\s/)[0]!.replace(/[`*_]/g, "").slice(0, 200);
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

export interface MarkdownPage {
  /** The page's URL path without slashes around it: `.md` appended gives the Markdown's. */
  path: string;
  title: string;
  description?: string;
  text: string;
}

/** The reference's pages as Markdown: one per operation and per group, and the introduction. */
export function markdownPages(r: ReferenceRoutes): MarkdownPage[] {
  const { reference } = r;
  const pages: MarkdownPage[] = [];
  const sdk = reference.languages.filter((l) => l.id !== "curl");
  pages.push({
    path: r.base,
    title: `${reference.title} API`,
    text: [
      `# ${reference.title} API`,
      reference.description ?? "",
      `Base URL: \`${reference.servers[0]?.url ?? ""}\``,
      ...sdk.map((l) => `## ${l.label}${l.version ? ` (${l.version})` : ""}\n\n\`\`\`sh\n${l.install ?? ""}\n\`\`\`\n\n\`\`\`${l.grammar}\n${l.setup ?? ""}\n\`\`\``),
    ]
      .filter(Boolean)
      .join("\n\n"),
  });
  for (const group of reference.groups) {
    pages.push({
      path: groupPath(r, group),
      title: group.title,
      ...(group.description ? { description: firstSentence(group.description) } : {}),
      text: [
        `# ${group.title}`,
        group.description ?? "",
        group.operations.map((op) => `- [${op.title}](${markdownHref(operationPath(r, op), r.format)}): \`${op.method.toUpperCase()} ${op.path}\``).join("\n"),
      ]
        .filter(Boolean)
        .join("\n\n"),
    });
    for (const op of group.operations) {
      pages.push({
        path: operationPath(r, op),
        title: op.title,
        ...(op.description ? { description: firstSentence(op.description) } : {}),
        text: operationMarkdown(r, op),
      });
    }
  }
  const webhooks = reference.webhooks ?? [];
  if (webhooks.length) {
    pages.push({
      path: `${r.base}/webhooks`,
      title: "Webhooks",
      text: [
        "# Webhooks",
        ...webhooks.map((w) => {
          const schema = w.body?.schema.ref ? w.body.schema.ref : w.body ? mdType(w.body.schema) : "";
          return [`## ${w.name}`, w.description ?? "", schema ? `Payload: ${schema}` : ""].filter(Boolean).join("\n\n");
        }),
      ].join("\n\n"),
    });
  }
  const models = Object.values(reference.models).sort((a, b) => a.name.localeCompare(b.name));
  if (models.length) {
    pages.push({
      path: `${r.base}/models`,
      title: "Models",
      text: ["# Models", ...models.map((m) => modelMarkdown({ ...m, schema: flatten(m.schema, reference.models) }))].join("\n\n"),
    });
  }
  return pages;
}

function modelMarkdown(model: import("../reference/index.js").Model): string {
  const props = Object.entries(model.schema.properties ?? {}).map(
    ([name, prop]) => `- \`${name}\`${model.schema.required?.includes(name) ? " (required)" : ""}: ${mdType(prop)}${prop.description ? ` — ${firstSentence(prop.description)}` : ""}`,
  );
  return [`## ${model.name}`, model.description ?? "", props.length ? props.join("\n") : `Type: ${mdType(model.schema)}`]
    .filter(Boolean)
    .join("\n\n");
}

function operationMarkdown(r: ReferenceRoutes, op: Operation): string {
  const { reference } = r;
  const out = [`# ${op.title}`, `\`${op.method.toUpperCase()} ${reference.servers[0]?.url ?? ""}${op.path}\``];
  if (op.description) out.push(op.description);
  const where: [string, string][] = [["path", "Path parameters"], ["query", "Query parameters"], ["header", "Headers"]];
  for (const [kind, title] of where) {
    const params = op.params.filter((p) => p.in === kind);
    if (params.length) {
      out.push(`## ${title}\n\n${params.map((p) => `- \`${p.name}\` (${mdType(p.schema)}${p.required ? ", required" : ""})${p.description ? `: ${oneLine(p.description)}` : ""}`).join("\n")}`);
    }
  }
  if (op.body) {
    const schema = op.body.schema.ref ? reference.models[op.body.schema.ref]?.schema : op.body.schema;
    const required = new Set(schema?.required ?? []);
    const props = Object.entries(schema?.properties ?? {});
    out.push(
      `## Body (\`${op.body.contentType}\`)${op.body.schema.ref ? `: ${op.body.schema.ref}` : ""}\n\n${props
        .map(([name, s]) => `- \`${name}\` (${mdType(s)}${required.has(name) ? ", required" : ""})${s.description ? `: ${oneLine(s.description)}` : ""}`)
        .join("\n")}`,
    );
  }
  for (const resp of op.responses) {
    const type = resp.schema ? `: ${mdType(resp.schema)}` : "";
    out.push(`## Response ${resp.status}${type}\n\n${resp.description ?? ""}${resp.example !== undefined && resp.status.startsWith("2") ? `\n\n\`\`\`json\n${JSON.stringify(resp.example, null, 2)}\n\`\`\`` : ""}`);
  }
  const samples = reference.languages
    .filter((l) => op.samples[l.id])
    .map((l) => `### ${l.label}\n\n${op.sdk[l.id]?.signature ? `\`${op.sdk[l.id]!.signature!.replace(/\s*\n\s*/g, " ")}\`\n\n` : ""}\`\`\`${l.grammar}\n${op.samples[l.id]}\n\`\`\``);
  if (samples.length) out.push(`## Examples\n\n${samples.join("\n\n")}`);
  return `${out.join("\n\n")}\n`;
}

function mdType(s: import("../reference/index.js").Schema): string {
  if (s.ref) return s.ref;
  if (s.type === "array" && s.items) return `array of ${mdType(s.items)}`;
  if (s.enum?.length) return `enum: ${s.enum.slice(0, 12).map((v) => String(v)).join(" | ")}${s.enum.length > 12 ? " | …" : ""}`;
  const variants = s.oneOf ?? s.anyOf;
  if (variants?.length) return variants.map(mdType).join(" | ");
  const t = Array.isArray(s.type) ? s.type.join(" | ") : (s.type ?? "any");
  return `${t}${s.format ? ` (${s.format})` : ""}${s.nullable ? ", nullable" : ""}`;
}

function oneLine(text: string): string {
  return text.replace(/\s*\n\s*/g, " ").trim();
}

/** A reference of the site, as the integration resolved it. */
export interface SiteReference {
  /** `""` for the main reference. */
  id: string;
  label: string;
  source: ReferenceSource;
  base: string;
  /** The banner of a legacy reference's pages, as HTML. */
  banner?: string;
}

/** The reference a route pattern serves: each is injected as `/<base>/[...slug]`. */
export function referenceOf(references: SiteReference[], pattern: string): SiteReference {
  const found = references.find((r) => pattern.endsWith(`/${r.base}/[...slug]`));
  if (!found) throw new Error(`perseid-docs: no reference serves ${pattern}`);
  return found;
}
