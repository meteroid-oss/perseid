import { parse as parseYaml } from "yaml";
import type { Schema } from "../reference/model.js";

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type Obj = { [key: string]: Json };

export const METHODS = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];

/** An OpenAPI document from its JSON or YAML text. */
export function parseSpec(text: string, location = "spec"): Obj {
  const doc = text.trimStart().startsWith("{") ? JSON.parse(text) : parseYaml(text);
  if (!doc || typeof doc !== "object" || !("openapi" in doc)) {
    throw new Error(`${location} is not an OpenAPI 3 document`);
  }
  return doc as Obj;
}

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);

/** The value a local `$ref` points to. */
export function resolve(spec: Obj, value: Json | undefined): Json | undefined {
  let current = value;
  for (let hops = 0; isObj(current) && typeof current.$ref === "string" && hops < 32; hops++) {
    const ref = current.$ref;
    if (!ref.startsWith("#/")) return current;
    let target: Json | undefined = spec;
    for (const raw of ref.slice(2).split("/")) {
      const key = decodeURIComponent(raw).replace(/~1/g, "/").replace(/~0/g, "~");
      target = isObj(target) ? target[key] : undefined;
    }
    current = target;
  }
  return current;
}

const SCHEMA_REF = "#/components/schemas/";

/** Reduces an OpenAPI 3.0/3.1 schema to the viewer's: named schemas stay references,
 * `nullable` and `type: [.., "null"]` and `oneOf: [x, {type: null}]` become `nullable`. */
export function toSchema(spec: Obj, raw: Json | undefined): Schema {
  if (!isObj(raw)) return {};
  if (typeof raw.$ref === "string") {
    if (raw.$ref.startsWith(SCHEMA_REF)) {
      const out: Schema = { ref: decodeURIComponent(raw.$ref.slice(SCHEMA_REF.length)) };
      if (typeof raw.description === "string") out.description = raw.description;
      return out;
    }
    const target = resolve(spec, raw);
    return target === raw ? {} : toSchema(spec, target);
  }
  for (const key of ["oneOf", "anyOf"] as const) {
    const variants = raw[key];
    if (Array.isArray(variants)) {
      const nonNull = variants.filter((v) => !(isObj(v) && v.type === "null"));
      if (nonNull.length === 1 && nonNull.length < variants.length) {
        const { [key]: _, ...rest } = raw;
        const inner = toSchema(spec, nonNull[0]);
        return { ...toSchema(spec, rest), ...inner, nullable: true, ...describe(rest) };
      }
    }
  }
  const out: Schema = {};
  let type = raw.type;
  if (Array.isArray(type)) {
    const types = type.filter((t): t is string => typeof t === "string" && t !== "null");
    if (types.length < type.length) out.nullable = true;
    type = types.length === 1 ? types[0]! : types;
  }
  if (typeof type === "string" || Array.isArray(type)) out.type = type as string | string[];
  if (raw.nullable === true) out.nullable = true;
  const scalars = ["format", "title", "description", "pattern"] as const;
  for (const key of scalars) if (typeof raw[key] === "string") out[key] = raw[key] as string;
  const numbers = ["minimum", "maximum", "minLength", "maxLength"] as const;
  for (const key of numbers) if (typeof raw[key] === "number") out[key] = raw[key] as number;
  for (const key of ["deprecated", "readOnly", "writeOnly"] as const) {
    if (raw[key] === true) out[key] = true;
  }
  if (Array.isArray(raw.enum)) out.enum = raw.enum.filter((v) => v !== null);
  if ("const" in raw) out.const = raw.const;
  if ("default" in raw) out.default = raw.default;
  if ("example" in raw) out.example = raw.example;
  else if (Array.isArray(raw.examples) && raw.examples.length) out.example = raw.examples[0];
  if (raw.items !== undefined) out.items = toSchema(spec, raw.items);
  if (isObj(raw.properties)) {
    out.properties = Object.fromEntries(
      Object.entries(raw.properties).map(([name, prop]) => [name, toSchema(spec, prop)]),
    );
  }
  if (Array.isArray(raw.required)) out.required = raw.required.filter((r) => typeof r === "string") as string[];
  if (raw.additionalProperties === true || isObj(raw.additionalProperties)) {
    out.additionalProperties =
      raw.additionalProperties === true ? true : toSchema(spec, raw.additionalProperties);
  }
  for (const key of ["oneOf", "anyOf", "allOf"] as const) {
    const list = raw[key];
    if (Array.isArray(list)) out[key] = list.map((v) => toSchema(spec, v));
  }
  if (isObj(raw.discriminator) && typeof raw.discriminator.propertyName === "string") {
    out.discriminator = { propertyName: raw.discriminator.propertyName };
    if (isObj(raw.discriminator.mapping)) {
      out.discriminator.mapping = Object.fromEntries(
        Object.entries(raw.discriminator.mapping).map(([k, v]) => [
          k,
          String(v).replace(SCHEMA_REF, ""),
        ]),
      );
    }
  }
  if (!out.type && out.properties) out.type = "object";
  return out;
}

function describe(raw: Obj): Partial<Schema> {
  return typeof raw.description === "string" ? { description: raw.description } : {};
}

/** A JSON value valid for `schema`, as an example. Named schemas are followed through `models`. */
export function sample(
  schema: Schema,
  models: Record<string, Schema>,
  options: { requiredOnly?: boolean; skipReadOnly?: boolean } = {},
  seen: string[] = [],
): unknown {
  if (schema.example !== undefined) return schema.example;
  if (schema.ref) {
    const target = models[schema.ref];
    if (!target || seen.includes(schema.ref) || seen.length > 6) return schema.nullable ? null : {};
    return sample(target, models, options, [...seen, schema.ref]);
  }
  if (schema.const !== undefined) return schema.const;
  if (schema.enum?.length) return schema.enum[0];
  if (schema.default !== undefined && schema.default !== null) return schema.default;
  if (schema.allOf?.length) {
    return Object.assign({}, ...schema.allOf.map((s) => sample(s, models, options, seen)));
  }
  const variant = schema.oneOf?.[0] ?? schema.anyOf?.[0];
  if (variant) return sample(variant, models, options, seen);
  const type = Array.isArray(schema.type) ? schema.type[0] : schema.type;
  switch (type) {
    case "object": {
      const out: Record<string, unknown> = {};
      for (const [name, prop] of Object.entries(schema.properties ?? {})) {
        if (options.requiredOnly && !schema.required?.includes(name)) continue;
        if (options.skipReadOnly && prop.readOnly) continue;
        out[name] = sample(prop, models, options, seen);
      }
      if (!schema.properties && typeof schema.additionalProperties === "object") {
        out.key = sample(schema.additionalProperties, models, options, seen);
      }
      return out;
    }
    case "array":
      return schema.items ? [sample(schema.items, models, options, seen)] : [];
    case "integer":
      return schema.minimum ?? 0;
    case "number":
      return schema.minimum ?? 0;
    case "boolean":
      return true;
    case "string":
      return stringSample(schema.format);
    default:
      return schema.properties ? sample({ ...schema, type: "object" }, models, options, seen) : null;
  }
}

function stringSample(format?: string): string {
  switch (format) {
    case "date-time":
      return "2024-01-02T15:04:05Z";
    case "date":
      return "2024-01-02";
    case "uuid":
      return "3fa85f64-5717-4562-b3fc-2c963f66afa6";
    case "email":
      return "jane@example.com";
    case "uri":
    case "url":
      return "https://example.com";
    default:
      return "string";
  }
}
