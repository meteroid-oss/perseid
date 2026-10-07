//! Rewrites an OpenAPI 3.1 document into the subset the model reads: `$ref` parameters, request
//! bodies and responses are inlined, path-level parameters are copied into operations, every
//! operation has an id, JSON media types are keyed `application/json`, inline object schemas
//! become named components, and `allOf` over object schemas is merged into one object.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context as _, Result, bail};
use heck::{ToSnakeCase as _, ToUpperCamelCase as _};
use serde_json::{Map, Value, json};

use super::upgrade;

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
    trim_descriptions(doc);
    upgrade::each_schema(doc, &mut binary_content);
    let renames = canonicalize_refs(doc);
    upgrade::type_unions(doc);
    let root = doc.clone();
    inline_operation_refs(doc, &root)?;
    open_enums(doc);
    nullable_references(doc);
    lower_base_discriminators(doc);
    infer_discriminators(doc);
    Promoter::run(doc);
    flatten_all_of(doc);
    // Pinned last, so that tags inferred from the variants' discriminator enums come first.
    let tags = renames.into_iter().map(|(old, new)| (new, old)).collect();
    pin_implicit_tags(doc, &tags);
    Ok(())
}

/// Gives `format: binary` to the strings of a binary `contentMediaType` without a
/// `contentEncoding`, or of `contentEncoding: binary` (OpenAPI 3.1 files), as 3.0 spells them.
fn binary_content(schema: &mut Map<String, Value>) {
    let media = schema.get("contentMediaType").and_then(Value::as_str);
    let encoding = schema.get("contentEncoding").and_then(Value::as_str);
    let binary_media = media.is_some_and(|media| {
        let media = media.to_ascii_lowercase();
        let essence = media.split(';').next().unwrap_or_default().trim();
        !essence.starts_with("text/")
            && !["json", "xml"]
                .iter()
                .any(|t| essence.ends_with(&format!("/{t}")) || essence.ends_with(&format!("+{t}")))
    });
    let string = schema.get("type").is_none_or(|t| t == "string");
    let binary = encoding == Some("binary") || (binary_media && encoding.is_none());
    if binary && string && !schema.contains_key("format") {
        schema.insert("format".into(), "binary".into());
    }
}

/// Strips the trailing whitespace of each line of the descriptions, which linters reject in
/// the doc comments they become.
fn trim_descriptions(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(trim_descriptions),
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match (key.as_str(), child) {
                    ("example" | "examples" | "default" | "enum" | "const", _) => {}
                    ("description" | "summary", Value::String(text)) => {
                        *text = text
                            .lines()
                            .map(str::trim_end)
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                    (_, child) => trim_descriptions(child),
                }
            }
        }
        _ => {}
    }
}

/// Removes `format: <format>` from every schema, which types its values as plain strings.
pub(super) fn drop_format(value: &mut Value, format: &str) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|item| drop_format(item, format)),
        Value::Object(map) => {
            if map.get("format").and_then(Value::as_str) == Some(format) {
                map.remove("format");
            }
            for (key, child) in map.iter_mut() {
                if !matches!(
                    key.as_str(),
                    "example" | "examples" | "default" | "enum" | "const"
                ) {
                    drop_format(child, format);
                }
            }
        }
        _ => {}
    }
}

/// Gives each `$ref` variant of a discriminated union that targets a schema in `tags` (by its
/// current name) and that no `mapping` entry names an explicit entry keyed by the tag given
/// there: the implicit tag is the schema name as the spec spells it, and renaming the schema
/// (unsafe characters, reserved names) must not change what is on the wire.
fn pin_implicit_tags(value: &mut Value, tags: &BTreeMap<String, String>) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| pin_implicit_tags(v, tags)),
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if !matches!(
                    key.as_str(),
                    "example" | "examples" | "default" | "enum" | "const"
                ) {
                    pin_implicit_tags(child, tags);
                }
            }
            let Some(Value::Object(discriminator)) = map.get("discriminator") else {
                return;
            };
            if !discriminator
                .get("propertyName")
                .is_some_and(Value::is_string)
            {
                return;
            }
            let mut mapping = discriminator
                .get("mapping")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let target_name = |t: &str| t.rsplit('/').next().map(str::to_owned);
            let mut added = false;
            for key in ["oneOf", "anyOf"] {
                for variant in map.get(key).and_then(Value::as_array).into_iter().flatten() {
                    let Some(reference) = variant.get("$ref").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some((name, tag)) = reference
                        .strip_prefix(SCHEMA_PREFIX)
                        .and_then(|n| tags.get_key_value(n))
                    else {
                        continue;
                    };
                    let mapped = mapping
                        .values()
                        .filter_map(Value::as_str)
                        .any(|t| target_name(t).as_deref() == Some(name.as_str()));
                    if !mapped && !mapping.contains_key(tag) {
                        mapping.insert(tag.clone(), Value::from(reference));
                        added = true;
                    }
                }
            }
            if added && let Some(Value::Object(d)) = map.get_mut("discriminator") {
                d.insert("mapping".into(), Value::Object(mapping));
            }
        }
        _ => {}
    }
}

/// Whether a schema name needs renaming because `$ref`s cannot spell it as a plain path segment.
fn is_unsafe_name_char(c: char) -> bool {
    matches!(c, '/' | '~' | '%' | '#' | '?')
}

/// Decodes `%XX` escapes of a URI fragment, leaving malformed escapes as they are.
pub(super) fn percent_decode(fragment: &str) -> String {
    let bytes = fragment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = fragment.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// RFC 6901 unescaping of one pointer segment.
pub(super) fn unescape_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Makes every local `$ref` plain: fragments are percent-decoded, JSON pointer escapes in schema
/// names are resolved (schemas whose names cannot be a path segment are renamed), and references
/// into a schema's nested locations are replaced by the schema found there.
/// Returns the schema renames made (spec name to new name).
fn canonicalize_refs(doc: &mut Value) -> BTreeMap<String, String> {
    let renames = sanitize_schema_names(doc);
    promote_recursive_locations(doc, &renames);
    for _ in 0..8 {
        let root = doc.clone();
        if !canonicalize_value(doc, &root, &renames, false) {
            break;
        }
    }
    renames
}

/// The schema name and the segments below it a local `$ref` into `components.schemas` points
/// to, decoded, with the schema renames applied.
fn schema_pointer(reference: &str, renames: &BTreeMap<String, String>) -> Option<Vec<String>> {
    let decoded = percent_decode(reference.strip_prefix('#')?);
    let mut segments = decoded
        .strip_prefix("/components/schemas/")?
        .split('/')
        .map(unescape_segment);
    let name = segments.next()?;
    let name = renames.get(&name).cloned().unwrap_or(name);
    Some(std::iter::once(name).chain(segments).collect())
}

/// What `segments` (a schema name and a location in it) point to.
fn lookup<'a>(schemas: &'a Value, segments: &[String]) -> Option<&'a Value> {
    segments
        .iter()
        .try_fold(schemas, |target, segment| match target {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?),
            _ => None,
        })
}

/// Calls `visit` on every `$ref` under `value`, skipping literals.
fn each_ref(value: &Value, names: bool, visit: &mut dyn FnMut(&str)) {
    match value {
        Value::Array(items) => items.iter().for_each(|i| each_ref(i, false, visit)),
        Value::Object(map) => {
            for (key, child) in map {
                if names {
                    each_ref(child, false, visit);
                } else if key == "$ref" {
                    child.as_str().into_iter().for_each(&mut *visit);
                } else if !upgrade::LITERALS.contains(&key.as_str()) {
                    each_ref(child, upgrade::NAME_MAPS.contains(&key.as_str()), visit);
                }
            }
        }
        _ => {}
    }
}

/// Moves the nested schema locations that reference themselves (`#/components/schemas/A/properties/x`
/// inside `A.properties.x`, directly or through other nested locations) into components of
/// their own, which inlining them would never finish. References to them, or into them, point
/// at the new component.
fn promote_recursive_locations(doc: &mut Value, renames: &BTreeMap<String, String>) {
    let Some(schemas) = doc.pointer("/components/schemas") else {
        return;
    };
    let nested = |value: &Value| {
        let mut found = BTreeSet::new();
        each_ref(value, false, &mut |reference| {
            if let Some(segments) = schema_pointer(reference, renames)
                && segments.len() > 1
            {
                found.insert(segments);
            }
        });
        found
    };
    let mut edges: BTreeMap<Vec<String>, BTreeSet<Vec<String>>> = BTreeMap::new();
    let mut pending: Vec<Vec<String>> = nested(doc).into_iter().collect();
    while let Some(location) = pending.pop() {
        if edges.contains_key(&location) {
            continue;
        }
        let targets = lookup(schemas, &location).map(nested).unwrap_or_default();
        pending.extend(targets.iter().cloned());
        edges.insert(location, targets);
    }
    let reaches_itself = |start: &Vec<String>| {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&Vec<String>> = edges[start].iter().collect();
        while let Some(location) = stack.pop() {
            if location == start {
                return true;
            }
            if seen.insert(location) {
                stack.extend(edges.get(location).into_iter().flatten());
            }
        }
        false
    };
    let mut recursive: Vec<Vec<String>> = edges
        .keys()
        .filter(|l| reaches_itself(l) && lookup(schemas, l).is_some_and(Value::is_object))
        .cloned()
        .collect();
    if recursive.is_empty() {
        return;
    }
    // The deepest first, so that a location inside another one moves before its parent does.
    recursive.sort_by_key(|l| std::cmp::Reverse(l.len()));
    let mut moved: Vec<(Vec<String>, String)> = Vec::new();
    for location in recursive {
        let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") else {
            return;
        };
        let words: String = location[1..]
            .iter()
            .filter(|s| !upgrade::NAME_MAPS.contains(&s.as_str()))
            .map(|s| s.to_upper_camel_case())
            .collect();
        let mut name = format!("{}{words}", location[0]);
        while schemas.contains_key(&name) {
            name.push('_');
        }
        let pointer: String = location
            .iter()
            .map(|s| format!("/{}", s.replace('~', "~0").replace('/', "~1")))
            .collect();
        let Some(slot) = doc.pointer_mut(&format!("/components/schemas{pointer}")) else {
            continue;
        };
        let schema = std::mem::replace(slot, reference(&name));
        if let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") {
            schemas.insert(name.clone(), schema);
        }
        moved.push((location, name));
    }
    redirect_moved(doc, &moved, renames, false);
}

/// Points the references at or into a moved location at the component it moved to.
fn redirect_moved(
    value: &mut Value,
    moved: &[(Vec<String>, String)],
    renames: &BTreeMap<String, String>,
    names: bool,
) {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|i| redirect_moved(i, moved, renames, false)),
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if names {
                    redirect_moved(child, moved, renames, false);
                } else if key == "$ref" {
                    let Some(segments) = child.as_str().and_then(|r| schema_pointer(r, renames))
                    else {
                        continue;
                    };
                    // Locations are moved deepest first, so the first prefix found is the longest.
                    let Some((location, name)) =
                        moved.iter().find(|(l, _)| segments.starts_with(l))
                    else {
                        continue;
                    };
                    let rest: String = segments[location.len()..]
                        .iter()
                        .map(|s| format!("/{}", s.replace('~', "~0").replace('/', "~1")))
                        .collect();
                    *child = format!("{SCHEMA_PREFIX}{name}{rest}").into();
                } else if !upgrade::LITERALS.contains(&key.as_str()) {
                    let names = upgrade::NAME_MAPS.contains(&key.as_str());
                    redirect_moved(child, moved, renames, names);
                }
            }
        }
        _ => {}
    }
}

fn sanitize_schema_names(doc: &mut Value) -> BTreeMap<String, String> {
    let mut renames = BTreeMap::new();
    let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") else {
        return renames;
    };
    let unsafe_names: Vec<String> = schemas
        .keys()
        .filter(|k| k.contains(is_unsafe_name_char))
        .cloned()
        .collect();
    for old in unsafe_names {
        let mut new: String = old
            .chars()
            .map(|c| if is_unsafe_name_char(c) { '_' } else { c })
            .collect();
        while schemas.contains_key(&new) {
            new.push('_');
        }
        if let Some(schema) = schemas.remove(&old) {
            schemas.insert(new.clone(), schema);
        }
        renames.insert(old, new);
    }
    renames
}

enum Rewritten {
    Ref(String),
    Inline(Value),
}

fn canonical_ref(
    reference: &str,
    root: &Value,
    renames: &BTreeMap<String, String>,
) -> Option<Rewritten> {
    let fragment = reference.strip_prefix('#')?;
    let decoded = percent_decode(fragment);
    if !decoded.starts_with('/') {
        return None;
    }
    if let Some(segments) = schema_pointer(reference, renames) {
        if segments.len() == 1 {
            let new = format!("{SCHEMA_PREFIX}{}", segments[0]);
            return (new != reference).then_some(Rewritten::Ref(new));
        }
        let target = lookup(root.pointer("/components/schemas")?, &segments)?;
        return Some(Rewritten::Inline(target.clone()));
    }
    (decoded != fragment).then(|| Rewritten::Ref(format!("#{decoded}")))
}

/// Rewrites the references under `value`, returning whether anything changed.
/// `names` tells that the keys of `value` are names (of properties, ...), not keywords.
fn canonicalize_value(
    value: &mut Value,
    root: &Value,
    renames: &BTreeMap<String, String>,
    names: bool,
) -> bool {
    let mut changed = false;
    match value {
        Value::Array(items) => {
            for item in items {
                changed |= canonicalize_value(item, root, renames, false);
            }
        }
        Value::Object(map) if names => {
            for child in map.values_mut() {
                changed |= canonicalize_value(child, root, renames, false);
            }
        }
        Value::Object(map) => {
            let rewritten = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| canonical_ref(r, root, renames));
            match rewritten {
                Some(Rewritten::Ref(new)) => {
                    map.insert("$ref".into(), new.into());
                    changed = true;
                }
                Some(Rewritten::Inline(target)) => {
                    let mut inlined = target;
                    if let Value::Object(inlined) = &mut inlined {
                        for (key, sibling) in std::mem::take(map) {
                            if key != "$ref" {
                                inlined.insert(key, sibling);
                            }
                        }
                    }
                    *value = inlined;
                    return true;
                }
                None => {}
            }
            let Value::Object(map) = value else {
                return changed;
            };
            for (key, child) in map.iter_mut() {
                match key.as_str() {
                    "example" | "examples" | "default" | "enum" | "const" | "$ref" => {}
                    "mapping" => {
                        let Value::Object(targets) = child else {
                            continue;
                        };
                        for target in targets.values_mut() {
                            let Some(reference) = target.as_str() else {
                                continue;
                            };
                            let new = match canonical_ref(reference, root, renames) {
                                Some(Rewritten::Ref(new)) => Some(new),
                                Some(Rewritten::Inline(_)) => None,
                                None => renames.get(reference).cloned(),
                            };
                            if let Some(new) = new {
                                *target = new.into();
                                changed = true;
                            }
                        }
                    }
                    _ => {
                        let names = upgrade::NAME_MAPS.contains(&key.as_str());
                        changed |= canonicalize_value(child, root, renames, names);
                    }
                }
            }
        }
        _ => {}
    }
    changed
}

/// Models a discriminator declared on a base schema as a union of its subtypes. The base's own
/// fields move to `{Base}Base`, which the subtypes' `allOf` parts reference instead, and the
/// base itself becomes the `oneOf` of its subtypes: those named by the discriminator mapping or,
/// for the ones left out of it, those that reference the base in `allOf`.
fn lower_base_discriminators(doc: &mut Value) {
    let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") else {
        return;
    };
    let bases: Vec<String> = schemas
        .iter()
        .filter(|(_, s)| {
            s.get("discriminator").is_some_and(Value::is_object)
                && !["oneOf", "anyOf", "x-perseid-union"]
                    .iter()
                    .any(|k| s.get(*k).is_some())
        })
        .map(|(name, _)| name.clone())
        .collect();
    let mut redirects = Vec::new();
    for base in bases {
        let snapshot = schemas.clone();
        let target = format!("{SCHEMA_PREFIX}{base}");
        let mut variants: Vec<String> = Vec::new();
        let mapped = snapshot[&base]
            .pointer("/discriminator/mapping")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.values().filter_map(Value::as_str));
        for mapped in mapped {
            let name = mapped.strip_prefix(SCHEMA_PREFIX).unwrap_or(mapped);
            if name != base && snapshot.contains_key(name) && !variants.iter().any(|v| v == name) {
                variants.push(name.to_owned());
            }
        }
        for (name, schema) in &snapshot {
            let extends = schema
                .get("allOf")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|p| p.get("$ref").and_then(Value::as_str) == Some(target.as_str()))
                });
            if extends && *name != base && !variants.contains(name) {
                variants.push(name.clone());
            }
        }
        if variants.is_empty() {
            continue;
        }
        let mut base_name = format!("{base}Base");
        while schemas.contains_key(&base_name) {
            base_name.push('_');
        }
        let mut fields = snapshot[&base].clone();
        let discriminator = fields
            .as_object_mut()
            .and_then(|m| m.remove("discriminator"))
            .unwrap_or_default();
        let property = discriminator["propertyName"].as_str().unwrap_or_default();
        let mut mapping = discriminator
            .get("mapping")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for variant in &variants {
            let is_mapped = mapping
                .values()
                .filter_map(Value::as_str)
                .any(|m| m.rsplit('/').next() == Some(variant.as_str()));
            if is_mapped {
                continue;
            }
            let values = discriminator_values(&snapshot[variant], property, |t| {
                snapshot.get(t.strip_prefix(SCHEMA_PREFIX)?)
            });
            if values.is_empty() {
                mapping.insert(variant.clone(), reference(variant)["$ref"].clone());
            }
        }
        let mut union = Map::new();
        for key in ["title", "description"] {
            if let Some(value) = fields.get(key) {
                union.insert(key.into(), value.clone());
            }
        }
        union.insert(
            "oneOf".into(),
            variants.iter().map(|v| reference(v)).collect(),
        );
        let mut discriminator = discriminator;
        if !mapping.is_empty() {
            discriminator["mapping"] = Value::Object(mapping);
        }
        union.insert("discriminator".into(), discriminator);
        schemas.insert(base_name.clone(), fields);
        schemas.insert(base.clone(), Value::Object(union));
        redirects.push((target, format!("{SCHEMA_PREFIX}{base_name}")));
    }
    for (from, to) in &redirects {
        redirect_all_of(doc, from, to, false);
    }
}

/// Points the `allOf` parts referencing `from` at `to`. `names` tells that the keys of `value`
/// are names (of properties, ...), not keywords.
fn redirect_all_of(value: &mut Value, from: &str, to: &str, names: bool) {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| redirect_all_of(v, from, to, false)),
        Value::Object(map) if names => map
            .values_mut()
            .for_each(|v| redirect_all_of(v, from, to, false)),
        Value::Object(map) => {
            if let Some(Value::Array(parts)) = map.get_mut("allOf") {
                for part in parts {
                    if part.get("$ref").and_then(Value::as_str) == Some(from)
                        && let Value::Object(part) = part
                    {
                        part.insert("$ref".into(), to.into());
                    }
                }
            }
            for (key, child) in map.iter_mut() {
                if !upgrade::LITERALS.contains(&key.as_str()) {
                    let names = upgrade::NAME_MAPS.contains(&key.as_str());
                    redirect_all_of(child, from, to, names);
                }
            }
        }
        _ => {}
    }
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
        explode_form_encodings(&mut body);
        if !is_empty_form(&body, root) {
            op.insert("requestBody".into(), body);
        }
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

/// Renames the schemas whose type name is not usable in the SDK: `Upload` becoming `UploadModel`
/// when the SDK has its own, `3DModel` becoming `N3dModel` as identifiers cannot start with a digit.
/// Returns the renames made, by schema name.
pub(super) fn rename_reserved_schemas(
    doc: &mut Value,
    reserved: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let Some(Value::Object(schemas)) = doc.pointer("/components/schemas") else {
        return BTreeMap::new();
    };
    let mut taken: BTreeSet<String> = schemas.keys().map(|n| n.to_upper_camel_case()).collect();
    let mut renames = BTreeMap::new();
    for name in schemas.keys() {
        let base = crate::reserved::safe_type_name(name, reserved);
        if base == name.to_upper_camel_case() {
            continue;
        }
        let mut candidate = base.clone();
        for n in 2.. {
            if taken.insert(candidate.clone()) {
                break;
            }
            candidate = format!("{base}{n}");
        }
        renames.insert(name.clone(), candidate);
    }
    if renames.is_empty() {
        return renames;
    }
    if let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") {
        let renamed = std::mem::take(schemas)
            .into_iter()
            .map(|(name, schema)| (renames.get(&name).cloned().unwrap_or(name), schema))
            .collect();
        *schemas = renamed;
    }
    let tags = renames.keys().map(|n| (n.clone(), n.clone())).collect();
    pin_implicit_tags(doc, &tags);
    rename_references(doc, &renames);
    renames
}

fn rename_references(value: &mut Value, renames: &BTreeMap<String, String>) {
    match value {
        Value::String(s) => {
            if let Some(new) = s.strip_prefix(SCHEMA_PREFIX).and_then(|n| renames.get(n)) {
                *s = format!("{SCHEMA_PREFIX}{new}");
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rename_references(v, renames)),
        Value::Object(map) => {
            if let Some(Value::Object(mapping)) = map
                .get_mut("discriminator")
                .and_then(|d| d.get_mut("mapping"))
            {
                for target in mapping.values_mut() {
                    if let Some(new) = target.as_str().and_then(|t| renames.get(t)) {
                        *target = new.clone().into();
                    }
                }
            }
            map.values_mut().for_each(|v| rename_references(v, renames));
        }
        _ => {}
    }
}

/// A form body allowing no property at all, which Stripe declares on its GET operations: it is
/// always empty, so the SDK has nothing to offer.
fn is_empty_form(body: &Value, root: &Value) -> bool {
    let Some(Value::Object(content)) = body.get("content") else {
        return false;
    };
    let Some(media) = content.get("application/x-www-form-urlencoded") else {
        return false;
    };
    let schema = resolve(root, media.get("schema").cloned().unwrap_or_default());
    content.len() == 1
        && schema.is_ok_and(|schema| {
            schema.get("additionalProperties") == Some(&Value::Bool(false))
                && schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_none_or(Map::is_empty)
        })
}

/// Spells out the `explode` of form body encodings, which defaults to true for `style: form`.
fn explode_form_encodings(body: &mut Value) {
    let encodings = body.pointer_mut("/content/application~1x-www-form-urlencoded/encoding");
    let Some(Value::Object(encodings)) = encodings else {
        return;
    };
    for encoding in encodings.values_mut().filter_map(Value::as_object_mut) {
        let form = encoding.get("style").is_none_or(|s| s == "form");
        encoding.entry("explode").or_insert(form.into());
    }
}

/// `{allOf: [{$ref: X}], required: [...]}` only tightens `X`, which SDKs type as `X` itself.
fn collapse_refinement(schema: &mut Value) {
    const REFINEMENTS: [&str; 8] = [
        "allOf",
        "required",
        "type",
        "description",
        "title",
        "example",
        "examples",
        "deprecated",
    ];
    let Some(map) = schema.as_object() else {
        return;
    };
    let Some([part]) = map
        .get("allOf")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    else {
        return;
    };
    let refines = map
        .keys()
        .all(|k| REFINEMENTS.contains(&k.as_str()) || k.starts_with("x-"));
    if !refines || part.get("$ref").is_none() || map.get("type").is_some_and(|t| t != "object") {
        return;
    }
    let mut collapsed = part.clone();
    if let (Some(description), Some(collapsed)) =
        (map.get("description"), collapsed.as_object_mut())
    {
        collapsed.insert("description".into(), description.clone());
    }
    *schema = collapsed;
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

/// Gives every `oneOf` without a discriminator the property its variants are told apart by, as
/// serde's tagged enums are written, and replaces a `oneOf` of one untagged inline variant by it.
fn infer_discriminators(doc: &mut Value) {
    let schemas = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for pointer in ["/components/schemas", "/paths"] {
        if let Some(Value::Object(children)) = doc.pointer_mut(pointer) {
            children
                .values_mut()
                .for_each(|c| infer_discriminator(c, &schemas));
        }
    }
}

fn infer_discriminator(value: &mut Value, schemas: &Map<String, Value>) {
    let map = match value {
        Value::Object(map) => map,
        Value::Array(items) => {
            return items
                .iter_mut()
                .for_each(|v| infer_discriminator(v, schemas));
        }
        _ => return,
    };
    for (key, child) in map.iter_mut() {
        match key.as_str() {
            "example" | "examples" | "default" | "enum" | "const" => {}
            "properties" | "patternProperties" => {
                for property in child
                    .as_object_mut()
                    .into_iter()
                    .flat_map(|p| p.values_mut())
                {
                    infer_discriminator(property, schemas);
                }
            }
            _ => infer_discriminator(child, schemas),
        }
    }
    // A `oneOf` next to properties is an adjacently tagged union, which the model reads as is.
    if ["discriminator", "x-perseid-union", "properties"]
        .iter()
        .any(|key| map.contains_key(*key))
    {
        return;
    }
    let Some(Value::Array(variants)) = map.get("oneOf") else {
        return;
    };
    if let Some(property) = implicit_discriminator(variants, schemas) {
        map.insert("discriminator".into(), json!({ "propertyName": property }));
    } else if let [Value::Object(variant)] = &variants[..]
        && !variant.contains_key("$ref")
        && !is_null_schema(&variants[0])
    {
        let variant = variant.clone();
        map.remove("oneOf");
        for (key, value) in variant {
            map.entry(key).or_insert(value);
        }
    }
}

/// The one property every variant requires with a single string value, distinct across them.
fn implicit_discriminator(variants: &[Value], schemas: &Map<String, Value>) -> Option<String> {
    let tags = variants
        .iter()
        .map(|v| tags(v, schemas, 0))
        .collect::<Option<Vec<_>>>()?;
    let mut candidates = tags.first()?.keys().filter(|property| {
        let values: BTreeSet<_> = tags.iter().filter_map(|t| t.get(*property)).collect();
        values.len() == tags.len()
    });
    let property = candidates.next()?;
    candidates.next().is_none().then(|| property.clone())
}

/// The required properties of an object schema that allow a single string, by name. `None` when
/// the schema is not an object.
fn tags(
    schema: &Value,
    schemas: &Map<String, Value>,
    depth: usize,
) -> Option<BTreeMap<String, String>> {
    let resolve = |s: &'_ Value| -> Option<Value> {
        match s.get("$ref").and_then(Value::as_str) {
            Some(target) => schemas.get(target.strip_prefix(SCHEMA_PREFIX)?).cloned(),
            None => Some(s.clone()),
        }
    };
    let schema = resolve(schema)?;
    if depth > 16 || is_null_schema(&schema) {
        return None;
    }
    is_object_shallow(&schema)?;
    let mut found = BTreeMap::new();
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        found.extend(tags(part, schemas, depth + 1)?);
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, property) in properties.into_iter().flatten() {
        let value = resolve(property).and_then(|p| match (p.get("const"), p.get("enum")) {
            (Some(Value::String(value)), _) => Some(value.clone()),
            (None, Some(Value::Array(values))) => match &values[..] {
                [Value::String(value)] => Some(value.clone()),
                _ => None,
            },
            _ => None,
        });
        if let Some(value) = value
            && required.contains(&name.as_str())
        {
            found.insert(name.clone(), value);
        }
    }
    Some(found)
}

/// The values a schema of strings contributes to an open enum, with their documentation.
struct Strings {
    /// Known values, empty for a plain string.
    values: Vec<(String, Option<String>)>,
    nullable: bool,
}

/// What `schema` allows when it only allows strings: a plain string, a string enum, a constant,
/// or a reference to one of those. `None` for anything else, such as a formatted string.
fn string_values(schema: &Value, schemas: &Map<String, Value>, depth: usize) -> Option<Strings> {
    const FOREIGN: [&str; 10] = [
        "format",
        "allOf",
        "oneOf",
        "anyOf",
        "not",
        "properties",
        "items",
        "additionalProperties",
        "patternProperties",
        "discriminator",
    ];
    let map = schema.as_object()?;
    if let Some(target) = map.get("$ref").and_then(Value::as_str) {
        let mut rest = map.clone();
        rest.remove("$ref");
        if depth > 8 || !is_annotation(&Value::Object(rest)) {
            return None;
        }
        let name = target.strip_prefix(SCHEMA_PREFIX)?;
        return string_values(schemas.get(name)?, schemas, depth + 1);
    }
    if FOREIGN.iter().any(|k| map.contains_key(*k)) {
        return None;
    }
    let mut nullable = false;
    match map.get("type") {
        None => {}
        Some(Value::String(t)) if t == "string" => {}
        Some(Value::Array(types)) => {
            for t in types {
                match t.as_str() {
                    Some("string") => {}
                    Some("null") => nullable = true,
                    _ => return None,
                }
            }
        }
        Some(_) => return None,
    }
    let doc = map
        .get("description")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
        .map(ToOwned::to_owned);
    let mut values = Vec::new();
    if let Some(Value::Array(items)) = map.get("enum") {
        let docs = map
            .get("x-enum-descriptions")
            .and_then(Value::as_array)
            .filter(|d| d.len() == items.len());
        for (i, item) in items.iter().enumerate() {
            match item {
                Value::Null => nullable = true,
                Value::String(value) => {
                    let described = docs
                        .and_then(|d| d[i].as_str())
                        .filter(|d| !d.is_empty())
                        .map(ToOwned::to_owned);
                    let single = doc.clone().filter(|_| items.len() == 1);
                    values.push((value.clone(), described.or(single)));
                }
                _ => return None,
            }
        }
    } else if let Some(constant) = map.get("const") {
        values.push((constant.as_str()?.to_owned(), doc));
    } else if map.get("type").is_none() {
        return None;
    }
    Some(Strings { values, nullable })
}

/// The `oneOf`/`anyOf` key of a plain union: no discriminator, no properties of its own, and
/// not opted out with `x-perseid-union: json`.
fn plain_union_key(map: &Map<String, Value>) -> Option<&'static str> {
    let key = match (map.contains_key("oneOf"), map.contains_key("anyOf")) {
        (true, false) => "oneOf",
        (false, true) => "anyOf",
        _ => return None,
    };
    let blocked = [
        "discriminator",
        "properties",
        "allOf",
        "enum",
        "const",
        "$ref",
    ];
    if blocked.iter().any(|k| map.contains_key(*k))
        || map.get("x-perseid-union").is_some_and(|v| v == "json")
    {
        return None;
    }
    Some(key)
}

/// Turns a union whose members are all strings (plain strings, string enums, constants,
/// references to string enums) into one open string enum: the union of the known values, with
/// the descriptions of constants as `x-enum-descriptions`. Enums keep values they do not know,
/// so a plain `string` member needs no type of its own. The schema keeps its name, and a
/// referenced enum stays a type of its own: the union carries a copy of its values.
fn open_enum(map: &mut Map<String, Value>, schemas: &Map<String, Value>) -> bool {
    let Some(key) = plain_union_key(map) else {
        return false;
    };
    if map.get("type").is_some_and(|t| t != "string") {
        return false;
    }
    let Some(Value::Array(variants)) = map.get(key) else {
        return false;
    };
    let mut nullable = false;
    let mut members = 0;
    let mut values: Vec<(String, Option<String>)> = Vec::new();
    for variant in variants {
        if is_null_schema(variant) {
            nullable = true;
            continue;
        }
        let Some(strings) = string_values(variant, schemas, 0) else {
            return false;
        };
        members += 1;
        nullable |= strings.nullable;
        for (value, doc) in strings.values {
            match values.iter_mut().find(|(known, _)| *known == value) {
                Some(known) => {
                    if known.1.is_none() {
                        known.1 = doc;
                    }
                }
                None => values.push((value, doc)),
            }
        }
    }
    // `oneOf: [X, null]` is a nullable `X`, not a union.
    if members < 2 {
        return false;
    }
    map.remove(key);
    map.insert(
        "type".into(),
        if nullable {
            json!(["string", "null"])
        } else {
            json!("string")
        },
    );
    if !values.is_empty() {
        map.insert(
            "enum".into(),
            Value::Array(values.iter().map(|(v, _)| json!(v)).collect()),
        );
    }
    if values.iter().any(|(_, doc)| doc.is_some()) {
        map.insert(
            "x-enum-descriptions".into(),
            Value::Array(
                values
                    .iter()
                    .map(|(_, doc)| json!(doc.as_deref().unwrap_or_default()))
                    .collect(),
            ),
        );
    }
    true
}

/// A union of `integer` and `number` is the `number`: every integer is one.
fn collapse_numbers(map: &mut Map<String, Value>) -> bool {
    let Some(key) = plain_union_key(map) else {
        return false;
    };
    if map.contains_key("type") {
        return false;
    }
    let Some(Value::Array(variants)) = map.get_mut(key) else {
        return false;
    };
    let kind = |v: &Value| -> Option<String> {
        const FOREIGN: [&str; 8] = [
            "enum",
            "const",
            "$ref",
            "oneOf",
            "anyOf",
            "allOf",
            "properties",
            "items",
        ];
        let object = v.as_object()?;
        if FOREIGN.iter().any(|k| object.contains_key(*k)) {
            return None;
        }
        object.get("type")?.as_str().map(ToOwned::to_owned)
    };
    let kinds: Vec<Option<String>> = variants.iter().map(kind).collect();
    let has = |t: &str| kinds.iter().any(|k| k.as_deref() == Some(t));
    if !(has("integer") && has("number")) {
        return false;
    }
    let mut kinds = kinds.into_iter();
    variants.retain(|_| kinds.next().flatten().as_deref() != Some("integer"));
    if variants.len() == 1 {
        let Value::Object(variant) = variants.remove(0) else {
            return true;
        };
        map.remove(key);
        for (k, v) in variant {
            map.entry(k).or_insert(v);
        }
    }
    true
}

/// Rewrites the unions of strings into open enums and those of `integer` and `number` into
/// `number`, until no union is left to rewrite (a union can reference another one).
fn open_enums(doc: &mut Value) {
    for _ in 0..4 {
        let schemas = doc
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        upgrade::each_schema(doc, &mut |map: &mut Map<String, Value>| {
            changed |= open_enum(map, &schemas) || collapse_numbers(map);
        });
        if !changed {
            break;
        }
    }
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

/// A schema of strings, numbers, booleans or arrays, such as Stripe's `enum: [""]`.
fn is_scalar(schema: &Value) -> bool {
    schema.get("enum").is_some()
        || schema
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| ["string", "integer", "number", "boolean", "array"].contains(&t))
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
        if let Some(Value::Array(params)) = op.get_mut("parameters") {
            for param in params.iter_mut().filter(|p| p["in"] == "query") {
                let name = format!("{id}_{}", param["name"].as_str().unwrap_or_default());
                if let Some(schema) = param.get_mut("schema") {
                    self.inline(schema, name);
                }
            }
        }
        for media in ["application/json", "application/x-www-form-urlencoded"] {
            if let Some(schema) = op.pointer_mut(&format!(
                "/requestBody/content/{}/schema",
                media.replace('/', "~1")
            )) {
                self.body(schema, format!("{id}_request"), false);
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
                self.body(schema, name, true);
            }
        }
    }

    /// Promotes the schema of a body. A request body drops `null`, which is not a value the SDK
    /// offers; a response body keeps it as `oneOf: [X, {type: null}]`, which the SDK returns
    /// as an optional.
    fn body(&mut self, schema: &mut Value, name: String, keep_null: bool) {
        let nullable = keep_null
            && (nullable_wrapper(schema).is_some()
                || schema
                    .get("type")
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|t| t == "null")));
        self.body_value(schema, name);
        if nullable {
            let inner = schema.take();
            *schema = json!({ "oneOf": [inner, { "type": "null" }] });
        }
    }

    fn body_value(&mut self, schema: &mut Value, name: String) {
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
            // A nullable component promoted here is typed as its object: the references carry
            // nullability.
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
        let own_properties = schema.get("discriminator").is_some() || has_properties(schema);
        for key in ["allOf", "oneOf", "anyOf"] {
            if let Some(Value::Array(parts)) = schema.get_mut(key) {
                // An object told apart from scalar variants by its JSON type needs a name.
                let scalar_or_object =
                    key != "allOf" && !own_properties && parts.iter().any(is_scalar);
                for part in parts {
                    if part.get("$ref").is_some() {
                        continue;
                    }
                    if scalar_or_object && is_structured(part) {
                        self.inline(part, format!("{parent}_object"));
                    } else {
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
            self.inline(&mut schema["allOf"][index], name);
            return collapse_refinement(schema);
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
    resolve: impl Fn(&str) -> Option<&'a Value> + Copy,
) -> Vec<String> {
    discriminator_values_at(variant, property, resolve, 8)
}

/// [discriminator_values], following `$ref` parts of `allOf` (a refined variant) at most `depth`
/// levels deep.
fn discriminator_values_at<'a>(
    variant: &'a Value,
    property: &str,
    resolve: impl Fn(&str) -> Option<&'a Value> + Copy,
    depth: usize,
) -> Vec<String> {
    let own = variant.get("properties").and_then(|p| p.get(property));
    let parts = variant.get("allOf").and_then(Value::as_array);
    let inherited = parts
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("properties")?.get(property));
    let values = own
        .into_iter()
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
        .find(|values| !values.is_empty());
    if let Some(values) = values {
        return values;
    }
    if depth == 0 {
        return vec![];
    }
    parts
        .into_iter()
        .flatten()
        .filter_map(|p| resolve(p.get("$ref")?.as_str()?))
        .map(|base| discriminator_values_at(base, property, resolve, depth - 1))
        .find(|values| !values.is_empty())
        .unwrap_or_default()
}

/// Moves the nullability of components to their references: `null` is dropped from a nullable
/// component, which SDKs type as its plain value, and the properties, items and map values
/// referencing it become `oneOf: [X, {type: null}]`, the nullable `X` fields read. Aliases of a nullable component
/// count as nullable too.
fn nullable_references(doc: &mut Value) {
    let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") else {
        return;
    };
    let mut nullable = BTreeSet::new();
    for (name, schema) in schemas.iter_mut() {
        if let Some((key, index)) = nullable_wrapper(schema) {
            let mut inner = schema[key][index].take();
            if let (Some(outer), Some(inner)) = (schema.as_object(), inner.as_object_mut()) {
                for (k, v) in outer.iter().filter(|(k, _)| *k != key) {
                    inner.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            *schema = inner;
            nullable.insert(format!("{SCHEMA_PREFIX}{name}"));
        } else if drop_null_type(schema) {
            nullable.insert(format!("{SCHEMA_PREFIX}{name}"));
        }
    }
    if nullable.is_empty() {
        return;
    }
    loop {
        let aliases: Vec<String> = schemas
            .iter()
            .filter(|(name, schema)| {
                schema.as_object().is_some_and(|s| s.len() == 1)
                    && schema
                        .get("$ref")
                        .and_then(Value::as_str)
                        .is_some_and(|r| nullable.contains(r))
                    && !nullable.contains(&format!("{SCHEMA_PREFIX}{name}"))
            })
            .map(|(name, _)| format!("{SCHEMA_PREFIX}{name}"))
            .collect();
        if aliases.is_empty() {
            break;
        }
        nullable.extend(aliases);
    }
    let mark = |slot: &mut Value| {
        if let Value::Object(slot) = slot
            && slot
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|r| nullable.contains(r))
        {
            let target = slot.remove("$ref").unwrap_or_default();
            slot.insert(
                "oneOf".into(),
                json!([{ "$ref": target }, { "type": "null" }]),
            );
        }
    };
    upgrade::each_schema(doc, &mut |map: &mut Map<String, Value>| {
        if let Some(Value::Object(properties)) = map.get_mut("properties") {
            properties.values_mut().for_each(mark);
        }
        for key in ["items", "additionalProperties"] {
            if let Some(slot) = map.get_mut(key) {
                mark(slot);
            }
        }
    });
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
    let union = parts
        .iter()
        .position(|p| tagged_union(p, schemas).is_some());
    let mut merged = Merged::default();
    for (index, part) in parts.iter().enumerate() {
        if Some(index) != union {
            merged.part(part, schemas)?;
        }
    }
    let mut own = map.clone();
    own.remove("allOf");
    merged.part(&Value::Object(own), schemas)?;
    if let Some(union) = union.and_then(|index| tagged_union(parts[index], schemas)) {
        if !merged.references.is_empty() {
            return None;
        }
        for key in ["oneOf", "discriminator"] {
            out.insert(key.into(), union[key].clone());
        }
    }
    merged.resolve_collisions(schemas);
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
            add_property(&mut self.properties, name, value);
        }
        add_required(&mut self.required, part.get("required"));
        Some(())
    }

    /// Replaces the embedded references by their fields when embedding would repeat a name: a
    /// property declared by several parts, or one named like an embedded schema (which SDKs
    /// declare as a field of that name).
    fn resolve_collisions(&mut self, schemas: &Map<String, Value>) {
        if self.references.is_empty() {
            return;
        }
        let mut inherited = Vec::new();
        let mut names = BTreeSet::new();
        for reference in &self.references {
            let mut fields = Map::new();
            let mut required = Vec::new();
            let mut stack = Vec::new();
            if let Some(name) = reference["$ref"]
                .as_str()
                .and_then(|r| r.strip_prefix(SCHEMA_PREFIX))
            {
                collect_fields(name, schemas, &mut stack, &mut fields, &mut required);
            }
            names.extend(stack.iter().map(|n| identifier_key(n)));
            inherited.push((fields, required));
        }
        let mut seen = BTreeSet::new();
        let mut collides = false;
        let inherited_names = inherited.iter().flat_map(|(fields, _)| fields.keys());
        for name in inherited_names.chain(self.properties.keys()) {
            let key = identifier_key(name);
            collides |= names.contains(&key) || !seen.insert(key);
        }
        if !collides {
            return;
        }
        let own = std::mem::take(&mut self.properties);
        let own_required = std::mem::take(&mut self.required);
        for (fields, required) in inherited {
            for (name, value) in &fields {
                add_property(&mut self.properties, name, value);
            }
            add_required(&mut self.required, Some(&Value::Array(required)));
        }
        for (name, value) in &own {
            add_property(&mut self.properties, name, value);
        }
        add_required(&mut self.required, Some(&Value::Array(own_required)));
        self.references.clear();
    }
}

/// A name as identifiers of any casing spell it, to find the names SDKs would declare twice.
fn identifier_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn add_required(required: &mut Vec<Value>, names: Option<&Value>) {
    for name in names.and_then(Value::as_array).into_iter().flatten() {
        if !required.contains(name) {
            required.push(name.clone());
        }
    }
}

fn add_property(properties: &mut Map<String, Value>, name: &str, value: &Value) {
    let merged = match properties.get(name) {
        Some(existing) => reconcile(name, existing, value),
        None => value.clone(),
    };
    properties.insert(name.to_owned(), merged);
}

/// The fields and required names of the schema `name` and of everything it composes, recording
/// the names visited in `stack`.
fn collect_fields(
    name: &str,
    schemas: &Map<String, Value>,
    stack: &mut Vec<String>,
    fields: &mut Map<String, Value>,
    required: &mut Vec<Value>,
) {
    if stack.iter().any(|n| n == name) || stack.len() > 32 {
        return;
    }
    let Some(schema) = schemas.get(name) else {
        return;
    };
    stack.push(name.to_owned());
    collect_schema_fields(schema, schemas, stack, fields, required);
}

fn collect_schema_fields(
    schema: &Value,
    schemas: &Map<String, Value>,
    stack: &mut Vec<String>,
    fields: &mut Map<String, Value>,
    required: &mut Vec<Value>,
) {
    for part in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match part.get("$ref").and_then(Value::as_str) {
            Some(target) => {
                if let Some(name) = target.strip_prefix(SCHEMA_PREFIX) {
                    collect_fields(name, schemas, stack, fields, required);
                }
            }
            None => collect_schema_fields(part, schemas, stack, fields, required),
        }
    }
    for (name, value) in schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        add_property(fields, name, value);
    }
    add_required(required, schema.get("required"));
}

/// Keys that describe a value rather than constrain it.
const DOCUMENTATION: [&str; 7] = [
    "description",
    "title",
    "example",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// Whether `narrow` accepts only values `wide` accepts, judging by the same type and by
/// constraints it adds or tightens.
fn refines(narrow: &Value, wide: &Value) -> bool {
    let (Some(narrow), Some(wide)) = (narrow.as_object(), wide.as_object()) else {
        return false;
    };
    if narrow.contains_key("$ref") || wide.contains_key("$ref") {
        return narrow.get("$ref") == wide.get("$ref") && narrow.get("$ref").is_some();
    }
    wide.iter()
        .filter(|(key, _)| !DOCUMENTATION.contains(&key.as_str()))
        .all(|(key, value)| match (key.as_str(), narrow.get(key)) {
            (_, None) => false,
            ("enum", Some(Value::Array(own))) => value
                .as_array()
                .is_some_and(|all| own.iter().all(|v| all.contains(v))),
            (_, Some(own)) => own == value,
        })
}

/// The one schema two `allOf` parts declare for the same property: the narrower of them, or an
/// untyped one when neither refines the other.
fn reconcile(name: &str, first: &Value, second: &Value) -> Value {
    if first == second || refines(second, first) {
        return second.clone();
    }
    if refines(first, second) {
        return first.clone();
    }
    tracing::warn!(
        property = name,
        "`allOf` parts declare incompatible schemas for a property, so it is typed as an untyped JSON value"
    );
    json!({})
}

/// The discriminated union an `allOf` part is, whose variants then carry the other parts' fields.
fn tagged_union<'a>(part: &'a Value, schemas: &'a Map<String, Value>) -> Option<&'a Value> {
    let union = match part.get("$ref").and_then(Value::as_str) {
        Some(target) => schemas.get(target.strip_prefix(SCHEMA_PREFIX)?)?,
        None => part,
    };
    (union.get("oneOf").is_some_and(Value::is_array)
        && union.get("discriminator").is_some()
        && !has_properties(union))
    .then_some(union)
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

    #[test]
    fn binary_content_media_types_are_files() {
        let mut doc = json!({ "properties": {
            "file": { "type": "string", "contentMediaType": "image/png" },
            "text": { "type": "string", "contentMediaType": "text/plain" },
            "b64": { "type": "string", "contentMediaType": "image/png", "contentEncoding": "base64" } } });
        upgrade::each_schema(&mut doc, &mut binary_content);
        assert_eq!(doc["properties"]["file"]["format"], "binary");
        assert!(doc["properties"]["text"].get("format").is_none());
        assert!(doc["properties"]["b64"].get("format").is_none());
    }

    #[test]
    fn dropped_formats_leave_examples_alone() {
        let mut doc = json!({ "properties": {
            "id": { "type": "string", "format": "uuid", "example": { "format": "uuid" } },
            "at": { "type": "string", "format": "date-time" } } });
        drop_format(&mut doc, "uuid");
        assert_eq!(
            doc["properties"]["id"],
            json!({ "type": "string", "example": { "format": "uuid" } })
        );
        assert_eq!(doc["properties"]["at"]["format"], "date-time");
    }

    fn op(responses: Value) -> Value {
        json!({ "operationId": "op", "responses": responses })
    }

    #[test]
    fn form_bodies_allowing_no_property_are_dropped() {
        let empty = json!({ "type": "object", "properties": {}, "additionalProperties": false });
        let doc = normalized(json!({
            "paths": { "/v1/customers": { "get": {
                "operationId": "GetCustomers",
                "requestBody": { "content": { "application/x-www-form-urlencoded": {
                    "schema": empty } } },
                "responses": {}
            } } }
        }));
        assert!(
            doc["paths"]["/v1/customers"]["get"]
                .get("requestBody")
                .is_none()
        );
    }

    #[test]
    fn refinements_of_a_reference_are_the_reference() {
        let refined = json!({ "type": "object", "description": "New app.",
            "allOf": [{ "$ref": "#/components/schemas/App" }], "required": ["name"] });
        let doc = normalized(json!({
            "paths": { "/apps": { "post": {
                "operationId": "create",
                "requestBody": { "content": { "application/json": { "schema": refined } } },
                "responses": {}
            } } },
            "components": { "schemas": { "App": {
                "type": "object", "properties": { "name": { "type": "string" } } } } }
        }));
        assert_eq!(
            doc.pointer("/paths/~1apps/post/requestBody/content/application~1json/schema"),
            Some(&json!({ "$ref": "#/components/schemas/App", "description": "New app." }))
        );
    }

    #[test]
    fn inline_query_objects_are_named_and_form_encodings_explode_by_default() {
        let range = json!({ "type": "object", "properties": { "gt": { "type": "integer" } } });
        let doc = normalized(json!({
            "paths": { "/logs": { "post": {
                "operationId": "list_logs",
                "parameters": [{ "name": "effective_at", "in": "query", "schema": range }],
                "requestBody": { "content": { "application/x-www-form-urlencoded": {
                    "schema": { "type": "object", "properties": { "a": { "type": "string" } } },
                    "encoding": { "a": { "style": "form" }, "b": { "style": "deepObject" } }
                } } },
                "responses": {}
            } } }
        }));
        let op = &doc["paths"]["/logs"]["post"];
        assert_eq!(
            op["parameters"][0]["schema"],
            json!({ "$ref": "#/components/schemas/ListLogsEffectiveAt" })
        );
        let encoding =
            &op["requestBody"]["content"]["application/x-www-form-urlencoded"]["encoding"];
        assert_eq!(encoding["a"]["explode"], true);
        assert_eq!(encoding["b"]["explode"], false);
    }

    #[test]
    fn reserved_schema_names_are_renamed_with_their_references() {
        let mut doc = json!({
            "paths": { "/uploads": { "get": { "operationId": "op", "responses": { "200": {
                "description": "", "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/upload" } } } } } } } },
            "components": { "schemas": {
                "upload": { "type": "object" },
                "UploadModel": { "type": "object" },
                "Event": { "oneOf": [{ "$ref": "#/components/schemas/upload" }],
                    "discriminator": { "propertyName": "type", "mapping": { "u": "upload" } } }
            } }
        });
        rename_reserved_schemas(&mut doc, &BTreeSet::from(["Upload".to_owned()]));
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        assert!(schemas.contains_key("UploadModel2") && !schemas.contains_key("upload"));
        assert_eq!(
            doc.pointer("/paths/~1uploads/get/responses/200/content/application~1json/schema/$ref"),
            Some(&json!("#/components/schemas/UploadModel2"))
        );
        assert_eq!(
            schemas["Event"]["discriminator"]["mapping"]["u"],
            json!("UploadModel2")
        );
    }

    #[test]
    fn renamed_variants_keep_their_implicit_discriminator_tag() {
        let mut doc = json!({
            "openapi": "3.1.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": { "schemas": {
                "Result": { "type": "object" },
                "Odd/Name": { "type": "object" },
                "Tagged": { "type": "object", "properties": {
                    "kind": { "type": "string", "enum": ["tagged"] } } },
                "Event": { "oneOf": [
                    { "$ref": "#/components/schemas/Result" },
                    { "$ref": "#/components/schemas/Odd~1Name" },
                    { "$ref": "#/components/schemas/Tagged" }],
                    "discriminator": { "propertyName": "kind" } }
            } }
        });
        normalize(&mut doc).unwrap();
        rename_reserved_schemas(&mut doc, &BTreeSet::from(["Result".to_owned()]));
        let mapping = &doc["components"]["schemas"]["Event"]["discriminator"]["mapping"];
        assert_eq!(mapping["Result"], json!("#/components/schemas/ResultModel"));
        assert_eq!(mapping["Odd/Name"], json!("#/components/schemas/Odd_Name"));
        assert_eq!(mapping.get("Tagged"), None);
    }

    #[test]
    fn schema_names_starting_with_a_digit_get_a_prefix() {
        let mut doc = json!({
            "paths": { "/m": { "get": { "operationId": "op", "responses": { "200": {
                "description": "", "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/3DModel" } } } } } } } },
            "components": { "schemas": {
                "3DModel": { "type": "object" },
                "N3dModel": { "type": "object" },
                "": { "type": "object" }
            } }
        });
        rename_reserved_schemas(&mut doc, &BTreeSet::new());
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        assert!(schemas.contains_key("N3dModel2") && schemas.contains_key("N3dModel"));
        assert!(schemas.contains_key("Schema") && !schemas.contains_key("3DModel"));
        assert_eq!(
            doc.pointer("/paths/~1m/get/responses/200/content/application~1json/schema/$ref"),
            Some(&json!("#/components/schemas/N3dModel2"))
        );
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
    fn refined_variants_are_mapped_from_the_discriminator_enum_of_their_base() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Grader": {
                "oneOf": [{ "$ref": "#/components/schemas/EvalStringCheck" }],
                "discriminator": { "propertyName": "type" }
            },
            "StringCheck": { "properties": { "type": { "enum": ["string_check"] } } },
            "EvalStringCheck": { "allOf": [
                { "$ref": "#/components/schemas/StringCheck" },
                { "type": "object", "description": "Refined." }
            ] }
        } } }));
        assert_eq!(
            doc["components"]["schemas"]["Grader"]["discriminator"]["mapping"],
            json!({ "string_check": "#/components/schemas/EvalStringCheck" })
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
    fn variants_requiring_one_distinct_constant_infer_their_discriminator() {
        let tagged = |kind: &str| {
            json!({ "type": "object", "required": ["kind", "id"], "properties": {
                "kind": { "type": "string", "enum": [kind] }, "id": { "type": "string", "enum": ["x"] } } })
        };
        let doc = normalized(json!({ "components": { "schemas": {
            "Event": { "oneOf": [
                tagged("created"),
                { "allOf": [{ "$ref": "#/components/schemas/Base" }, tagged("deleted")] }
            ] },
            "Base": { "type": "object", "properties": { "at": { "type": "string" } } },
            "Untagged": { "oneOf": [tagged("a"), { "type": "object", "properties": { "kind": { "const": "b" } } }] },
            "Shared": { "oneOf": [tagged("a"), tagged("a")] },
            "Opted": { "oneOf": [tagged("a"), tagged("b")], "x-perseid-union": "json" },
            "Adjacent": {
                "properties": { "id": { "type": "string" } },
                "oneOf": [tagged("a"), tagged("b")]
            }
        } } }));
        let s = &doc["components"]["schemas"];
        assert_eq!(
            s["Event"]["discriminator"]["mapping"],
            json!({
                "created": "#/components/schemas/EventCreatedVariant",
                "deleted": "#/components/schemas/EventDeletedVariant"
            })
        );
        assert!(s["Untagged"].get("discriminator").is_none());
        assert!(s["Shared"].get("discriminator").is_none());
        assert!(s["Opted"].get("discriminator").is_none());
        assert!(s["Adjacent"].get("discriminator").is_none());
    }

    #[test]
    fn a_union_of_one_untagged_inline_variant_is_that_variant() {
        let doc = normalized(json!({ "components": { "schemas": { "Strategy": {
            "description": "d",
            "oneOf": [{ "type": "object", "required": ["Suffix"], "properties": { "Suffix": { "type": "string" } } }]
        } } } }));
        assert_eq!(
            doc["components"]["schemas"]["Strategy"],
            json!({ "description": "d", "type": "object", "required": ["Suffix"],
                    "properties": { "Suffix": { "type": "string" } } })
        );
    }

    #[test]
    fn all_of_over_a_tagged_union_gives_its_variants_the_other_fields() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Kind": {
                "oneOf": [{ "$ref": "#/components/schemas/Static" }],
                "discriminator": { "propertyName": "type" }
            },
            "Static": { "type": "object", "properties": { "type": { "enum": ["static"] } } },
            "Label": { "allOf": [
                { "$ref": "#/components/schemas/Kind" },
                { "type": "object", "required": ["id"], "properties": { "id": { "type": "string" } } }
            ] }
        } } }));
        let label = &doc["components"]["schemas"]["Label"];
        assert!(label.get("allOf").is_none());
        assert_eq!(label["required"], json!(["id"]));
        assert_eq!(
            label["oneOf"],
            json!([{ "$ref": "#/components/schemas/Static" }])
        );
        assert_eq!(label["discriminator"]["propertyName"], "type");
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

    #[test]
    fn all_of_properties_named_like_an_embedded_schema_are_flattened() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Owner": { "type": "object", "properties": { "id": { "type": "string" } } },
            "Child": { "allOf": [
                { "$ref": "#/components/schemas/Owner" },
                { "type": "object", "properties": { "owner": { "type": "string" } } }
            ] }
        } } }));
        let child = &doc["components"]["schemas"]["Child"];
        assert!(child.get("allOf").is_none());
        assert_eq!(child["properties"]["id"], json!({ "type": "string" }));
        assert_eq!(child["properties"]["owner"], json!({ "type": "string" }));
    }

    #[test]
    fn all_of_parts_redeclaring_a_property_keep_the_narrower_schema() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Base": { "type": "object", "properties": {
                "kind": { "type": "string" }, "n": { "type": "integer" } } },
            "Child": { "allOf": [
                { "$ref": "#/components/schemas/Base" },
                { "type": "object", "properties": {
                    "kind": { "type": "string", "enum": ["a"] }, "n": { "type": "string" } } }
            ] }
        } } }));
        let child = &doc["components"]["schemas"]["Child"];
        assert!(child.get("allOf").is_none());
        assert_eq!(
            child["properties"]["kind"],
            json!({ "type": "string", "enum": ["a"] })
        );
        assert_eq!(child["properties"]["n"], json!({}));
    }

    #[test]
    fn a_discriminator_on_a_base_makes_it_a_union_of_its_subtypes() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Pet": {
                "type": "object", "required": ["petType"],
                "properties": { "petType": { "type": "string" } },
                "discriminator": { "propertyName": "petType",
                    "mapping": { "cat": "#/components/schemas/Cat" } }
            },
            "Cat": { "allOf": [
                { "$ref": "#/components/schemas/Pet" },
                { "type": "object", "properties": { "meow": { "type": "boolean" } } }
            ] },
            "Dog": { "allOf": [
                { "$ref": "#/components/schemas/Pet" },
                { "type": "object", "properties": { "bark": { "type": "boolean" } } }
            ] }
        } } }));
        let s = &doc["components"]["schemas"];
        assert_eq!(
            s["Pet"]["oneOf"],
            json!([{ "$ref": "#/components/schemas/Cat" }, { "$ref": "#/components/schemas/Dog" }])
        );
        assert_eq!(
            s["Pet"]["discriminator"]["mapping"]["cat"],
            "#/components/schemas/Cat"
        );
        assert_eq!(
            s["Pet"]["discriminator"]["mapping"]["Dog"],
            "#/components/schemas/Dog"
        );
        assert!(s["PetBase"].get("discriminator").is_none());
        assert_eq!(
            s["Cat"]["allOf"],
            json!([{ "$ref": "#/components/schemas/PetBase" }])
        );
        assert_eq!(s["Cat"]["properties"]["meow"], json!({ "type": "boolean" }));
    }

    #[test]
    fn json_pointer_escapes_and_nested_locations_resolve() {
        let doc = normalized(json!({ "components": { "schemas": {
            "a/b": { "type": "object", "properties": { "v": { "type": "string" } } },
            "Holder": { "type": "object", "properties": {
                "x": { "$ref": "#/components/schemas/a~1b" },
                "y": { "$ref": "#/components/schemas/a~1b/properties/v" },
                "z": { "$ref": "#/components/schemas/Sp%20ace" }
            } },
            "Sp ace": { "type": "object", "properties": { "w": { "type": "integer" } } }
        } } }));
        let s = &doc["components"]["schemas"];
        assert!(s.get("a_b").is_some());
        let holder = &s["Holder"]["properties"];
        assert_eq!(holder["x"]["$ref"], "#/components/schemas/a_b");
        assert_eq!(holder["y"], json!({ "type": "string" }));
        assert_eq!(holder["z"]["$ref"], "#/components/schemas/Sp ace");
    }

    #[test]
    fn nested_locations_resolve_under_properties_named_like_keywords() {
        let doc = normalized(json!({ "components": { "schemas": {
            "Other": { "type": "object", "properties": { "inner": { "type": "string" } } },
            "Thing": { "type": "object", "properties": {
                "default": { "$ref": "#/components/schemas/Other/properties/inner" },
                "enum": { "$ref": "#/components/schemas/Other/properties/inner" }
            } }
        } } }));
        let thing = &doc["components"]["schemas"]["Thing"]["properties"];
        assert_eq!(thing["default"], json!({ "type": "string" }));
        assert_eq!(thing["enum"], json!({ "type": "string" }));
    }

    #[test]
    fn recursive_nested_locations_become_components() {
        let doc = normalized(json!({ "components": { "schemas": {
            "A": { "type": "object", "properties": { "x": { "type": "object", "properties": {
                "p1": { "$ref": "#/components/schemas/A/properties/x" },
                "p2": { "$ref": "#/components/schemas/A/properties/x" },
                "p3": { "$ref": "#/components/schemas/A/properties/x" }
            } } } },
            "List": { "type": "object", "properties": { "node": { "type": "object", "properties": {
                "value": { "type": "string" },
                "next": { "$ref": "#/components/schemas/List/properties/node" }
            } } } },
            "Value": { "$ref": "#/components/schemas/List/properties/node/properties/value" }
        } } }));
        let s = &doc["components"]["schemas"];
        assert_eq!(s["A"]["properties"]["x"]["$ref"], "#/components/schemas/AX");
        assert_eq!(
            s["AX"]["properties"]["p3"]["$ref"],
            "#/components/schemas/AX"
        );
        assert_eq!(
            s["List"]["properties"]["node"]["$ref"],
            "#/components/schemas/ListNode"
        );
        assert_eq!(
            s["ListNode"]["properties"]["next"]["$ref"],
            "#/components/schemas/ListNode"
        );
        assert_eq!(s["Value"], json!({ "type": "string" }));
    }

    #[test]
    fn references_to_nullable_components_are_nullable() {
        let s = schemas_of(json!({
            "Count": { "type": ["integer", "null"] },
            "Name": { "oneOf": [{ "type": "string" }, { "type": "null" }], "description": "n" },
            "Obj": { "type": ["object", "null"], "properties": { "a": { "type": "string" } } },
            "Alias": { "$ref": "#/components/schemas/Count" },
            "Thing": { "type": "object", "properties": {
                "count": { "$ref": "#/components/schemas/Count", "description": "c" },
                "name": { "$ref": "#/components/schemas/Name" },
                "objs": { "type": "array", "items": { "$ref": "#/components/schemas/Obj" } },
                "alias": { "$ref": "#/components/schemas/Alias" }
            } }
        }));
        assert_eq!(s["Count"], json!({ "type": "integer" }));
        assert_eq!(s["Name"], json!({ "type": "string", "description": "n" }));
        assert_eq!(s["Obj"]["type"], "object");
        let nullable =
            |name: &str| json!([{ "$ref": format!("{SCHEMA_PREFIX}{name}") }, { "type": "null" }]);
        let thing = &s["Thing"]["properties"];
        assert_eq!(thing["count"]["oneOf"], nullable("Count"));
        assert_eq!(thing["count"]["description"], "c");
        assert_eq!(thing["name"]["oneOf"], nullable("Name"));
        assert_eq!(thing["objs"]["items"]["oneOf"], nullable("Obj"));
        assert_eq!(thing["alias"]["oneOf"], nullable("Alias"));
    }

    fn schemas_of(doc: Value) -> Value {
        normalized(json!({ "paths": {}, "components": { "schemas": doc } }))["components"]
            ["schemas"]
            .clone()
    }

    #[test]
    fn unions_of_strings_become_one_open_enum() {
        let s = schemas_of(json!({
            "Model": { "description": "d", "anyOf": [
                { "type": "string" },
                { "type": "string", "enum": ["a", "b"] },
                { "$ref": "#/components/schemas/Known" },
                { "type": "null" }
            ] },
            "Known": { "type": "string", "enum": ["b", "c"] }
        }));
        assert_eq!(
            s["Model"],
            json!({ "description": "d", "type": "string", "enum": ["a", "b", "c"] })
        );
        // The referenced enum stays a type of its own.
        assert_eq!(s["Known"], json!({ "type": "string", "enum": ["b", "c"] }));
    }

    #[test]
    fn unions_of_constants_are_an_enum_with_value_docs() {
        let s = schemas_of(json!({
            "Mode": { "oneOf": [
                { "const": "fast", "description": "Quick" },
                { "type": "string", "const": "slow" },
                { "const": "auto", "description": "Picks" }
            ] }
        }));
        assert_eq!(
            s["Mode"],
            json!({
                "type": "string",
                "enum": ["fast", "slow", "auto"],
                "x-enum-descriptions": ["Quick", "", "Picks"]
            })
        );
    }

    #[test]
    fn union_enums_referencing_unions_are_merged_too() {
        let s = schemas_of(json!({
            "Outer": { "anyOf": [{ "type": "string" }, { "$ref": "#/components/schemas/Inner" }] },
            "Inner": { "oneOf": [{ "const": "x" }, { "const": "y" }] }
        }));
        assert_eq!(s["Outer"], json!({ "type": "string", "enum": ["x", "y"] }));
    }

    #[test]
    fn unions_with_other_members_or_formats_are_kept() {
        let doc = json!({
            "Dated": { "anyOf": [{ "type": "string", "format": "date-time" }, { "type": "string" }] },
            "Mixed": { "anyOf": [{ "type": "string" }, { "type": "integer" }] },
            "Wrapped": { "type": "object", "properties": { "w": {
                "anyOf": [{ "type": "string", "enum": ["a", "b"] }, { "type": "null" }] } } },
            "Tagged": { "oneOf": [{ "const": "a" }, { "const": "b" }], "x-perseid-union": "json" }
        });
        assert_eq!(schemas_of(doc.clone()), doc);
    }

    #[test]
    fn integer_and_number_in_a_union_collapse_to_number() {
        let s = schemas_of(json!({
            "Both": { "description": "d", "oneOf": [{ "type": "integer" }, { "type": "number" }] },
            "Nullable": { "anyOf": [{ "type": "integer" }, { "type": "number" }, { "type": "null" }] },
            "More": { "anyOf": [{ "type": "integer" }, { "type": "number" }, { "type": "string" }] }
        }));
        assert_eq!(s["Both"], json!({ "description": "d", "type": "number" }));
        // The references to a nullable component carry its nullability.
        assert_eq!(s["Nullable"], json!({ "type": "number" }));
        assert_eq!(
            s["More"],
            json!({ "anyOf": [{ "type": "number" }, { "type": "string" }] })
        );
    }

    #[test]
    fn open_enums_are_found_in_properties_and_parameters() {
        let doc = normalized(json!({
            "paths": { "/x": { "get": {
                "operationId": "op",
                "parameters": [{ "name": "m", "in": "query", "schema": {
                    "anyOf": [{ "type": "string" }, { "const": "a" }, { "const": "b" }]
                } }],
                "responses": {}
            } } },
            "components": { "schemas": { "Holder": { "type": "object", "properties": {
                "enum": { "type": "string" },
                "model": { "anyOf": [{ "type": "string" }, { "enum": ["a", "b"] }] }
            } } } }
        }));
        let model = &doc["components"]["schemas"]["Holder"]["properties"]["model"];
        assert_eq!(model, &json!({ "type": "string", "enum": ["a", "b"] }));
        let param = &doc["paths"]["/x"]["get"]["parameters"][0]["schema"];
        assert_eq!(param, &json!({ "type": "string", "enum": ["a", "b"] }));
    }
}
