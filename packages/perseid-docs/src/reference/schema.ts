import { esc, md, mdInline } from "./html.js";
import type { Model, Reference, Schema } from "./model.js";

const IGNORE = " data-pagefind-ignore";

/** Child attributes rendered inline down to this depth; deeper ones link to their model. */
const MAX_DEPTH = 3;

export interface SchemaContext {
  ref: Reference;
  /** Languages that name properties, for the per-language spans. */
  languages: string[];
  /** URL of a model's section. */
  modelHref: (name: string) => string;
}

/** `allOf` merged into one object schema, references followed. */
export function flatten(schema: Schema, models: Record<string, Model>, seen: string[] = []): Schema {
  if (schema.ref) {
    const model = models[schema.ref];
    if (!model || seen.includes(schema.ref)) return schema;
    return flatten(model.schema, models, [...seen, schema.ref]);
  }
  if (!schema.allOf?.length) return schema;
  const merged: Schema = { ...schema, type: "object", properties: { ...schema.properties }, required: [...(schema.required ?? [])] };
  delete merged.allOf;
  for (const part of schema.allOf) {
    const flat = flatten(part, models, seen);
    Object.assign(merged.properties!, flat.properties);
    merged.required!.push(...(flat.required ?? []));
    if (!merged.description && flat.description) merged.description = flat.description;
  }
  return merged;
}

function single(type: Schema["type"]): string | undefined {
  return Array.isArray(type) ? type.join(" | ") : type;
}

/** `string`, `integer <int64>`, `Customer`, `array of Customer`. */
export function typeLabel(schema: Schema, ctx: SchemaContext): string {
  if (schema.ref) {
    return `<a class="pr-type-ref" href="${esc(ctx.modelHref(schema.ref))}">${esc(schema.ref)}</a>`;
  }
  const type = single(schema.type);
  if (type === "array" && schema.items) return `array of ${typeLabel(schema.items, ctx)}`;
  const variants = schema.oneOf ?? schema.anyOf;
  if (variants?.length) return variants.map((v) => typeLabel(v, ctx)).join(" | ");
  if (schema.allOf?.length === 1) return typeLabel(schema.allOf[0]!, ctx);
  if (schema.enum?.length) return `enum`;
  if (type === "object" && !schema.properties && typeof schema.additionalProperties === "object") {
    return `map of ${typeLabel(schema.additionalProperties, ctx)}`;
  }
  const base = type ?? (schema.properties ? "object" : "any");
  return schema.format ? `${esc(base)} <span class="pr-format">${esc(schema.format)}</span>` : esc(base);
}

/** The facts under a property's description: allowed values, default, bounds. */
function facts(schema: Schema, ctx: SchemaContext): string {
  const target = schema.ref ? ctx.ref.models[schema.ref]?.schema : schema;
  const out: string[] = [];
  const values = target?.enum ?? (schema.items?.enum);
  if (values?.length && values.length <= 40) {
    out.push(`<span class="pr-fact-label">One of</span> ${values.map((v) => `<code>${esc(JSON.stringify(v).replace(/^"|"$/g, ""))}</code>`).join(" ")}`);
  }
  if (schema.default !== undefined) out.push(`<span class="pr-fact-label">Default</span> <code>${esc(JSON.stringify(schema.default))}</code>`);
  const bounds: string[] = [];
  if (schema.minimum !== undefined) bounds.push(`≥ ${schema.minimum}`);
  if (schema.maximum !== undefined) bounds.push(`≤ ${schema.maximum}`);
  if (schema.minLength !== undefined) bounds.push(`min length ${schema.minLength}`);
  if (schema.maxLength !== undefined) bounds.push(`max length ${schema.maxLength}`);
  if (bounds.length) out.push(bounds.map((b) => `<code>${esc(b)}</code>`).join(" "));
  if (schema.pattern) out.push(`<span class="pr-fact-label">Pattern</span> <code>${esc(schema.pattern)}</code>`);
  return out.length ? `<div class="pr-facts">${out.map((f) => `<div>${f}</div>`).join("")}</div>` : "";
}

/** One `<span data-l>` per language whose name differs, the wire name for the others. */
export function names(wire: string, perLanguage: Record<string, string | undefined>, ctx: SchemaContext): string {
  const distinct = new Set(ctx.languages.map((l) => perLanguage[l] ?? wire));
  if (distinct.size <= 1 && (perLanguage[ctx.languages[0]!] ?? wire) === wire) return esc(wire);
  const spans = ctx.languages.map((l, i) => `<span data-l="${l}"${i ? IGNORE : ""}>${esc(perLanguage[l] ?? wire)}</span>`);
  return `${spans.join("")}<span data-l="curl"${IGNORE}>${esc(wire)}</span>`;
}

function children(schema: Schema, ctx: SchemaContext, depth: number, seen: string[]): string {
  const target = schema.type === "array" && schema.items ? schema.items : schema;
  const variants = target.ref ? undefined : (target.oneOf ?? target.anyOf);
  if (variants && variants.length > 1) {
    const items = variants
      .map((v, i) => {
        const label = v.ref ?? v.title ?? `Variant ${i + 1}`;
        const body = properties(v, ctx, depth + 1, seen);
        return body
          ? `<details class="pr-variant"><summary>${esc(label)}</summary>${body}</details>`
          : `<div class="pr-variant-leaf">${typeLabel(v, ctx)}</div>`;
      })
      .join("");
    const disc = target.discriminator ? ` by <code>${esc(target.discriminator.propertyName)}</code>` : "";
    return `<div class="pr-children pr-union"><div class="pr-union-title">One of${disc}</div>${items}</div>`;
  }
  if (target.ref && seen.includes(target.ref)) return "";
  if (depth >= MAX_DEPTH && target.ref) {
    return `<a class="pr-see" href="${esc(ctx.modelHref(target.ref))}">See ${esc(target.ref)}</a>`;
  }
  const body = properties(target, ctx, depth + 1, seen);
  if (!body) return "";
  const count = Object.keys(flatten(target, ctx.ref.models).properties ?? {}).length;
  return `<details class="pr-children"><summary><span class="pr-children-show">Show</span><span class="pr-children-hide">Hide</span> ${count} child attribute${count === 1 ? "" : "s"}</summary>${body}</details>`;
}

/** A language's field names for `schema`, the parts it composes with `allOf` included: an SDK
 * modelling `allOf` as an intersection names inherited fields on the parts only. */
function sdkFields(schema: Schema, language: string, models: Record<string, Model>, seen: string[] = []): Record<string, string> {
  if (schema.ref) {
    const model = models[schema.ref];
    if (!model || seen.includes(schema.ref)) return {};
    return { ...sdkFields(model.schema, language, models, [...seen, schema.ref]), ...model.sdk[language]?.fields };
  }
  return Object.assign({}, ...(schema.allOf ?? []).map((part) => sdkFields(part, language, models, seen)));
}

/** The property list of an object schema, or `""` when it has none. */
export function properties(schema: Schema, ctx: SchemaContext, depth = 0, seen: string[] = []): string {
  const flat = flatten(schema, ctx.ref.models);
  const props = Object.entries(flat.properties ?? {});
  const inside = schema.ref ? [...seen, schema.ref] : seen;
  if (!props.length) return "";
  const required = new Set(flat.required ?? []);
  const sorted = [...props.filter(([n]) => required.has(n)), ...props.filter(([n]) => !required.has(n))];
  const fields = Object.fromEntries(ctx.languages.map((l) => [l, sdkFields(schema, l, ctx.ref.models)]));
  const rows = sorted.map(([name, prop]) => {
    const perLanguage = Object.fromEntries(ctx.languages.map((l) => [l, fields[l]![name]]));
    return row(names(name, perLanguage, ctx), prop, required.has(name), ctx, depth, inside);
  });
  return `<ul class="pr-props">${rows.join("")}</ul>`;
}

/** A property or parameter row. `name` is HTML. */
export function row(name: string, schema: Schema, required: boolean, ctx: SchemaContext, depth = 0, seen: string[] = []): string {
  const target = schema.ref ? ctx.ref.models[schema.ref]?.schema : undefined;
  const description = schema.description ?? target?.description;
  const badges = [
    required ? `<span class="pr-badge pr-required">required</span>` : "",
    schema.nullable ? `<span class="pr-badge">nullable</span>` : "",
    schema.readOnly ? `<span class="pr-badge">read-only</span>` : "",
    schema.deprecated ? `<span class="pr-badge pr-deprecated">deprecated</span>` : "",
  ].join("");
  return `<li class="pr-prop">
<div class="pr-prop-head"><code class="pr-prop-name">${name}</code><span class="pr-type">${typeLabel(schema, ctx)}</span>${badges}</div>
${description ? `<div class="pr-prop-desc">${md(description)}</div>` : ""}${facts(schema, ctx)}${children(schema, ctx, depth, seen)}
</li>`;
}

export { mdInline };
