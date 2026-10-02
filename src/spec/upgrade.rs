use anyhow::{Result, bail};
use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

const SCHEMA_MAPS: [&str; 4] = ["properties", "patternProperties", "$defs", "definitions"];
const SCHEMA_VALUES: [&str; 6] = ["items", "additionalProperties", "not", "if", "then", "else"];
const SCHEMA_LISTS: [&str; 4] = ["allOf", "anyOf", "oneOf", "prefixItems"];
const ANNOTATIONS: [&str; 7] = [
    "description",
    "title",
    "default",
    "example",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// Upgrades an OpenAPI 3.0.x document to 3.1 in place, and rejects anything that is not 3.x.
pub(super) fn to_3_1(doc: &mut Value) -> Result<()> {
    if doc.get("swagger").is_some() {
        bail!(
            "Swagger 2.0 is not supported, convert it to OpenAPI 3.x first \
             (for example with `npx swagger2openapi`)"
        );
    }
    let version = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
    if version.starts_with("3.1") || version.starts_with("3.2") {
        let is_3_2 = version.starts_with("3.2");
        doc["openapi"] = json!("3.1.0");
        if is_3_2 {
            drop_3_2_operations(doc);
        }
        return Ok(());
    }
    if !version.starts_with("3.0") {
        if version.is_empty() {
            bail!("the document has no `openapi` version: perseid reads OpenAPI 3.0, 3.1 and 3.2");
        }
        bail!("OpenAPI {version} is not supported; perseid reads 3.0, 3.1 and 3.2");
    }
    doc["openapi"] = json!("3.1.0");
    walk(doc);
    Ok(())
}

/// Drops what 3.2 adds to a path item and 3.1 has no place for: the QUERY method and
/// `additionalOperations`, with a warning.
fn drop_3_2_operations(doc: &mut Value) {
    for section in ["paths", "webhooks"] {
        let Some(items) = doc.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        for (path, item) in items.iter_mut() {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            if item.remove("query").is_some() {
                eprintln!("warning: QUERY {path} skipped: the QUERY method is not supported");
            }
            if item.remove("additionalOperations").is_some() {
                eprintln!("warning: additionalOperations of {path} skipped: not supported");
            }
        }
    }
}

/// Replaces boolean schemas by schema objects: `true` accepts any value, so it is `{}`; `false`
/// accepts none, which no SDK type expresses, so it is warned about and read as `{}` too.
pub(super) fn boolean_schemas(doc: &mut Value) {
    if let Some(schemas) = doc
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    {
        for (name, s) in schemas.iter_mut() {
            boolean_schema(s, name);
        }
    }
    walk_slots(doc);
}

fn walk_slots(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key.starts_with("x-") || key.starts_with("example") {
                    continue;
                }
                if key == "schema" {
                    boolean_schema(child, "(inline)");
                }
                walk_slots(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(walk_slots),
        _ => {}
    }
}

/// Keys of a schema holding named schemas (or, for `responses`, named responses) rather than
/// being a schema themselves, so that a property called `enum` or a response `default` is not
/// mistaken for a keyword.
const NAME_MAPS: [&str; 10] = [
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
    "schemas",
    "responses",
    "headers",
    "requestBodies",
    "content",
];
/// Keys holding values of the API rather than schemas.
const LITERALS: [&str; 5] = ["example", "examples", "default", "enum", "const"];

/// Calls `visit` on every JSON object of the document that can be a schema, children first.
pub(super) fn each_schema(doc: &mut Value, visit: &mut dyn FnMut(&mut Map<String, Value>)) {
    each_node(doc, false, visit);
}

fn each_node(value: &mut Value, names: bool, visit: &mut dyn FnMut(&mut Map<String, Value>)) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if !names && (LITERALS.contains(&key.as_str()) || key.starts_with("x-")) {
                    continue;
                }
                each_node(child, !names && NAME_MAPS.contains(&key.as_str()), visit);
            }
            if !names {
                visit(map);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| each_node(i, false, visit)),
        _ => {}
    }
}

/// Rewrites `type` arrays of several non-null types (`[string, integer, null]`) into
/// `oneOf`, one variant per type that keeps the keywords applying to it, so that the union
/// machinery types them. `integer` next to `number` is `number`. Unions of one type and `null`
/// are left to the `type` array.
pub(super) fn type_unions(doc: &mut Value) {
    each_schema(doc, &mut type_union);
}

/// The keywords that only constrain values of one JSON type.
fn keyword_types(key: &str, value: &Value) -> Option<&'static [&'static str]> {
    const NUMERIC: [&str; 11] = [
        "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "uint", "float",
        "double",
    ];
    Some(match key {
        "format" if value.as_str().is_some_and(|f| NUMERIC.contains(&f)) => &["integer", "number"],
        "format" | "pattern" | "minLength" | "maxLength" | "contentEncoding"
        | "contentMediaType" => &["string"],
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
            &["integer", "number"]
        }
        "items" | "prefixItems" | "minItems" | "maxItems" | "uniqueItems" | "contains"
        | "minContains" | "maxContains" | "unevaluatedItems" => &["array"],
        "properties"
        | "required"
        | "additionalProperties"
        | "patternProperties"
        | "minProperties"
        | "maxProperties"
        | "propertyNames"
        | "unevaluatedProperties"
        | "dependentRequired" => &["object"],
        _ => return None,
    })
}

fn is_of_type(value: &Value, ty: &str) -> bool {
    match ty {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn type_union(map: &mut Map<String, Value>) {
    const TYPES: [&str; 7] = [
        "string", "integer", "number", "boolean", "array", "object", "null",
    ];
    let Some(Value::Array(declared)) = map.get("type") else {
        return;
    };
    let mut types: Vec<String> = Vec::new();
    for t in declared {
        match t.as_str() {
            Some(t) if TYPES.contains(&t) => {
                if !types.iter().any(|x| x == t) {
                    types.push(t.to_owned());
                }
            }
            _ => return,
        }
    }
    let nullable = types.iter().any(|t| t == "null");
    types.retain(|t| t != "null");
    let integer_in_number = types.iter().any(|t| t == "number");
    if integer_in_number {
        types.retain(|t| t != "integer");
    }
    if types.len() <= 1 {
        // `[integer, number]` is the one `number`; other single types stay as they are.
        if integer_in_number && let Some(t) = types.pop() {
            map.insert(
                "type".into(),
                if nullable {
                    json!([t, "null"])
                } else {
                    json!(t)
                },
            );
        }
        return;
    }
    if ["oneOf", "anyOf", "allOf", "$ref"]
        .iter()
        .any(|k| map.contains_key(*k))
    {
        return;
    }
    map.remove("type");
    let mut variants: Vec<Map<String, Value>> = types
        .iter()
        .map(|t| {
            let mut variant = Map::new();
            variant.insert("type".into(), json!(t));
            variant
        })
        .collect();
    for key in map.keys().cloned().collect::<Vec<_>>() {
        let value = &map[&key];
        if key == "enum" || key == "const" {
            // Each value goes to the variant of its type.
            let values: Vec<Value> = match value {
                Value::Array(values) if key == "enum" => values.clone(),
                other => vec![other.clone()],
            };
            let mut found = false;
            for variant in &mut variants {
                let ty = variant["type"].as_str().unwrap_or_default().to_owned();
                let own: Vec<Value> = values
                    .iter()
                    .filter(|v| is_of_type(v, &ty))
                    .cloned()
                    .collect();
                if !own.is_empty() {
                    found = true;
                    variant.insert("enum".into(), Value::Array(own));
                }
            }
            if found {
                // Variants without a value of their own would accept anything of their type.
                let constrained: Vec<Map<String, Value>> = variants
                    .iter()
                    .filter(|v| v.contains_key("enum"))
                    .cloned()
                    .collect();
                variants = constrained;
                map.remove(&key);
            }
            continue;
        }
        if let Some(applies) = keyword_types(&key, value) {
            let value = value.clone();
            for variant in &mut variants {
                if applies.contains(&variant["type"].as_str().unwrap_or_default()) {
                    variant.insert(key.clone(), value.clone());
                }
            }
            map.remove(&key);
        }
    }
    let mut variants: Vec<Value> = variants.into_iter().map(Value::Object).collect();
    if nullable {
        variants.push(json!({ "type": "null" }));
    }
    map.insert("oneOf".into(), Value::Array(variants));
}

/// Warnings already printed: every SDK of a run loads the spec, on threads of its own.
static WARNED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());

/// Prints a warning once per run.
fn warn_once(message: String) {
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warned.insert(message.clone()) {
        eprintln!("warning: {message}");
    }
}

/// Reads what no SDK type expresses as untyped JSON: `not` (alone, the schema accepts anything
/// but a value; next to other keywords it is ignored) and tuple `prefixItems`, a list of
/// untyped values.
fn drop_unsupported_keywords(map: &mut Map<String, Value>, at: &str) {
    if map.remove("not").is_some() {
        let only_not = map
            .keys()
            .all(|k| ANNOTATIONS.contains(&k.as_str()) || k.starts_with("x-"));
        warn_once(if only_not {
            format!(
                "schema `{at}` is only a `not`, which no SDK type expresses: read as untyped JSON"
            )
        } else {
            format!(
                "`not` in schema `{at}` is ignored, as no SDK type expresses it: values it excludes are not rejected"
            )
        });
    }
    let tuple_items = map.get("items").is_some_and(Value::is_array);
    if map.remove("prefixItems").is_some() || tuple_items {
        warn_once(format!(
            "tuple array in schema `{at}` (`prefixItems`) is typed as a list of untyped JSON values: \
             SDKs have no fixed-length heterogeneous list"
        ));
        map.insert("items".to_owned(), json!({}));
        map.entry("type").or_insert_with(|| json!("array"));
    }
}

fn boolean_schema(value: &mut Value, at: &str) {
    match value {
        Value::Bool(allowed) => {
            if !*allowed {
                eprintln!(
                    "warning: schema `{at}` is `false`, which matches no value: read as untyped JSON"
                );
            }
            *value = json!({});
        }
        Value::Object(map) => {
            drop_unsupported_keywords(map, at);
            for key in SCHEMA_MAPS {
                for s in map
                    .get_mut(key)
                    .and_then(Value::as_object_mut)
                    .into_iter()
                    .flat_map(|m| m.values_mut())
                {
                    boolean_schema(s, at);
                }
            }
            for key in SCHEMA_VALUES {
                // `additionalProperties: false` is a flag, not a schema.
                if let Some(s) = map.get_mut(key)
                    && (key != "additionalProperties" || s.is_object())
                {
                    boolean_schema(s, at);
                }
            }
            for key in SCHEMA_LISTS {
                for s in map
                    .get_mut(key)
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    boolean_schema(s, at);
                }
            }
        }
        _ => {}
    }
}

fn walk(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key.starts_with("x-") || key.starts_with("example") {
                    continue;
                }
                if key == "schema" {
                    schema(child);
                } else if key == "schemas" {
                    for s in child
                        .as_object_mut()
                        .into_iter()
                        .flat_map(|m| m.values_mut())
                    {
                        schema(s);
                    }
                } else {
                    walk(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(walk),
        _ => {}
    }
}

fn schema(value: &mut Value) {
    let Value::Object(map) = value else { return };
    for key in SCHEMA_MAPS {
        for s in map
            .get_mut(key)
            .and_then(Value::as_object_mut)
            .into_iter()
            .flat_map(|m| m.values_mut())
        {
            schema(s);
        }
    }
    for key in SCHEMA_VALUES {
        if let Some(s) = map.get_mut(key) {
            schema(s);
        }
    }
    for key in SCHEMA_LISTS {
        for s in map
            .get_mut(key)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            schema(s);
        }
    }
    exclusive_bounds(map);
    let nullable = ["nullable", "x-nullable"]
        .into_iter()
        .filter_map(|key| map.remove(key))
        .any(|v| v == Value::Bool(true));
    if nullable {
        make_nullable(value);
    }
}

fn exclusive_bounds(map: &mut Map<String, Value>) {
    for (flag, bound) in [
        ("exclusiveMinimum", "minimum"),
        ("exclusiveMaximum", "maximum"),
    ] {
        match map.get(flag) {
            Some(Value::Bool(true)) => {
                if let Some(limit) = map.remove(bound) {
                    map.insert(flag.into(), limit);
                } else {
                    map.remove(flag);
                }
            }
            Some(Value::Bool(false)) => {
                map.remove(flag);
            }
            _ => {}
        }
    }
}

fn make_nullable(value: &mut Value) {
    let Value::Object(map) = value else { return };
    if let Some(Value::String(t)) = map.get("type") {
        let t = t.clone();
        map.insert("type".into(), json!([t, "null"]));
        return;
    }
    if matches!(map.get("type"), Some(Value::Array(_))) {
        if let Some(Value::Array(types)) = map.get_mut("type")
            && !types.contains(&json!("null"))
        {
            types.push(json!("null"));
        }
        return;
    }
    let mut outer = Map::new();
    for key in map.keys().cloned().collect::<Vec<_>>() {
        if ANNOTATIONS.contains(&key.as_str()) || key.starts_with("x-") {
            let v = map.remove(&key).unwrap();
            outer.insert(key, v);
        }
    }
    let inner = Value::Object(std::mem::take(map));
    outer.insert("oneOf".into(), json!([{ "type": "null" }, inner]));
    *map = outer;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn upgraded(components: Value) -> Value {
        let mut doc = json!({ "openapi": "3.0.3", "paths": {}, "components": components });
        to_3_1(&mut doc).unwrap();
        doc
    }

    fn schemas(s: Value) -> Value {
        upgraded(json!({ "schemas": s }))["components"]["schemas"].clone()
    }

    #[test]
    fn bumps_the_version() {
        assert_eq!(upgraded(json!({}))["openapi"], "3.1.0");
    }

    #[test]
    fn nullable_scalar_becomes_a_type_array() {
        let s = schemas(json!({ "A": { "type": "string", "nullable": true, "format": "uuid" } }));
        assert_eq!(
            s["A"],
            json!({ "type": ["string", "null"], "format": "uuid" })
        );
    }

    #[test]
    fn nullable_false_is_dropped() {
        let s = schemas(json!({ "A": { "type": "string", "nullable": false } }));
        assert_eq!(s["A"], json!({ "type": "string" }));
    }

    #[test]
    fn x_nullable_is_honoured() {
        let s = schemas(json!({ "A": { "type": "integer", "x-nullable": true } }));
        assert_eq!(s["A"], json!({ "type": ["integer", "null"] }));
    }

    #[test]
    fn nullable_ref_is_wrapped_keeping_annotations_outside() {
        let s = schemas(json!({
            "A": { "$ref": "#/components/schemas/B", "nullable": true, "description": "d" }
        }));
        assert_eq!(
            s["A"],
            json!({
                "description": "d",
                "oneOf": [{ "type": "null" }, { "$ref": "#/components/schemas/B" }]
            })
        );
    }

    #[test]
    fn nullable_all_of_is_wrapped() {
        let s = schemas(json!({
            "A": { "allOf": [{ "$ref": "#/components/schemas/B" }], "nullable": true }
        }));
        assert_eq!(
            s["A"]["oneOf"][1],
            json!({ "allOf": [{ "$ref": "#/components/schemas/B" }] })
        );
    }

    #[test]
    fn nullable_enum_keeps_its_values() {
        let s = schemas(json!({
            "A": { "type": "string", "enum": ["a", "b"], "nullable": true }
        }));
        assert_eq!(
            s["A"],
            json!({ "type": ["string", "null"], "enum": ["a", "b"] })
        );
    }

    #[test]
    fn exclusive_bounds_become_numeric() {
        let s = schemas(json!({
            "A": {
                "type": "number",
                "minimum": 1, "exclusiveMinimum": true,
                "maximum": 9, "exclusiveMaximum": false
            }
        }));
        assert_eq!(
            s["A"],
            json!({ "type": "number", "exclusiveMinimum": 1, "maximum": 9 })
        );
    }

    #[test]
    fn nested_schemas_are_converted() {
        let s = schemas(json!({
            "A": {
                "type": "object",
                "properties": {
                    "nullable": { "type": "string", "nullable": true },
                    "list": { "type": "array", "items": { "type": "integer", "nullable": true } },
                    "map": {
                        "type": "object",
                        "additionalProperties": { "type": "string", "nullable": true }
                    },
                    "all": { "allOf": [{ "type": "string", "nullable": true }] },
                    "one": { "oneOf": [{ "type": "string", "nullable": true }, { "type": "integer" }] }
                }
            }
        }));
        let p = &s["A"]["properties"];
        assert_eq!(p["nullable"]["type"], json!(["string", "null"]));
        assert_eq!(p["list"]["items"]["type"], json!(["integer", "null"]));
        assert_eq!(
            p["map"]["additionalProperties"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(p["all"]["allOf"][0]["type"], json!(["string", "null"]));
        assert_eq!(p["one"]["oneOf"][0]["type"], json!(["string", "null"]));
    }

    #[test]
    fn inline_schemas_and_examples_are_handled_correctly() {
        let mut doc = json!({
            "openapi": "3.0.0",
            "paths": { "/a": { "get": {
                "parameters": [{ "name": "q", "in": "query", "schema": { "type": "string", "nullable": true } }],
                "responses": { "200": { "description": "ok", "content": { "application/json": {
                    "schema": { "type": "string", "nullable": true },
                    "example": { "schema": { "nullable": true } }
                } } } }
            } } }
        });
        to_3_1(&mut doc).unwrap();
        let op = &doc["paths"]["/a"]["get"];
        assert_eq!(
            op["parameters"][0]["schema"]["type"],
            json!(["string", "null"])
        );
        let media = &op["responses"]["200"]["content"]["application/json"];
        assert_eq!(media["schema"]["type"], json!(["string", "null"]));
        assert_eq!(media["example"], json!({ "schema": { "nullable": true } }));
    }

    #[test]
    fn three_one_documents_are_untouched() {
        let mut doc = json!({ "openapi": "3.1.0", "components": { "schemas": {
            "A": { "type": "string", "nullable": true }
        } } });
        let before = doc.clone();
        to_3_1(&mut doc).unwrap();
        assert_eq!(doc, before);
    }

    #[test]
    fn swagger_2_is_rejected_with_a_hint() {
        let err = to_3_1(&mut json!({ "swagger": "2.0" })).unwrap_err();
        assert!(err.to_string().contains("swagger2openapi"), "{err}");
    }

    #[test]
    fn unknown_versions_are_rejected() {
        assert!(to_3_1(&mut json!({ "openapi": "4.0.0" })).is_err());
        assert!(to_3_1(&mut json!({})).is_err());
    }

    #[test]
    fn three_two_is_read_as_three_one_without_query_operations() {
        let mut doc = json!({ "openapi": "3.2.0", "paths": { "/x": {
            "get": {}, "query": {}, "additionalOperations": { "COPY": {} }
        } } });
        to_3_1(&mut doc).unwrap();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["paths"]["/x"], json!({ "get": {} }));
    }

    #[test]
    fn unsupported_versions_say_which_are_read() {
        let err = to_3_1(&mut json!({ "openapi": "4.0.0" })).unwrap_err();
        assert!(
            err.to_string()
                .contains("OpenAPI 4.0.0 is not supported; perseid reads 3.0, 3.1 and 3.2"),
            "{err}"
        );
    }

    #[test]
    fn boolean_schemas_become_empty_schemas() {
        let mut doc = json!({ "components": { "schemas": {
            "Any": true,
            "Never": false,
            "Holder": { "type": "object", "additionalProperties": false, "properties": {
                "a": true, "b": { "items": true }
            } }
        } }, "paths": { "/x": { "get": { "responses": { "200": { "content": {
            "application/json": { "schema": true } } } } } } } });
        boolean_schemas(&mut doc);
        let schemas = &doc["components"]["schemas"];
        assert_eq!(schemas["Any"], json!({}));
        assert_eq!(schemas["Never"], json!({}));
        assert_eq!(schemas["Holder"]["additionalProperties"], json!(false));
        assert_eq!(schemas["Holder"]["properties"]["a"], json!({}));
        assert_eq!(schemas["Holder"]["properties"]["b"]["items"], json!({}));
        let media = &doc["paths"]["/x"]["get"]["responses"]["200"]["content"]["application/json"];
        assert_eq!(media["schema"], json!({}));
    }

    #[test]
    fn not_and_tuples_become_untyped() {
        let mut doc = json!({ "components": { "schemas": {
            "Only": { "not": { "type": "string" }, "description": "d" },
            "Next": { "type": "string", "not": { "enum": ["x"] } },
            "Pair": { "type": "array", "prefixItems": [{ "type": "string" }, { "type": "integer" }] }
        } } });
        boolean_schemas(&mut doc);
        let schemas = &doc["components"]["schemas"];
        assert_eq!(schemas["Only"], json!({ "description": "d" }));
        assert_eq!(schemas["Next"], json!({ "type": "string" }));
        assert_eq!(schemas["Pair"], json!({ "type": "array", "items": {} }));
    }

    fn type_unioned(s: Value) -> Value {
        let mut doc = json!({ "components": { "schemas": s } });
        type_unions(&mut doc);
        doc["components"]["schemas"].clone()
    }

    #[test]
    fn type_arrays_of_several_types_become_one_of() {
        let s = type_unioned(json!({
            "A": { "type": ["string", "integer", "null"], "description": "d",
                   "format": "date-time", "minimum": 1, "enum": ["x", 2] }
        }));
        assert_eq!(
            s["A"],
            json!({ "description": "d", "oneOf": [
                { "type": "string", "format": "date-time", "enum": ["x"] },
                { "type": "integer", "minimum": 1, "enum": [2] },
                { "type": "null" }
            ] })
        );
    }

    #[test]
    fn integer_and_number_type_arrays_collapse_to_number() {
        let s = type_unioned(json!({
            "A": { "type": ["integer", "number"] },
            "B": { "type": ["number", "integer", "null"] },
            "C": { "type": ["integer", "string"] },
            "D": { "type": ["string", "null"] }
        }));
        assert_eq!(s["A"], json!({ "type": "number" }));
        assert_eq!(s["B"], json!({ "type": ["number", "null"] }));
        assert_eq!(
            s["C"],
            json!({ "oneOf": [{ "type": "integer" }, { "type": "string" }] })
        );
        assert_eq!(s["D"], json!({ "type": ["string", "null"] }));
    }

    #[test]
    fn type_arrays_in_examples_and_property_names_are_left_alone() {
        let s = type_unioned(json!({
            "A": { "type": "object", "example": { "type": ["a", "b"] },
                   "properties": { "type": { "type": ["array", "null"], "items": {} } } }
        }));
        assert_eq!(s["A"]["example"], json!({ "type": ["a", "b"] }));
        assert_eq!(
            s["A"]["properties"]["type"]["type"],
            json!(["array", "null"])
        );
    }
}
