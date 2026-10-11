import { attrs, esc, md, plain, type Highlighter } from "./html.js";
import type { Group, Language, Model, Operation, Param, Reference, Response, Schema, Webhook } from "./model.js";
import { flatten, names, properties, row, type SchemaContext } from "./schema.js";

export interface RenderOptions {
  highlight?: Highlighter;
  /** URL of a model's section: `#model-<name>` on the same page by default. */
  modelHref?: (name: string) => string;
  /** URL of an operation: `#<slug>` by default. */
  operationHref?: (op: Operation, group: Group) => string;
  /** URL of a group: `#<id>` by default. */
  groupHref?: (group: Group) => string;
  /** URL of a webhook: `#<slug>` by default. */
  webhookHref?: (webhook: Webhook) => string;
}

interface Ctx extends SchemaContext {
  highlight: Highlighter;
  sdkLanguages: Language[];
  operationHref: (op: Operation, group: Group) => string;
  groupHref: (group: Group) => string;
  webhookHref: (webhook: Webhook) => string;
}

function context(ref: Reference, options: RenderOptions): Ctx {
  const sdkLanguages = ref.languages.filter((l) => l.id !== "curl");
  return {
    ref,
    languages: sdkLanguages.map((l) => l.id),
    sdkLanguages,
    highlight: options.highlight ?? plain,
    modelHref: options.modelHref ?? ((name) => `#model-${modelId(name)}`),
    operationHref: options.operationHref ?? ((op) => `#${op.slug}`),
    groupHref: options.groupHref ?? ((group) => `#${group.id}`),
    webhookHref: options.webhookHref ?? ((webhook) => `#${webhook.slug}`),
  };
}

export function modelId(name: string): string {
  return name.replace(/[^A-Za-z0-9_-]+/g, "-");
}

/** Signatures longer than this many characters are folded behind a "Show all" button. */
const CLAMP_CHARS = 280;

/** Left out of search indexes: every language but the first repeats the same names. */
export const IGNORE = " data-pagefind-ignore";

const METHOD_LABEL: Record<string, string> = { delete: "DEL", options: "OPT" };

export function method(m: string, short = false): string {
  const label = short ? (METHOD_LABEL[m] ?? m.toUpperCase()) : m.toUpperCase();
  return `<span class="pr-method pr-m-${esc(m)}">${esc(label)}</span>`;
}

/** The path with its parameters marked. */
function path(p: string): string {
  return esc(p).replace(/\{([^}]+)\}/g, '<span class="pr-path-param">{$1}</span>');
}

function code(source: string, grammar: string, ctx: Ctx, extra: Record<string, string> = {}): string {
  return `<pre class="pr-code"${attrs(extra)}><code class="language-${esc(grammar)}">${ctx.highlight(source, grammar)}</code></pre>`;
}

/** One block per language, each shown when that language is selected. */
function perLanguage(render: (lang: Language) => string | undefined, ctx: Ctx, curl?: string): string {
  const parts = ctx.sdkLanguages.map((l, i) => {
    const html = render(l);
    return html ? `<div data-l="${l.id}"${i ? IGNORE : ""}>${html}</div>` : "";
  });
  if (curl) parts.push(`<div data-l="curl"${IGNORE}>${curl}</div>`);
  return parts.join("");
}

function paramRows(params: Param[], op: Operation, ctx: Ctx): string {
  return `<ul class="pr-props">${params
    .map((p) => {
      const perLang = Object.fromEntries(ctx.languages.map((l) => [l, op.sdk[l]?.params?.[p.name]]));
      return row(names(p.name, perLang, ctx), { ...p.schema, ...(p.description ? { description: p.description } : {}), ...(p.deprecated ? { deprecated: true } : {}) }, p.required, ctx);
    })
    .join("")}</ul>`;
}

const PARAM_SECTIONS: [Param["in"], string][] = [
  ["path", "Path parameters"],
  ["query", "Query parameters"],
  ["header", "Headers"],
  ["cookie", "Cookies"],
];

function responseSummary(r: Response, ctx: Ctx): string {
  const schema = r.schema;
  const ok = r.status.startsWith("2");
  const body = schema ? properties(schema, ctx) : "";
  const type = schema?.ref
    ? `<a class="pr-type-ref" href="${esc(ctx.modelHref(schema.ref))}">${esc(schema.ref)}</a>`
    : schema?.type === "array" && schema.items?.ref
      ? `array of <a class="pr-type-ref" href="${esc(ctx.modelHref(schema.items.ref))}">${esc(schema.items.ref)}</a>`
      : "";
  const head = `<span class="pr-status pr-status-${esc(r.status[0])}">${esc(r.status)}</span><span class="pr-response-desc">${esc(r.description ?? "")}</span>${type ? `<span class="pr-type">${type}</span>` : ""}`;
  return body
    ? `<details class="pr-response"${ok ? " open" : ""}><summary>${head}</summary>${body}</details>`
    : `<div class="pr-response pr-response-leaf">${head}</div>`;
}

function opData(op: Operation, ref: Reference): string {
  const example = (p: Param) => p.schema.example ?? (p.schema.ref ? ref.models[p.schema.ref]?.schema.example : undefined);
  const data = {
    title: op.title,
    method: op.method,
    path: op.path,
    params: op.params.map((p) => {
      const value = example(p);
      const type = p.schema.ref ?? (p.schema.enum ? p.schema.enum.join(" | ") : (p.schema.format ?? [p.schema.type].flat().join(" | ")));
      return { name: p.name, in: p.in, required: p.required, type, ...(value !== undefined ? { example: String(value) } : {}) };
    }),
    ...(op.body ? { contentType: op.body.contentType, body: op.body.example } : {}),
    auth: ref.auth.filter((a) => op.auth.includes(a.id)).map(({ kind, in: where, name }) => ({ kind, in: where, name })),
  };
  return `<script type="application/json" class="pr-op-data">${JSON.stringify(data).replace(/</g, "\\u003c")}</script>`;
}

export function renderOperation(op: Operation, group: Group, ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const signature = perLanguage(
    (l) => {
      const sig = op.sdk[l.id]?.signature;
      if (!sig) return undefined;
      const code = `<code class="language-${esc(l.grammar)}">${ctx.highlight(sig, l.grammar)}</code>`;
      return sig.length > CLAMP_CHARS
        ? `<div class="pr-clamp">${code}<button class="pr-unclamp" type="button">Show the whole signature</button></div>`
        : code;
    },
    ctx,
    `<code>${esc(op.method.toUpperCase())} ${esc(op.path)}</code>`,
  );
  const sections: string[] = [];
  for (const [where, title] of PARAM_SECTIONS) {
    const params = op.params.filter((p) => p.in === where);
    if (params.length) sections.push(`<h4 class="pr-section-title">${title}</h4>${paramRows(params, op, ctx)}`);
  }
  if (op.body) {
    const body = properties(op.body.schema, ctx) || `<ul class="pr-props">${row("body", op.body.schema, op.body.required, ctx)}</ul>`;
    sections.push(
      `<h4 class="pr-section-title">Body <span class="pr-content-type">${esc(op.body.contentType)}</span></h4>${op.body.description ? `<div class="pr-desc">${md(op.body.description)}</div>` : ""}${body}`,
    );
  }
  const ok = op.responses.filter((r) => r.status.startsWith("2") || r.status === "default");
  const errors = op.responses.filter((r) => !ok.includes(r));
  if (ok.length) sections.push(`<h4 class="pr-section-title">Returns</h4>${ok.map((r) => responseSummary(r, ctx)).join("")}`);
  if (errors.length) {
    sections.push(
      `<h4 class="pr-section-title">Errors</h4><div class="pr-errors">${errors
        .map((r) => `<div class="pr-error"><span class="pr-status pr-status-${esc(r.status[0])}">${esc(r.status)}</span><span>${esc(r.description ?? "")}</span></div>`)
        .join("")}</div>`,
    );
  }

  const samples = perLanguage(
    (l) => (op.samples[l.id] ? code(op.samples[l.id]!, l.grammar, ctx, l.setup ? { "data-setup": l.setup } : {}) : undefined),
    ctx,
    op.samples.curl ? code(op.samples.curl, "bash", ctx) : undefined,
  );
  const examples = op.responses.filter((r) => r.example !== undefined);
  const responseCard = examples.length
    ? `<div class="pr-card pr-examples">
<div class="pr-card-head"><div class="pr-tabs" role="tablist">${examples
        .map((r, i) => `<button class="pr-tab" role="tab" data-tab="${esc(r.status)}" aria-selected="${i === 0}">${esc(r.status)}</button>`)
        .join("")}</div><button class="pr-copy" type="button" aria-label="Copy">${COPY_ICON}</button></div>
<div class="pr-card-body">${examples.map((r, i) => code(JSON.stringify(r.example, null, 2), "json", ctx, { "data-panel": r.status, ...(i ? { hidden: "" } : {}) })).join("")}</div>
</div>`
    : "";

  const badges = `${op.deprecated ? `<span class="pr-badge pr-deprecated">deprecated</span>` : ""}${op.auth.length ? "" : `<span class="pr-badge">public</span>`}`;
  return `<section class="pr-op" id="${esc(op.slug)}" data-group="${esc(group.id)}">
<div class="pr-op-main">
<div class="pr-op-head"><p class="pr-crumb">${esc(groupPath(group, ref))}</p><h3 class="pr-op-title"><a href="${esc(ctx.operationHref(op, group))}">${esc(op.title)}</a></h3></div>
<div class="pr-endpoint-bar">${method(op.method)}<code class="pr-path">${path(op.path)}</code>${badges}<button class="pr-try pr-m-${esc(op.method)}" type="button">Try it${PLAY_ICON}</button></div>
${Object.values(op.sdk).some((s) => s?.signature) ? `<div class="pr-card pr-sdk"${IGNORE}><div class="pr-card-head"><span class="pr-card-title">Method</span>${languageSelect(ctx)}</div><div class="pr-card-body pr-signature">${signature}</div></div>` : ""}
${op.description ? `<div class="pr-desc">${md(op.description)}</div>` : ""}
${authSection(op, ctx)}${sections.join("\n")}
</div>
<aside class="pr-op-side"${IGNORE}>
<div class="pr-card pr-request">
<div class="pr-card-head"><span class="pr-card-title">${esc(op.title)}</span>${languageSelect(ctx)}<button class="pr-copy" type="button" aria-label="Copy">${COPY_ICON}</button></div>
<div class="pr-card-body">${samples}</div>
</div>
${responseCard}
</aside>
${opData(op, ref)}
</section>`;
}

/** An event the API sends: its payload's fields and type in each SDK, and an example. */
export function renderWebhook(webhook: Webhook, ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const body = webhook.body;
  const model = body?.schema.ref ? ref.models[body.schema.ref] : undefined;
  const typeNames = model ? perLanguage((l) => (model.sdk[l.id] ? `<code>${esc(model.sdk[l.id]!.name)}</code>` : undefined), ctx) : "";
  const typeCard = typeNames
    ? `<div class="pr-card pr-sdk"${IGNORE}><div class="pr-card-head"><span class="pr-card-title">Payload type</span>${languageSelect(ctx)}</div><div class="pr-card-body pr-signature">${typeNames}</div></div>`
    : "";
  const payload = body
    ? `<h4 class="pr-section-title">Payload <span class="pr-content-type">${esc(body.contentType)}</span></h4>${properties(body.schema, ctx) || `<ul class="pr-props">${row("body", body.schema, true, ctx)}</ul>`}`
    : "";
  const example = body?.example !== undefined
    ? `<div class="pr-card pr-request"><div class="pr-card-head"><span class="pr-card-title">Example payload</span><button class="pr-copy" type="button" aria-label="Copy">${COPY_ICON}</button></div><div class="pr-card-body">${code(JSON.stringify(body.example, null, 2), "json", ctx)}</div></div>`
    : "";
  return `<section class="pr-op pr-webhook" id="${esc(webhook.slug)}" data-group="webhooks">
<div class="pr-op-main">
<div class="pr-op-head"><p class="pr-crumb">Webhooks</p><h3 class="pr-op-title"><a href="${esc(ctx.webhookHref(webhook))}">${esc(webhook.name)}</a></h3></div>
<div class="pr-endpoint-bar">${method(webhook.method)}<code class="pr-path">Your webhook endpoint</code><span class="pr-badge">event</span></div>
${webhook.description ? `<div class="pr-desc">${md(webhook.description)}</div>` : ""}
${typeCard}
${payload}
</div>
<aside class="pr-op-side"${IGNORE}>${example}</aside>
</section>`;
}

/** Every webhook, under the resource its name starts with: `invoice.paid` under Invoice. */
export function renderWebhookIndex(ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const byPrefix = new Map<string, Webhook[]>();
  for (const w of ref.webhooks ?? []) {
    const prefix = w.name.split(/[.:/]/)[0] ?? w.name;
    byPrefix.set(prefix, [...(byPrefix.get(prefix) ?? []), w]);
  }
  const title = (prefix: string) => prefix.replace(/[_-]+/g, " ").replace(/^./, (c) => c.toUpperCase());
  return `<section class="pr-model-index pr-webhook-index" id="webhooks">
<h1 class="pr-title">Webhooks</h1>
<p class="pr-model-index-intro">The events ${esc(ref.title)} sends to your endpoint, each as a request with a JSON payload.</p>
${[...byPrefix]
  .map(([prefix, hooks]) => `<h4 class="pr-section-title">${esc(title(prefix))} <span class="pr-count">${hooks.length}</span></h4><div class="pr-model-grid">${hooks
    .map((w) => `<a class="pr-model-link" href="${esc(ctx.webhookHref(w))}"><span class="pr-model-name"><code>${esc(w.name)}</code></span>${w.description ? `<span class="pr-model-summary">${esc(w.description.split(/\n\n|(?<=\.)\s/)[0]!.replace(/[`*_]/g, ""))}</span>` : ""}</a>`)
    .join("")}</div>`)
  .join("\n")}
</section>`;
}

/** Mintlify's "Authorizations": the header each scheme the operation accepts reads. */
function authSection(op: Operation, ctx: Ctx): string {
  const schemes = ctx.ref.auth.filter((a) => op.auth.includes(a.id));
  if (!schemes.length) return "";
  const rows = schemes.map((a) => {
    const [name, where, how] =
      a.kind === "apiKey"
        ? [a.name ?? "key", a.in ?? "header", `API key in the <code>${esc(a.name)}</code> ${esc(a.in)}.`]
        : a.kind === "basic"
          ? ["Authorization", "header", "Basic authentication header of the form <code>Basic &lt;credentials&gt;</code>."]
          : ["Authorization", "header", "Bearer authentication header of the form <code>Bearer &lt;token&gt;</code>, where <code>&lt;token&gt;</code> is your API key."];
    return `<li class="pr-prop"><div class="pr-prop-head"><code class="pr-prop-name">${esc(name)}</code><span class="pr-type">string</span><span class="pr-badge pr-where">${esc(where)}</span><span class="pr-badge pr-required">required</span></div><div class="pr-prop-desc"><p>${how}</p></div></li>`;
  });
  return `<h4 class="pr-section-title">Authorization</h4><ul class="pr-props">${rows.join("")}</ul>\n`;
}

/** `Add-ons › Entitlements` for a sub-resource. */
function groupPath(group: Group, ref: Reference): string {
  const parent = group.parent ? ref.groups.find((g) => g.id === group.parent) : undefined;
  return parent ? `${groupPath(parent, ref)} › ${group.title}` : group.title;
}

/** The language picker of a code card, kept in step with every other one on the page. */
function languageSelect(ctx: Ctx): string {
  const options = ctx.ref.languages.map((l) => `<option value="${esc(l.id)}">${esc(l.label)}</option>`).join("");
  return `<label class="pr-lang-pick"><span class="pr-sr">Language</span><select class="pr-lang-select">${options}</select></label>`;
}

export function renderGroup(group: Group, ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const list = group.operations
    .map((op) => `<a class="pr-endpoint" href="${esc(ctx.operationHref(op, group))}">${method(op.method)}<code>${path(op.path)}</code></a>`)
    .join("");
  return `<section class="pr-group" id="${esc(group.id)}">
<div class="pr-group-head">
<div class="pr-group-intro"><h2 class="pr-group-title"><a href="${esc(ctx.groupHref(group))}">${esc(group.title)}</a></h2>${group.description ? `<div class="pr-desc">${md(group.description)}</div>` : ""}</div>
<div class="pr-card pr-endpoints"${IGNORE}><div class="pr-card-head"><span class="pr-card-title">Endpoints</span></div><div class="pr-card-body pr-endpoint-list">${list}</div></div>
</div>
${group.operations.map((op) => renderOperation(op, group, ref, options)).join("\n")}
</section>`;
}

export function renderModel(model: Model, ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const names = perLanguage((l) => (model.sdk[l.id] ? `<code>${esc(model.sdk[l.id]!.name)}</code>` : undefined), ctx, `<code>${esc(model.name)}</code>`);
  const props = properties({ ref: model.name }, ctx) || `<ul class="pr-props">${row(esc(model.name), model.schema, false, ctx)}</ul>`;
  return `<section class="pr-model" id="model-${esc(modelId(model.name))}">
<h3 class="pr-model-title">${esc(model.name)}</h3>
<div class="pr-signature">${names}</div>
${model.description ? `<div class="pr-desc">${md(model.description)}</div>` : ""}
${props}
</section>`;
}

/** The named schemas `schema` reaches, through properties, items and variants. */
export function reachedModels(schema: Schema | undefined, models: Record<string, Model>, out = new Set<string>()): Set<string> {
  if (!schema) return out;
  if (schema.ref) {
    if (!out.has(schema.ref)) {
      out.add(schema.ref);
      reachedModels(models[schema.ref]?.schema, models, out);
    }
    return out;
  }
  const nested = [
    schema.items,
    typeof schema.additionalProperties === "object" ? schema.additionalProperties : undefined,
    ...Object.values(schema.properties ?? {}),
    ...(schema.oneOf ?? []),
    ...(schema.anyOf ?? []),
    ...(schema.allOf ?? []),
  ];
  for (const child of nested) reachedModels(child, models, out);
  return out;
}

function shape(schema: Schema): string {
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  if (schema.enum?.length) return `enum · ${plural(schema.enum.length, "value")}`;
  const variants = schema.oneOf ?? schema.anyOf;
  if (variants?.length) return `union · ${plural(variants.length, "variant")}`;
  if (schema.properties) return plural(Object.keys(schema.properties).length, "property").replace("propertys", "properties");
  if (schema.type === "array") return "list";
  return Array.isArray(schema.type) ? schema.type.join(" | ") : (schema.format ?? schema.type ?? "any");
}

/** Every model, under the one group whose operations use it, or under "Shared". */
export function renderModelIndex(ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const users = new Map<string, Set<string>>();
  for (const group of ref.groups) {
    for (const op of group.operations) {
      const schemas = [...op.params.map((p) => p.schema), op.body?.schema, ...op.responses.map((r) => r.schema)];
      const reached = new Set<string>();
      for (const schema of schemas) reachedModels(schema, ref.models, reached);
      for (const name of reached) users.set(name, (users.get(name) ?? new Set()).add(group.id));
    }
  }
  const sections = [
    ...ref.groups.map((g) => ({ title: g.title, owns: (n: string) => users.get(n)?.size === 1 && users.get(n)!.has(g.id) })),
    { title: "Shared", owns: (n: string) => (users.get(n)?.size ?? 0) !== 1 },
  ];
  const names = Object.keys(ref.models).sort((a, b) => a.localeCompare(b));
  const entry = (model: Model) => {
    const sdkNames = ctx.sdkLanguages
      .map((l, i) => `<code data-l="${l.id}"${i ? IGNORE : ""}>${esc(model.sdk[l.id]?.name ?? model.name)}</code>`)
      .join("");
    const summary = model.description?.split(/\n\n|(?<=\.)\s/)[0];
    return `<a class="pr-model-link" href="${esc(ctx.modelHref(model.name))}"><span class="pr-model-name">${sdkNames}<code data-l="curl">${esc(model.name)}</code><span class="pr-model-shape">${esc(shape(flatten(model.schema, ref.models)))}</span></span>${summary ? `<span class="pr-model-summary">${esc(summary.replace(/[`*_]/g, ""))}</span>` : ""}</a>`;
  };
  return `<section class="pr-model-index" id="models">
<h1 class="pr-title">Models</h1>
<p class="pr-model-index-intro">The objects the API takes and returns, under the resource using them, named as your SDK names them.</p>
${sections
  .map((section) => {
    const owned = names.filter(section.owns);
    return owned.length
      ? `<h4 class="pr-section-title">${esc(section.title)} <span class="pr-count">${owned.length}</span></h4><div class="pr-model-grid">${owned.map((n) => entry(ref.models[n]!)).join("")}</div>`
      : "";
  })
  .join("\n")}
</section>`;
}

/** Where the reader sets, once, the server, key and headers every "Try it" sends. */
function tryItSettings(ref: Reference): string {
  const key = ref.auth.length
    ? `<label class="pr-client-row"><span class="pr-client-label">API key</span><span class="pr-client-secret"><input name="__key" type="password" placeholder="Paste a test-mode key" autocomplete="off" spellcheck="false"><button type="button" class="pr-client-reveal" aria-label="Show the key" aria-pressed="false">${EYE_ICON}</button></span></label>
<label class="pr-client-row pr-client-toggle"><span class="pr-client-label">Remember on this device</span><input type="checkbox" role="switch" name="__remember"></label>`
    : "";
  return `<h4 class="pr-section-title">Try it settings</h4>
<form class="pr-settings"${IGNORE}><div class="pr-client-rows">
<label class="pr-client-row"><span class="pr-client-label">Server</span><input name="__server" value="${esc(ref.servers[0]?.url ?? "")}" autocomplete="off" spellcheck="false"></label>
${key}
<label class="pr-client-row pr-client-row-wide"><span class="pr-client-label">Headers<span class="pr-client-type">one per line</span></span><textarea name="__headers" rows="2" placeholder="Idempotency-Key: abc" spellcheck="false"></textarea></label>
</div><p class="pr-client-note"></p></form>`;
}

const EYE_ICON = `<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8Z"/><circle cx="8" cy="8" r="2"/></svg>`;

export function renderIntro(ref: Reference, options: RenderOptions = {}): string {
  const ctx = context(ref, options);
  const server = ref.servers[0]?.url;
  const auth = ref.auth
    .map((a) => {
      const how =
        a.kind === "bearer" ? "Bearer token in the <code>Authorization</code> header"
        : a.kind === "basic" ? "HTTP basic authentication"
        : a.kind === "apiKey" ? `API key in the <code>${esc(a.name)}</code> ${esc(a.in)}`
        : "OAuth 2";
      return `<div class="pr-auth"><code>${esc(a.id)}</code><span>${how}</span></div>${a.description ? `<div class="pr-desc">${md(a.description)}</div>` : ""}`;
    })
    .join("");
  const clients = perLanguage(
    (l) =>
      [
        l.version ? `<p class="pr-sdk-version">Latest release <code>${esc(l.version)}</code></p>` : "",
        l.install ? code(l.install, "bash", ctx) : "",
        l.setup ? code(l.setup, l.grammar, ctx) : "",
      ].join("") || undefined,
    ctx,
    server ? code(`curl "${server}"`, "bash", ctx) : undefined,
  );
  return `<section class="pr-intro" id="introduction">
<div class="pr-op-main">
<div class="pr-op-eyebrow">${ref.version ? `<span class="pr-badge">v${esc(ref.version)}</span>` : ""}<span class="pr-badge">${ref.groups.reduce((n, g) => n + g.operations.length, 0)} operations</span></div>
<h1 class="pr-title">${esc(ref.title)}</h1>
${ref.description ? `<div class="pr-desc">${md(ref.description)}</div>` : ""}
${server ? `<h4 class="pr-section-title">Base URL</h4><div class="pr-server"><code>${esc(server)}</code></div>` : ""}
${auth ? `<h4 class="pr-section-title">Authentication</h4>${auth}` : ""}
${tryItSettings(ref)}
</div>
<aside class="pr-op-side"><div class="pr-card pr-request"><div class="pr-card-head"><span class="pr-card-title">Client libraries</span>${languageSelect(ctx)}<button class="pr-copy" type="button" aria-label="Copy">${COPY_ICON}</button></div><div class="pr-card-body">${clients}</div></div></aside>
</section>`;
}

export function renderSidebar(ref: Reference, options: RenderOptions = {}, current?: string): string {
  const ctx = context(ref, options);
  const group = (g: Group): string => {
    const open = current === g.id || !current;
    const items = g.operations
      .map((op) => `<li><a class="pr-nav-op" href="${esc(ctx.operationHref(op, g))}" data-slug="${esc(op.slug)}"><span class="pr-nav-label">${esc(op.title)}</span>${method(op.method, true)}</a></li>`)
      .join("");
    const children = ref.groups.filter((c) => c.parent === g.id).map((c) => `<li>${group(c)}</li>`).join("");
    return `<details class="pr-nav-group" data-group="${esc(g.id)}"${open && current ? " open" : ""}><summary><a href="${esc(ctx.groupHref(g))}">${esc(g.title)}</a>${CHEVRON_ICON}</summary><ul>${items}${children}</ul></details>`;
  };
  const groups = ref.groups.filter((g) => !g.parent).map(group).join("");
  return `<nav class="pr-nav" aria-label="API reference">${groups}</nav>`;
}

export function renderLanguageSwitcher(ref: Reference): string {
  return `<div class="pr-langs" role="radiogroup" aria-label="Language"${IGNORE}>${ref.languages
    .map((l, i) => `<button class="pr-lang" type="button" role="radio" data-lang="${esc(l.id)}" aria-checked="${i === 0}">${esc(l.label)}</button>`)
    .join("")}</div>`;
}

/** CSS hiding every language but the selected one, which `data-pr-lang` on `<html>` names
 * (set before the first paint), the first one when none is. */
export function languageCss(ref: Reference): string {
  const ids = ref.languages.map((l) => l.id);
  return ids
    .map((id) => `:root[data-pr-lang="${id}"] [data-l]:not([data-l="${id}"]){display:none}`)
    .concat(`:root:not([data-pr-lang]) [data-l]:not([data-l="${ids[0]}"]){display:none}`)
    .join("\n");
}

export const COPY_ICON = `<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path fill="currentColor" d="M5 2.5A1.5 1.5 0 0 1 6.5 1h6A1.5 1.5 0 0 1 14 2.5v6a1.5 1.5 0 0 1-1.5 1.5H11v1.5A1.5 1.5 0 0 1 9.5 13h-6A1.5 1.5 0 0 1 2 11.5v-6A1.5 1.5 0 0 1 3.5 4H5V2.5ZM6.5 2a.5.5 0 0 0-.5.5V4h3.5A1.5 1.5 0 0 1 11 5.5V9h1.5a.5.5 0 0 0 .5-.5v-6a.5.5 0 0 0-.5-.5h-6Zm-3 3a.5.5 0 0 0-.5.5v6a.5.5 0 0 0 .5.5h6a.5.5 0 0 0 .5-.5v-6a.5.5 0 0 0-.5-.5h-6Z"/></svg>`;
export const PLAY_ICON = `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path fill="currentColor" d="M4 2.8v10.4a.6.6 0 0 0 .9.5l8.3-5.2a.6.6 0 0 0 0-1L4.9 2.3a.6.6 0 0 0-.9.5Z"/></svg>`;
export const CHEVRON_ICON = `<svg class="pr-chevron" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="1.6" d="m6 4 4 4-4 4"/></svg>`;
export const SEARCH_ICON = `<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path stroke="currentColor" stroke-width="1.5" d="m10.5 10.5 3 3"/></svg>`;
