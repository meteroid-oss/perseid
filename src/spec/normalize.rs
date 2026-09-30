//! Rewrites an OpenAPI 3.1 document into the subset the model reads: `$ref` parameters, request
//! bodies and responses are inlined, path-level parameters are copied into operations, every
//! operation has an id, JSON media types are keyed `application/json`, inline object schemas
//! become named components, and `allOf` over object schemas is merged into one object.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context as _, Result, bail};
use heck::{ToSnakeCase as _, ToUpperCamelCase as _};
use serde_json::{Map, Value, json};

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];
const SCHEMA_PREFIX: &str = "#/components/schemas/";
/// Keys describing how a property is used rather than what its value is.
const PROPERTY_ANNOTATIONS: [&str; 5] = [
    "description",
    "deprecated",
    "readOnly",
    "writeOnly",
    "default",
];

pub(super) fn normalize(doc: &mut Value) -> Result<()> {
    let root = doc.clone();
    inline_operation_refs(doc, &root)?;
    Promoter::run(doc);
    flatten_all_of(doc);
    Ok(())
}

fn inline_operation_refs(doc: &mut Value, root: &Value) -> Result<()> {
    let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    for (path, item) in paths.iter_mut() {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.contains_key("$ref") {
            bail!("path `{path}`: `$ref` path items are not supported");
        }
        let shared = match item.remove("parameters") {
            Some(Value::Array(params)) => params
                .into_iter()
                .map(|p| resolve(root, p))
                .collect::<Result<Vec<_>>>()
                .with_context(|| format!("path `{path}`"))?,
            _ => Vec::new(),
        };
        for method in METHODS {
            let Some(op) = item.get_mut(method).and_then(Value::as_object_mut) else {
                continue;
            };
            normalize_operation(op, method, path, &shared, root)
                .with_context(|| format!("{} {path}", method.to_uppercase()))?;
        }
    }
    Ok(())
}

fn normalize_operation(
    op: &mut Map<String, Value>,
    method: &str,
    path: &str,
    shared: &[Value],
    root: &Value,
) -> Result<()> {
    if !op.get("operationId").is_some_and(Value::is_string) {
        op.insert(
            "operationId".into(),
            derive_operation_id(method, path).into(),
        );
    }
    let own = match op.remove("parameters") {
        Some(Value::Array(params)) => params
            .into_iter()
            .map(|p| resolve(root, p))
            .collect::<Result<Vec<_>>>()?,
        _ => Vec::new(),
    };
    let key = |p: &Value| format!("{}\0{}", p["in"], p["name"]);
    let overridden: BTreeSet<_> = own.iter().map(key).collect();
    let mut params: Vec<Value> = shared
        .iter()
        .filter(|p| !overridden.contains(&key(p)))
        .cloned()
        .collect();
    params.extend(own);
    if !params.is_empty() {
        op.insert("parameters".into(), params.into());
    }
    if let Some(body) = op.remove("requestBody") {
        let mut body = resolve(root, body).context("request body")?;
        normalize_content(&mut body);
        op.insert("requestBody".into(), body);
    }
    if let Some(Value::Object(responses)) = op.get_mut("responses") {
        for (status, response) in responses.iter_mut() {
            *response =
                resolve(root, response.take()).with_context(|| format!("response `{status}`"))?;
            normalize_content(response);
        }
    }
    Ok(())
}

/// `GET /things/{id}/parts` becomes `get_things_by_id_parts`.
fn derive_operation_id(method: &str, path: &str) -> String {
    let mut parts = vec![method.to_owned()];
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(param) => parts.push(format!("by_{param}")),
            None => parts.push(segment.to_owned()),
        }
    }
    parts.join("_").to_snake_case()
}

/// Follows local `$ref`s, keeping sibling keys of the reference over the referenced ones.
fn resolve(root: &Value, value: Value) -> Result<Value> {
    let mut value = value;
    for _ in 0..32 {
        let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
            return Ok(value);
        };
        let Some(pointer) = reference.strip_prefix('#') else {
            bail!("external reference `{reference}` is not supported");
        };
        let target = root
            .pointer(pointer)
            .with_context(|| format!("unresolved reference `{reference}`"))?;
        let mut resolved = target.clone();
        if let (Value::Object(resolved), Value::Object(siblings)) = (&mut resolved, value) {
            for (key, sibling) in siblings {
                if key != "$ref" {
                    resolved.insert(key, sibling);
                }
            }
        }
        value = resolved;
    }
    bail!("reference cycle")
}

fn is_json_media_type(media_type: &str) -> bool {
    let base = media_type.split(';').next().unwrap_or("").trim();
    base.eq_ignore_ascii_case("application/json")
        || (base.starts_with("application/") && base.ends_with("+json"))
}

/// Keys the first JSON-like media type (`application/vnd.api+json`, `application/json;
/// charset=utf-8`) as `application/json` when the plain one is absent.
fn normalize_content(holder: &mut Value) {
    let Some(content) = holder.get_mut("content").and_then(Value::as_object_mut) else {
        return;
    };
    if content.contains_key("application/json") {
        return;
    }
    let Some(key) = content.keys().find(|k| is_json_media_type(k)).cloned() else {
        return;
    };
    *content = std::mem::take(content)
        .into_iter()
        .map(|(k, v)| match k == key {
            true => ("application/json".to_owned(), v),
            false => (k, v),
        })
        .collect();
}

fn is_null_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

/// The index of `X` in `oneOf`/`anyOf: [X, {type: null}]`, in either order.
fn nullable_wrapper(schema: &Value) -> Option<(&'static str, usize)> {
    for key in ["oneOf", "anyOf"] {
        if let Some(Value::Array(variants)) = schema.get(key)
            && variants.len() == 2
            && let Some(nulls) = variants.iter().position(is_null_schema)
        {
            return Some((key, 1 - nulls));
        }
    }
    None
}

fn has_properties(schema: &Value) -> bool {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| !p.is_empty())
}

/// Whether the schema deserves a named type rather than an inline field type.
fn is_structured(schema: &Value) -> bool {
    if schema.get("$ref").is_some() || nullable_wrapper(schema).is_some() {
        return false;
    }
    has_properties(schema)
        || substantive_parts(schema).len() > 1
        || (schema.get("oneOf").is_some() && schema.get("discriminator").is_some())
}

/// A part like `{nullable: true}` or `{description: ...}`, which only annotates its siblings.
fn is_annotation(part: &Value) -> bool {
    const KEYS: [&str; 9] = [
        "description",
        "title",
        "nullable",
        "default",
        "example",
        "examples",
        "deprecated",
        "readOnly",
        "writeOnly",
    ];
    part.as_object().is_some_and(|p| {
        p.keys()
            .all(|k| KEYS.contains(&k.as_str()) || k.starts_with("x-"))
    })
}

/// Indices of the `allOf` parts that are not mere annotations.
fn substantive_parts(schema: &Value) -> Vec<usize> {
    let parts = schema.get("allOf").and_then(Value::as_array);
    let parts = parts.into_iter().flatten().enumerate();
    parts
        .filter(|(_, p)| !is_annotation(p))
        .map(|(i, _)| i)
        .collect()
}

struct Promoter {
    schemas: Map<String, Value>,
    /// Schema names by type name, as `Issue` and `issue` would both declare `Issue`.
    types: BTreeMap<String, String>,
    queue: VecDeque<String>,
}

impl Promoter {
    fn run(doc: &mut Value) {
        let schemas = match doc.pointer_mut("/components/schemas") {
            Some(Value::Object(schemas)) => std::mem::take(schemas),
            _ => Map::new(),
        };
        let mut types = BTreeMap::new();
        for name in schemas.keys() {
            types
                .entry(name.to_upper_camel_case())
                .or_insert_with(|| name.clone());
        }
        let mut promoter = Self {
            queue: schemas.keys().cloned().collect(),
            types,
            schemas,
        };
        if let Some(Value::Object(paths)) = doc.get_mut("paths") {
            for item in paths.values_mut() {
                for method in METHODS {
                    if let Some(op) = item.get_mut(method) {
                        promoter.operation(op);
                    }
                }
            }
        }
        promoter.drain();
        if !promoter.schemas.is_empty() {
            let components = doc
                .as_object_mut()
                .map(|d| d.entry("components").or_insert_with(|| json!({})));
            if let Some(Value::Object(components)) = components {
                components.insert("schemas".into(), Value::Object(promoter.schemas));
            }
        }
    }

    fn operation(&mut self, op: &mut Value) {
        let id = op["operationId"].as_str().unwrap_or_default().to_owned();
        for media in ["application/json", "application/x-www-form-urlencoded"] {
            if let Some(schema) = op.pointer_mut(&format!(
                "/requestBody/content/{}/schema",
                media.replace('/', "~1")
            )) {
                self.body(schema, format!("{id}_request"));
            }
        }
        let Some(Value::Object(responses)) = op.get_mut("responses") else {
            return;
        };
        let mut success = 0;
        for (status, response) in responses.iter_mut() {
            let name = match status.as_str() {
                s if s.starts_with('2') => {
                    success += 1;
                    match success {
                        1 => format!("{id}_response"),
                        _ => format!("{id}_response_{status}"),
                    }
                }
                "default" => format!("{id}_default_response"),
                _ => format!("{id}_response_{status}"),
            };
            if let Some(schema) = response.pointer_mut("/content/application~1json/schema") {
                self.body(schema, name);
            }
        }
    }

    fn body(&mut self, schema: &mut Value, name: String) {
        // A body is either sent or not: `null` is not a value the SDK should offer.
        if let Some((key, index)) = nullable_wrapper(schema) {
            *schema = schema[key][index].take();
        }
        drop_null_type(schema);
        let is_union = schema.get("oneOf").is_some() || schema.get("anyOf").is_some();
        // `{type: object, properties: {}}` is an empty named body, not a free-form map.
        let is_empty_object = schema.get("type").is_some_and(|t| t == "object")
            && schema.get("properties").is_some()
            && !schema
                .get("additionalProperties")
                .is_some_and(Value::is_object);
        if (is_union || is_empty_object)
            && nullable_wrapper(schema).is_none()
            && schema.get("$ref").is_none()
        {
            let promoted = self.add(name, std::mem::take(schema));
            *schema = reference(&promoted);
            return;
        }
        self.inline(schema, name);
    }

    fn drain(&mut self) {
        while let Some(name) = self.queue.pop_front() {
            let Some(mut schema) = self.schemas.get_mut(&name).map(Value::take) else {
                continue;
            };
            // A nullable component is typed as its object: the references carry nullability.
            if let Some((key, index)) = nullable_wrapper(&schema)
                && is_structured(&schema[key][index])
            {
                let mut inner = schema[key][index].take();
                if let (Some(outer), Some(inner)) = (schema.as_object(), inner.as_object_mut()) {
                    for (k, v) in outer.iter().filter(|(k, _)| *k != key) {
                        inner.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                schema = inner;
            }
            self.children(&mut schema, &name);
            self.schemas.insert(name, schema);
        }
    }

    /// Promotes the inline object schemas nested in `schema`, named after `parent`.
    fn children(&mut self, schema: &mut Value, parent: &str) {
        self.variants(schema, parent);
        if let Some(Value::Object(properties)) = schema.get_mut("properties") {
            for (property, value) in properties.iter_mut() {
                self.inline(value, format!("{parent}_{property}"));
            }
        }
        if let Some(items) = schema.get_mut("items") {
            self.inline(items, format!("{parent}_item"));
        }
        if let Some(value) = schema.get_mut("additionalProperties")
            && value.is_object()
        {
            self.inline(value, format!("{parent}_value"));
        }
        for key in ["allOf", "oneOf", "anyOf"] {
            if let Some(Value::Array(parts)) = schema.get_mut(key) {
                for part in parts {
                    if part.get("$ref").is_none() {
                        self.children(part, parent);
                    }
                }
            }
        }
    }

    /// Names the inline variants of a discriminated union `{parent}_{value}_variant` (templates
    /// use `{parent}_{value}` for their own declarations), and maps their discriminator value to
    /// them, so that every SDK sees referenced variants. Unmapped referenced variants are mapped
    /// from the values their discriminator property allows, rather than their schema name.
    fn variants(&mut self, schema: &mut Value, parent: &str) {
        let Some(property) = schema
            .pointer("/discriminator/propertyName")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let nested_mapping = self.flatten_nested_unions(schema, &property);
        let mapped: Vec<String> = schema
            .pointer("/discriminator/mapping")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.values().filter_map(Value::as_str).map(ToOwned::to_owned))
            .collect();
        let Some(Value::Array(variants)) = schema.get_mut("oneOf") else {
            return;
        };
        let mut mapping = nested_mapping;
        for variant in variants.iter_mut().filter(|v| v.get("$ref").is_none()) {
            let values = self.discriminator_values(variant, &property);
            let Some(first) = values.first() else {
                continue;
            };
            let promoted = self.add(format!("{parent}_{first}_variant"), variant.take());
            *variant = reference(&promoted);
            for value in values {
                mapping.push((value, format!("{SCHEMA_PREFIX}{promoted}")));
            }
        }
        for target in variants.iter().filter_map(|v| v.get("$ref")?.as_str()) {
            let Some(name) = target.strip_prefix(SCHEMA_PREFIX) else {
                continue;
            };
            let is_mapped = |m: &str| m.rsplit('/').next() == Some(name);
            if mapped.iter().any(|m| is_mapped(m)) || mapping.iter().any(|(_, m)| is_mapped(m)) {
                continue;
            }
            let Some(variant) = self.schemas.get(name) else {
                continue;
            };
            for value in self.discriminator_values(variant, &property) {
                mapping.push((value, target.to_owned()));
            }
        }
        if mapping.is_empty() {
            return;
        }
        let discriminator = &mut schema["discriminator"];
        if !discriminator.get("mapping").is_some_and(Value::is_object) {
            discriminator["mapping"] = json!({});
        }
        for (value, target) in mapping {
            if discriminator["mapping"].get(&value).is_none() {
                discriminator["mapping"][value] = target.into();
            }
        }
    }

    /// Replaces variants that are themselves unions on the same `property` by their variants,
    /// returning the nested mappings.
    fn flatten_nested_unions(&self, schema: &mut Value, property: &str) -> Vec<(String, String)> {
        let mut mapping = Vec::new();
        for _ in 0..8 {
            let Some(Value::Array(variants)) = schema.get("oneOf") else {
                break;
            };
            let mut flattened = Vec::new();
            let mut changed = false;
            for variant in variants {
                let nested = variant
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| self.schemas.get(r.strip_prefix(SCHEMA_PREFIX)?))
                    .filter(|n| {
                        n.pointer("/discriminator/propertyName")
                            .and_then(Value::as_str)
                            == Some(property)
                    })
                    .and_then(|n| Some((n.get("oneOf")?.as_array()?, n)))
                    .filter(|(parts, _)| parts.iter().all(|p| p.get("$ref").is_some()));
                let Some((parts, nested)) = nested else {
                    flattened.push(variant.clone());
                    continue;
                };
                changed = true;
                flattened.extend(parts.iter().cloned());
                let nested_mapping = nested
                    .pointer("/discriminator/mapping")
                    .and_then(Value::as_object);
                for (value, target) in nested_mapping.into_iter().flatten() {
                    if let Some(target) = target.as_str() {
                        let name = target.rsplit('/').next().unwrap_or(target);
                        mapping.push((value.clone(), format!("{SCHEMA_PREFIX}{name}")));
                    }
                }
            }
            if !changed {
                break;
            }
            let mut seen = Vec::new();
            flattened.retain(|v| {
                !seen.contains(v) && {
                    seen.push(v.clone());
                    true
                }
            });
            schema["oneOf"] = Value::Array(flattened);
        }
        mapping
    }

    /// The strings a variant allows for the discriminator `property`, which may be an enum
    /// component.
    fn discriminator_values(&self, variant: &Value, property: &str) -> Vec<String> {
        discriminator_values(variant, property, |target| {
            self.schemas.get(target.strip_prefix(SCHEMA_PREFIX)?)
        })
    }

    fn inline(&mut self, schema: &mut Value, name: String) {
        if let Some((key, index)) = nullable_wrapper(schema) {
            return self.inline(&mut schema[key][index], name);
        }
        if let [index] = substantive_parts(schema)[..]
            && !has_properties(schema)
        {
            return self.inline(&mut schema["allOf"][index], name);
        }
        if !is_structured(schema) {
            return self.children(schema, &name);
        }
        let Value::Object(mut map) = std::mem::take(schema) else {
            unreachable!("structured schemas are objects")
        };
        let mut outer = Map::new();
        for key in PROPERTY_ANNOTATIONS {
            if let Some(value) = map.remove(key) {
                if key == "description" {
                    map.insert(key.into(), value.clone());
                }
                outer.insert(key.into(), value);
            }
        }
        let mut promoted = Value::Object(map);
        let nullable = drop_null_type(&mut promoted);
        let Value::Object(map) = promoted else {
            unreachable!("still an object")
        };
        let promoted = reference(&self.add(name, Value::Object(map)));
        if nullable {
            outer.insert("oneOf".into(), json!([{ "type": "null" }, promoted]));
        } else if let Value::Object(promoted) = promoted {
            outer.extend(promoted);
        }
        *schema = Value::Object(outer);
    }

    /// Adds a component named after the schema's `title`, or `name`, suffixed on collision.
    fn add(&mut self, name: String, schema: Value) -> String {
        let base = schema
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.to_upper_camel_case().is_empty())
            .unwrap_or(&name)
            .to_upper_camel_case();
        let mut candidate = base.clone();
        for n in 2.. {
            let Some(existing) = self.types.get(&candidate) else {
                break;
            };
            if self.schemas.get(existing) == Some(&schema) {
                return existing.clone();
            }
            candidate = format!("{base}{n}");
        }
        self.types.insert(candidate.clone(), candidate.clone());
        self.schemas.insert(candidate.clone(), schema);
        self.queue.push_back(candidate.clone());
        candidate
    }
}

/// The strings an inline variant allows for the discriminator `property`.
fn discriminator_values<'a>(
    variant: &'a Value,
    property: &str,
    resolve: impl Fn(&str) -> Option<&'a Value>,
) -> Vec<String> {
    let own = variant.get("properties").and_then(|p| p.get(property));
    let parts = variant.get("allOf").and_then(Value::as_array);
    let inherited = parts
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("properties")?.get(property));
    own.into_iter()
        .chain(inherited)
        .filter_map(|schema| match schema.get("$ref").and_then(Value::as_str) {
            Some(target) => resolve(target),
            None => Some(schema),
        })
        .map(|schema| nullable_wrapper(schema).map_or(schema, |(key, index)| &schema[key][index]))
        .map(|schema| match (schema.get("const"), schema.get("enum")) {
            (Some(Value::String(value)), _) => vec![value.clone()],
            (None, Some(Value::Array(values))) => values
                .iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect(),
            _ => vec![],
        })
        .find(|values| !values.is_empty())
        .unwrap_or_default()
}

/// Turns `type: [X, "null"]` into `type: X`, returning whether `null` was allowed.
fn drop_null_type(schema: &mut Value) -> bool {
    let Some(Value::Array(types)) = schema.get_mut("type") else {
        return false;
    };
    let before = types.len();
    types.retain(|t| t != "null");
    let nullable = types.len() < before;
    if types.len() == 1 {
        schema["type"] = types.remove(0);
    }
    nullable
}

fn reference(name: &str) -> Value {
    json!({ "$ref": format!("{SCHEMA_PREFIX}{name}") })
}

fn flatten_all_of(doc: &mut Value) {
    let schemas = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(Value::Object(components)) = doc.pointer_mut("/components/schemas") {
        components
            .values_mut()
            .for_each(|s| flatten_schema(s, &schemas));
    }
    if let Some(Value::Object(paths)) = doc.get_mut("paths") {
        for item in paths.values_mut() {
            for method in METHODS {
                let Some(op) = item.get_mut(method) else {
                    continue;
                };
                for param in op
                    .get_mut("parameters")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(schema) = param.get_mut("schema") {
                        flatten_schema(schema, &schemas);
                    }
                }
                let bodies = op
                    .pointer_mut("/requestBody/content")
                    .and_then(Value::as_object_mut)
                    .into_iter()
                    .flat_map(|c| c.values_mut())
                    .filter_map(|m| m.get_mut("schema"));
                bodies.for_each(|s| flatten_schema(s, &schemas));
                if let Some(Value::Object(responses)) = op.get_mut("responses") {
                    for response in responses.values_mut() {
                        let bodies = response
                            .get_mut("content")
                            .and_then(Value::as_object_mut)
                            .into_iter()
                            .flat_map(|c| c.values_mut())
                            .filter_map(|m| m.get_mut("schema"));
                        bodies.for_each(|s| flatten_schema(s, &schemas));
                    }
                }
            }
        }
    }
}

fn flatten_schema(schema: &mut Value, schemas: &Map<String, Value>) {
    if schema.get("allOf").is_some()
        && let Some(merged) = merge_all_of(schema, schemas, 0)
    {
        *schema = merged;
    }
    let Value::Object(map) = schema else { return };
    for key in ["properties", "patternProperties"] {
        if let Some(Value::Object(children)) = map.get_mut(key) {
            children
                .values_mut()
                .for_each(|s| flatten_schema(s, schemas));
        }
    }
    for key in ["items", "additionalProperties", "not"] {
        if let Some(child) = map.get_mut(key) {
            flatten_schema(child, schemas);
        }
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(Value::Array(children)) = map.get_mut(key) {
            children.iter_mut().for_each(|s| flatten_schema(s, schemas));
        }
    }
}

/// Merges the inline `allOf` parts, nested ones included, and sibling properties into one object
/// that keeps only its referenced parts in `allOf`, which SDKs embed. A single part without
/// siblings is the schema itself. `None` when a part is not an object (a union, a scalar), which
/// the model then rejects by name.
fn merge_all_of(schema: &Value, schemas: &Map<String, Value>, depth: usize) -> Option<Value> {
    let Value::Object(map) = schema else {
        return None;
    };
    let all_parts = map.get("allOf")?.as_array()?;
    let mut out = map.clone();
    out.remove("allOf");
    for part in all_parts.iter().filter(|p| is_annotation(p)) {
        for (key, value) in part.as_object().into_iter().flatten() {
            out.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    let parts: Vec<&Value> = all_parts.iter().filter(|p| !is_annotation(p)).collect();
    if parts.len() <= 1 && !has_properties(schema) && map.get("required").is_none() {
        if let Some(Value::Object(part)) = parts.first() {
            if part.contains_key("$ref") {
                out.remove("type");
            }
            for (key, value) in part {
                out.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let out = Value::Object(out);
        return match out.get("allOf") {
            Some(_) if depth < 16 => merge_all_of(&out, schemas, depth + 1),
            _ => Some(out),
        };
    }
    let mut merged = Merged::default();
    for part in parts {
        merged.part(part, schemas)?;
    }
    let mut own = map.clone();
    own.remove("allOf");
    merged.part(&Value::Object(own), schemas)?;
    out.insert("type".into(), "object".into());
    out.insert("properties".into(), Value::Object(merged.properties));
    match merged.required.is_empty() {
        true => out.remove("required"),
        false => out.insert("required".into(), merged.required.into()),
    };
    if !merged.references.is_empty() {
        out.insert("allOf".into(), merged.references.into());
    }
    Some(Value::Object(out))
}

#[derive(Default)]
struct Merged {
    references: Vec<Value>,
    properties: Map<String, Value>,
    required: Vec<Value>,
}

impl Merged {
    fn part(&mut self, part: &Value, schemas: &Map<String, Value>) -> Option<()> {
        if let Some(reference) = part.get("$ref").and_then(Value::as_str) {
            let name = reference.strip_prefix(SCHEMA_PREFIX)?;
            is_object(schemas.get(name)?, schemas, &mut vec![name.to_owned()])?;
            let reference = json!({ "$ref": reference });
            if !self.references.contains(&reference) {
                self.references.push(reference);
            }
            return Some(());
        }
        is_object_shallow(part)?;
        for sub in part
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.part(sub, schemas)?;
        }
        let properties = part.get("properties").and_then(Value::as_object);
        for (name, value) in properties.into_iter().flatten() {
            self.properties.insert(name.clone(), value.clone());
        }
        for name in part
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !self.required.contains(name) {
                self.required.push(name.clone());
            }
        }
        Some(())
    }
}

/// Whether a referenced `allOf` part is an object, which SDKs can embed.
fn is_object(schema: &Value, schemas: &Map<String, Value>, stack: &mut Vec<String>) -> Option<()> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.strip_prefix(SCHEMA_PREFIX)?.to_owned();
        if stack.contains(&name) {
            return None;
        }
        let target = schemas.get(&name)?;
        stack.push(name);
        return is_object(target, schemas, stack);
    }
    is_object_shallow(schema)?;
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        is_object(part, schemas, stack)?;
    }
    Some(())
}

fn is_object_shallow(schema: &Value) -> Option<()> {
    let map = schema.as_object()?;
    if ["oneOf", "anyOf", "not"]
        .iter()
        .any(|k| map.contains_key(*k))
    {
        return None;
    }
    let is_object = |t: &Value| t == "object";
    match map.get("type") {
        None => Some(()),
        Some(t) if is_object(t) => Some(()),
        Some(Value::Array(types))
            if types.iter().any(is_object) && types.iter().all(|t| is_object(t) || t == "null") =>
        {
            Some(())
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(mut doc: Value) -> Value {
        normalize(&mut doc).unwrap();
        doc
    }

    fn op(responses: Value) -> Value {
        json!({ "operationId": "op", "responses": responses })
    }

    #[test]
    fn referenced_parameters_bodies_and_responses_are_inlined() {
        let doc = normalized(json!({
            "paths": { "/x/{id}": {
                "parameters": [{ "$ref": "#/components/parameters/Id" }],
                "post": {
                    "operationId": "op",
                    "parameters": [{ "$ref": "#/components/parameters/Limit" }],
                    "requestBody": { "$ref": "#/components/requestBodies/B" },
                    "responses": { "200": { "$ref": "#/components/responses/R" } }
                }
            } },
            "components": {
                "parameters": {
                    "Id": { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } },
                    "Limit": { "name": "limit", "in": "query", "schema": { "type": "integer" } }
                },
                "requestBodies": { "B": { "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/Obj" } } } } },
                "responses": { "R": { "description": "r", "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/Obj" } } } } },
                "schemas": { "Obj": { "type": "object" } }
            }
        }));
        let op = &doc["paths"]["/x/{id}"]["post"];
        assert!(doc["paths"]["/x/{id}"].get("parameters").is_none());
        let names: Vec<_> = op["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| &p["name"])
            .collect();
        assert_eq!(names, ["id", "limit"]);
        assert_eq!(
            op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Obj"
        );
        assert_eq!(op["responses"]["200"]["description"], "r");
    }

    #[test]
    fn operation_level_parameters_override_path_level_ones() {
        let doc = normalized(json!({ "paths": { "/x": {
            "parameters": [{ "name": "q", "in": "query", "description": "shared" }],
            "get": { "operationId": "op", "parameters": [{ "name": "q", "in": "query", "description": "own" }], "responses": {} }
        } } }));
        let params = doc["paths"]["/x"]["get"]["parameters"].as_array().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["description"], "own");
    }

    #[test]
    fn missing_operation_ids_are_derived_from_method_and_path() {
        let doc = normalized(
            json!({ "paths": { "/v2/things/{thing_id}": { "get": { "responses": {} } } } }),
        );
        assert_eq!(
            doc["paths"]["/v2/things/{thing_id}"]["get"]["operationId"],
            "get_v2_things_by_thing_id"
        );
    }

    #[test]
    fn vendor_json_media_types_are_read_as_json() {
        let doc = normalized(json!({ "paths": { "/x": { "get": op(json!({ "200": {
            "content": { "application/vnd.api+json": { "schema": { "$ref": "#/components/schemas/A" } } }
        } })) } } }));
        let content = &doc["paths"]["/x"]["get"]["responses"]["200"]["content"];
        assert!(content.get("application/json").is_some());
        assert!(is_json_media_type("application/json; charset=utf-8"));
        assert!(!is_json_media_type("application/xml"));
    }

    #[test]
    fn inline_bodies_and_properties_become_components() {
        let doc = normalized(json!({
            "paths": { "/x": { "post": {
                "operationId": "create_thing",
                "requestBody": { "content": { "application/json": { "schema": {
                    "type": "object", "properties": { "name": { "type": "string" } } } } } },
                "responses": {
                    "200": { "content": { "application/json": { "schema": {
                        "type": "array", "items": { "type": "object", "properties": {
                            "id": { "type": "string" } } } } } } },
                    "422": { "content": { "application/json": { "schema": {
                        "type": "object", "properties": { "message": { "type": "string" } } } } } }
                }
            } } },
            "components": { "schemas": { "Holder": { "type": "object", "properties": {
                "address": { "type": ["object", "null"], "description": "where",
                    "properties": { "line1": { "type": "string" } } },
                "tags": { "type": "array", "items": { "type": "object",
                    "properties": { "label": { "type": "string" } } } }
            } } } }
        }));
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        let names: BTreeSet<_> = schemas.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            BTreeSet::from([
                "CreateThingRequest",
                "CreateThingResponseItem",
                "CreateThingResponse422",
                "Holder",
                "HolderAddress",
                "HolderTagsItem"
            ])
        );
        assert_eq!(
            schemas["Holder"]["properties"]["address"],
            json!({ "description": "where", "oneOf": [{ "type": "null" }, { "$ref": "#/components/schemas/HolderAddress" }] })
        );
        assert_eq!(schemas["HolderAddress"]["type"], "object");
        let op = &doc["paths"]["/x"]["post"];
        assert_eq!(
            op["responses"]["200"]["content"]["application/json"]["schema"]["items"]["$ref"],
            "#/components/schemas/CreateThingResponseItem"
        );
    }

    #[test]
    fn promoted_names_avoid_existing_components() {
        let doc = normalized(json!({ "components": { "schemas": {
            "HolderInner": { "type": "string" },
            "Holder": { "type": "object", "properties": { "inner": {
                "type": "object", "properties": { "a": { "type": "string" } } } } }
        } } }));
        assert_eq!(
            doc["components"]["schemas"]["Holder"]["properties"]["inner"]["$ref"],
            "#/components/schemas/HolderInner2"
        );
    }

    #[test]
    fn discriminated_property_unions_are_promoted() {
        let doc = normalized(json!({ "components": { "schemas": { "Holder": {
            "type": "object", "properties": { "shape": {
                "oneOf": [{ "$ref": "#/components/schemas/A" }],
                "discriminator": { "propertyName": "kind" } } } } } } }));
        assert_eq!(
            doc["components"]["schemas"]["Holder"]["properties"]["shape"]["$ref"],
            "#/components/schemas/HolderShape"
        );
    }

    #[test]
    fn inline_union_variants_become_mapped_components() {
        let doc = normalized(json!({ "components": { "schemas": { "Event": {
            "oneOf": [
                { "type": "object", "properties": { "kind": { "const": "created" }, "id": { "type": "string" } } },
                { "$ref": "#/components/schemas/Deleted" }
            ],
            "discriminator": { "propertyName": "kind", "mapping": { "deleted": "#/components/schemas/Deleted" } }
        } } } }));
        let event = &doc["components"]["schemas"]["Event"];
        assert_eq!(
            event["oneOf"][0]["$ref"],
            "#/components/schemas/EventCreatedVariant"
        );
        assert_eq!(
            event["discriminator"]["mapping"],
            json!({ "deleted": "#/components/schemas/Deleted", "created": "#/components/schemas/EventCreatedVariant" })
        );
    }

    #[test]
    fn inline_variants_allowing_several_values_map_each_of_them() {
        let doc = normalized(json!({ "components": { "schemas": { "Run": {
            "oneOf": [{ "properties": { "status": { "enum": ["queued", "in_progress"] } } }],
            "discriminator": { "propertyName": "status" }
        } } } }));
        assert_eq!(
            doc["components"]["schemas"]["Run"]["discriminator"]["mapping"],
            json!({
                "queued": "#/components/schemas/RunQueuedVariant",
                "in_progress": "#/components/schemas/RunQueuedVariant"
            })
        );
    }

    #[test]
    fn referenced_variants_are_mapped_from_their_discriminator_enum() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Tool": {
                "oneOf": [
                    { "$ref": "#/components/schemas/WebSearch" },
                    { "$ref": "#/components/schemas/Function" },
                    { "$ref": "#/components/schemas/Legacy" }
                ],
                "discriminator": { "propertyName": "type", "mapping": { "fn": "Function" } }
            },
            "WebSearch": { "properties": { "type": { "enum": ["web_search", "web_search_2"] } } },
            "Function": { "properties": { "type": { "enum": ["function"] } } },
            "Legacy": { "properties": { "type": { "type": "string" } } }
        } } }));
        assert_eq!(
            doc["components"]["schemas"]["Tool"]["discriminator"]["mapping"],
            json!({
                "fn": "Function",
                "web_search": "#/components/schemas/WebSearch",
                "web_search_2": "#/components/schemas/WebSearch"
            })
        );
    }

    #[test]
    fn nested_unions_on_the_same_property_are_flattened() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Input": {
                "oneOf": [{ "$ref": "#/components/schemas/Message" }, { "$ref": "#/components/schemas/Item" }],
                "discriminator": { "propertyName": "type" }
            },
            "Item": {
                "oneOf": [{ "$ref": "#/components/schemas/Call" }, { "$ref": "#/components/schemas/Message" }],
                "discriminator": { "propertyName": "type", "mapping": { "call": "Call" } }
            },
            "Message": { "properties": { "type": { "anyOf": [{ "enum": ["message"] }, { "type": "null" }] } } },
            "Call": { "properties": { "type": { "type": "string" } } }
        } } }));
        let input = &doc["components"]["schemas"]["Input"];
        assert_eq!(
            input["oneOf"],
            json!([{ "$ref": "#/components/schemas/Message" }, { "$ref": "#/components/schemas/Call" }])
        );
        assert_eq!(
            input["discriminator"]["mapping"],
            json!({ "call": "#/components/schemas/Call", "message": "#/components/schemas/Message" })
        );
    }

    #[test]
    fn all_of_merges_inline_nested_and_sibling_properties_and_keeps_references() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Base": { "type": "object", "required": ["id"], "properties": { "id": { "type": "string" } } },
            "Composed": {
                "description": "d",
                "allOf": [
                    { "allOf": [{ "$ref": "#/components/schemas/Base" }, { "properties": { "mid": { "type": "integer" } } }] },
                    { "required": ["extra"], "properties": { "extra": { "type": "string" } } }
                ],
                "required": ["sibling"],
                "properties": { "sibling": { "type": "string" } }
            },
            "Nested": { "allOf": [{ "allOf": [{ "$ref": "#/components/schemas/Base" }] }] },
            "Wrapper": { "type": "object", "properties": { "base": {
                "allOf": [{ "$ref": "#/components/schemas/Base" }, { "nullable": true }], "description": "doc" } } }
        } } }));
        let s = &doc["components"]["schemas"];
        assert_eq!(
            s["Composed"]["allOf"],
            json!([{ "$ref": "#/components/schemas/Base" }])
        );
        let keys: Vec<_> = s["Composed"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, ["mid", "extra", "sibling"]);
        assert_eq!(s["Composed"]["required"], json!(["extra", "sibling"]));
        assert_eq!(s["Composed"]["description"], "d");
        assert_eq!(s["Nested"], json!({ "$ref": "#/components/schemas/Base" }));
        assert_eq!(
            s["Wrapper"]["properties"]["base"],
            json!({ "$ref": "#/components/schemas/Base", "description": "doc", "nullable": true })
        );
    }

    #[test]
    fn all_of_over_a_union_is_left_for_the_model_to_reject() {
        let doc = normalized(json!({ "components": { "schemas": {
            "U": { "oneOf": [{ "type": "string" }, { "type": "integer" }] },
            "C": { "allOf": [{ "$ref": "#/components/schemas/U" }, { "properties": { "a": { "type": "string" } } }] }
        } } }));
        assert!(doc["components"]["schemas"]["C"].get("allOf").is_some());
    }

    #[test]
    fn external_references_are_rejected_with_their_location() {
        let mut doc = json!({ "paths": { "/x": { "get": {
            "operationId": "op", "parameters": [{ "$ref": "other.yaml#/P" }], "responses": {} } } } });
        let error = format!("{:#}", normalize(&mut doc).unwrap_err());
        assert!(
            error.contains("GET /x") && error.contains("other.yaml"),
            "{error}"
        );
    }
}
