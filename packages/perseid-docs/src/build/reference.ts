import type {
  AuthScheme,
  Body,
  Group,
  Language,
  Model,
  Operation,
  Param,
  Reference,
  Response,
  Schema,
  Webhook,
} from "../reference/model.js";
import type { DataResource, DocsData } from "./data.js";
import { wrap } from "./wrap.js";
import { METHODS, resolve, sample, toSchema, type Json, type Obj } from "./spec.js";

export interface AssembleOptions {
  /** Over the docs data's `base_url` and the spec's first server. */
  baseUrl?: string;
}

const LABELS: Record<string, [string, string]> = {
  typescript: ["TypeScript", "ts"],
  python: ["Python", "python"],
  go: ["Go", "go"],
  java: ["Java", "java"],
  csharp: ["C#", "csharp"],
  rust: ["Rust", "rust"],
  curl: ["cURL", "bash"],
};

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);

interface SpecOperation {
  id: string;
  method: string;
  path: string;
  op: Obj;
  params: Obj[];
}

/** The docs model of `spec`, with each SDK's names and samples when `data` is given. Pure: it
 * runs in a browser as well as in Node. */
export function assemble(spec: Obj, data?: DocsData, options: AssembleOptions = {}): Reference {
  const info = isObj(spec.info) ? spec.info : {};
  const components = isObj(spec.components) ? spec.components : {};
  const schemas = isObj(components.schemas) ? components.schemas : {};
  const models: Record<string, Model> = {};
  for (const [name, raw] of Object.entries(schemas)) {
    const schema = toSchema(spec, raw);
    const model: Model = { name, schema, sdk: {} };
    if (schema.description) model.description = schema.description;
    for (const [language, sdk] of Object.entries(data?.languages ?? {})) {
      const names = sdk.types[name];
      if (names) model.sdk[language] = { name: names.name, fields: names.fields };
    }
    models[name] = model;
  }
  const modelSchemas = Object.fromEntries(Object.entries(models).map(([k, m]) => [k, m.schema]));

  const servers = Array.isArray(spec.servers)
    ? spec.servers.filter(isObj).map((s) => ({
        url: String(s.url),
        ...(typeof s.description === "string" ? { description: s.description } : {}),
      }))
    : [];
  const baseUrl = (
    options.baseUrl ??
    data?.base_url ??
    servers[0]?.url ??
    "https://api.example.com"
  ).replace(/\/$/, "");
  if (!servers.length) servers.push({ url: baseUrl });
  const name = String(data?.name ?? info.title ?? "API");
  const envVar = `${name.replace(/[^A-Za-z0-9]+/g, "_").replace(/([a-z])([A-Z])/g, "$1_$2").toUpperCase()}_API_KEY`;

  const auth = authSchemes(spec, components);
  const operations = specOperations(spec);
  const languages: Language[] = Object.entries(data?.languages ?? {}).map(([id, sdk]) => ({
    id,
    label: LABELS[id]?.[0] ?? id,
    grammar: LABELS[id]?.[1] ?? id,
    install: sdk.install,
    setup: wrap(sdk.setup, id),
    ...(sdk.version ? { version: sdk.version } : {}),
  }));
  languages.push({ id: "curl", label: "cURL", grammar: "bash" });

  const tagDescriptions = new Map<string, string>();
  for (const tag of Array.isArray(spec.tags) ? spec.tags.filter(isObj) : []) {
    if (typeof tag.description === "string" && tag.description) {
      tagDescriptions.set(String(tag.name), tag.description);
    }
  }

  const build = (s: SpecOperation, slug: string): Operation =>
    operation(spec, s, slug, modelSchemas, data, auth, baseUrl, envVar);

  const groups: Group[] = [];
  const used = new Set<string>();
  const slugs = new Set<string>();
  const slugFor = (base: string) => {
    let slug = base;
    for (let i = 2; slugs.has(slug); i++) slug = `${base}-${i}`;
    slugs.add(slug);
    return slug;
  };
  const byId = new Map(operations.map((o) => [o.id, o]));
  const addResource = (resource: DataResource, parent?: Group) => {
    const ops = resource.operations.map((id) => byId.get(id)).filter((o): o is SpecOperation => !!o);
    const counts = new Map<string, number>();
    for (const o of ops) {
      for (const t of Array.isArray(o.op.tags) ? o.op.tags.map(String) : []) counts.set(t, (counts.get(t) ?? 0) + 1);
    }
    const tag = [...counts].sort((a, b) => b[1] - a[1])[0]?.[0];
    const id = parent ? `${parent.id}.${resource.name}` : resource.name;
    const group: Group = {
      id,
      title: parent ? humanize(resource.name) : (tag ?? humanize(resource.name)),
      operations: ops.map((o) => {
        used.add(o.id);
        return build(o, slugFor(kebab(`${id}-${String(o.op.summary ?? o.id)}`)));
      }),
    };
    if (parent) group.parent = parent.id;
    const description = !parent && tag && tagDescriptions.get(tag);
    if (description) group.description = description;
    groups.push(group);
    for (const sub of resource.subresources ?? []) addResource(sub, group);
  };
  for (const resource of data?.resources ?? []) addResource(resource);

  const byTag = new Map<string, SpecOperation[]>();
  for (const o of operations) {
    if (used.has(o.id) || o.op["x-internal"] === true) continue;
    const tag = Array.isArray(o.op.tags) && o.op.tags.length ? String(o.op.tags[0]) : "Other";
    byTag.set(tag, [...(byTag.get(tag) ?? []), o]);
  }
  for (const [tag, ops] of byTag) {
    const id = kebab(tag);
    const group: Group = {
      id,
      title: tag,
      operations: ops.map((o) => build(o, slugFor(kebab(`${id}-${o.id}`)))),
    };
    const description = tagDescriptions.get(tag);
    if (description) group.description = description;
    groups.push(group);
  }

  const reference: Reference = {
    title: data?.name ? name : String(info.title ?? name),
    servers,
    auth,
    languages,
    groups,
    models,
  };
  const webhooks = specWebhooks(spec, modelSchemas);
  if (webhooks.length) reference.webhooks = webhooks;
  if (typeof info.version === "string") reference.version = info.version;
  if (typeof info.description === "string" && info.description) reference.description = info.description;
  return reference;
}

function requestBody(spec: Obj, op: Obj, models: Record<string, Schema>): Body | undefined {
  const body = resolve(spec, op.requestBody);
  if (!isObj(body) || !isObj(body.content)) return undefined;
  const [contentType, media] = pickContent(body.content);
  if (!contentType || !isObj(media)) return undefined;
  const b: Body = { contentType, required: body.required === true, schema: toSchema(spec, media.schema) };
  if (typeof body.description === "string") b.description = body.description;
  b.example = mediaExample(spec, media) ?? sample(b.schema, models, { skipReadOnly: true });
  return b;
}

function specWebhooks(spec: Obj, models: Record<string, Schema>): Webhook[] {
  const all = { ...(isObj(spec["x-webhooks"]) ? spec["x-webhooks"] : {}), ...(isObj(spec.webhooks) ? spec.webhooks : {}) };
  const out: Webhook[] = [];
  for (const [name, raw] of Object.entries(all)) {
    const item = resolve(spec, raw);
    if (!isObj(item)) continue;
    const method = METHODS.find((m) => isObj(item[m]));
    if (!method) continue;
    const op = item[method] as Obj;
    const webhook: Webhook = { name, slug: `webhook-${kebab(name)}`, method };
    const description = [op.description, op.summary].find((d) => typeof d === "string" && d && d !== `${name} webhook`);
    if (typeof description === "string") webhook.description = description;
    const body = requestBody(spec, op, models);
    if (body) webhook.body = body;
    out.push(webhook);
  }
  return out;
}

function specOperations(spec: Obj): SpecOperation[] {
  const out: SpecOperation[] = [];
  const paths = isObj(spec.paths) ? spec.paths : {};
  for (const [path, rawItem] of Object.entries(paths)) {
    const item = resolve(spec, rawItem);
    if (!isObj(item)) continue;
    const shared = Array.isArray(item.parameters) ? item.parameters : [];
    for (const method of METHODS) {
      const op = item[method];
      if (!isObj(op)) continue;
      const own = Array.isArray(op.parameters) ? op.parameters : [];
      const params = new Map<string, Obj>();
      for (const raw of [...shared, ...own]) {
        const p = resolve(spec, raw);
        if (isObj(p)) params.set(`${p.in}:${p.name}`, p);
      }
      const id = typeof op.operationId === "string" ? op.operationId : `${method} ${path}`;
      out.push({ id, method, path, op, params: [...params.values()] });
    }
  }
  return out;
}

function authSchemes(spec: Obj, components: Obj): AuthScheme[] {
  const schemes = isObj(components.securitySchemes) ? components.securitySchemes : {};
  const out: AuthScheme[] = [];
  for (const [id, raw] of Object.entries(schemes)) {
    const s = resolve(spec, raw);
    if (!isObj(s)) continue;
    const scheme: AuthScheme =
      s.type === "http"
        ? { id, kind: String(s.scheme).toLowerCase() === "basic" ? "basic" : "bearer" }
        : s.type === "apiKey"
          ? { id, kind: "apiKey", in: s.in as "header", name: String(s.name) }
          : { id, kind: s.type === "openIdConnect" ? "openIdConnect" : "oauth2" };
    if (typeof s.description === "string") scheme.description = s.description;
    out.push(scheme);
  }
  return out;
}

function operation(
  spec: Obj,
  s: SpecOperation,
  slug: string,
  models: Record<string, Schema>,
  data: DocsData | undefined,
  auth: AuthScheme[],
  baseUrl: string,
  envVar: string,
): Operation {
  const { op } = s;
  const params: Param[] = s.params.map((p) => {
    const param: Param = {
      name: String(p.name),
      in: p.in as Param["in"],
      required: p.required === true || p.in === "path",
      schema: toSchema(spec, p.schema),
    };
    if (typeof p.description === "string") param.description = p.description;
    if (p.deprecated === true) param.deprecated = true;
    const examples = isObj(p.examples) ? Object.values(p.examples).filter(isObj) : [];
    const example = "example" in p ? p.example : examples[0]?.value;
    if (example !== undefined) param.schema.example = example;
    return param;
  });
  const security = (Array.isArray(op.security) ? op.security : Array.isArray(spec.security) ? spec.security : [])
    .filter(isObj)
    .flatMap((req) => Object.keys(req));
  const out: Operation = {
    id: s.id,
    slug,
    title: String(op.summary ?? firstLine(op.description) ?? humanize(s.id)),
    method: s.method,
    path: s.path,
    auth: [...new Set(security)].filter((id) => auth.some((a) => a.id === id)),
    params,
    responses: [],
    sdk: {},
    samples: {},
  };
  if (typeof op.description === "string" && op.description !== op.summary) out.description = op.description;
  if (op.deprecated === true) out.deprecated = true;

  const body = requestBody(spec, op, models);
  if (body) out.body = body;
  const responses = isObj(op.responses) ? op.responses : {};
  for (const [status, raw] of Object.entries(responses)) {
    const r = resolve(spec, raw);
    if (!isObj(r)) continue;
    const response: Response = { status };
    if (typeof r.description === "string") response.description = r.description;
    if (isObj(r.content)) {
      const [contentType, media] = pickContent(r.content);
      if (contentType) {
        response.contentType = contentType;
        if (isObj(media) && media.schema !== undefined) {
          response.schema = toSchema(spec, media.schema);
          response.example = mediaExample(spec, media) ?? sample(response.schema, models);
        }
      }
    }
    out.responses.push(response);
  }

  for (const [language, sdk] of Object.entries(data?.languages ?? {})) {
    const call = sdk.operations[s.id];
    if (!call) continue;
    const { "": _, ...names } = call.params;
    out.sdk[language] = { method: call.method, signature: call.signature, params: names };
    out.samples[language] = wrap(call.sample, language);
  }
  out.samples.curl = curl(out, models, baseUrl, envVar, auth);
  return out;
}

function pickContent(content: Obj): [string | undefined, Json | undefined] {
  const types = Object.keys(content);
  const type = types.find((t) => /json/.test(t)) ?? types[0];
  return [type, type ? content[type] : undefined];
}

function mediaExample(spec: Obj, media: Obj): unknown {
  if (media.example !== undefined) return media.example;
  if (isObj(media.examples)) {
    const first = resolve(spec, Object.values(media.examples)[0]);
    if (isObj(first) && first.value !== undefined) return first.value;
  }
  return undefined;
}

function curl(
  op: Operation,
  models: Record<string, Schema>,
  baseUrl: string,
  envVar: string,
  auth: AuthScheme[],
): string {
  const query = op.params
    .filter((p) => p.in === "query" && p.required)
    .map((p) => `${p.name}=${encodeURIComponent(String(sample(p.schema, models)))}`);
  const path = op.path.replace(/\{([^}]+)\}/g, (whole, name: string) => {
    const param = op.params.find((p) => p.in === "path" && p.name === name);
    const example = param && (param.schema.example ?? (param.schema.ref ? models[param.schema.ref]?.example : undefined));
    return example === undefined ? whole : encodeURIComponent(String(example));
  });
  const url = `${baseUrl}${path}${query.length ? `?${query.join("&")}` : ""}`;
  const lines = [`curl ${op.method === "get" ? "" : `-X ${op.method.toUpperCase()} `}"${url}"`];
  const scheme = auth.find((a) => op.auth.includes(a.id));
  if (scheme?.kind === "bearer" || scheme?.kind === "oauth2") lines.push(`-H "Authorization: Bearer $${envVar}"`);
  else if (scheme?.kind === "basic") lines.push(`-u "$${envVar}:"`);
  else if (scheme?.kind === "apiKey" && scheme.in === "header") lines.push(`-H "${scheme.name}: $${envVar}"`);
  for (const p of op.params.filter((p) => p.in === "header" && p.required)) {
    lines.push(`-H "${p.name}: ${String(sample(p.schema, models))}"`);
  }
  if (op.body) {
    if (/json/.test(op.body.contentType)) {
      const example = sample(op.body.schema, models, { requiredOnly: true, skipReadOnly: true });
      lines.push(`-H "Content-Type: ${op.body.contentType}"`);
      lines.push(`-d '${JSON.stringify(example, null, 2).replace(/'/g, "'\\''")}'`);
    } else if (/multipart/.test(op.body.contentType)) {
      lines.push(`-F "file=@path/to/file"`);
    } else {
      lines.push(`-H "Content-Type: ${op.body.contentType}"`, `--data-binary @path/to/file`);
    }
  }
  return lines.join(" \\\n  ");
}

function firstLine(text: unknown): string | undefined {
  return typeof text === "string" && text.trim() ? text.trim().split("\n")[0] : undefined;
}

export function humanize(id: string): string {
  const words = id.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_\-.]+/g, " ").trim().toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function kebab(text: string): string {
  return text
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/[^A-Za-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase();
}
