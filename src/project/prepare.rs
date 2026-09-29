//! Optional compatibility adapter for the inherited OpenAPI parser.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    #[serde(default)]
    pub exclude_unsupported: bool,
    #[serde(default)]
    pub normalize_tags: bool,
}

fn promote(schema: Value, suggested: &str, schemas: &mut Map<String, Value>) -> Value {
    if schema.get("$ref").is_some() {
        return schema;
    }
    let base = suggested
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", p[..1].to_ascii_uppercase(), &p[1..]))
        .collect::<String>();
    let mut key = base.clone();
    let mut counter = 2;
    while schemas
        .get(&key)
        .is_some_and(|existing| existing != &schema)
    {
        key = format!("{base}{counter}");
        counter += 1;
    }
    schemas.insert(key.clone(), schema);
    json!({"$ref": format!("#/components/schemas/{key}")})
}
fn nonempty(v: Option<&Value>) -> bool {
    v.is_some_and(|v| match v {
        Value::Object(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => !v.is_null(),
    })
}
fn normalize(
    mut value: Value,
    context: &str,
    schemas: &mut Map<String, Value>,
    depth: usize,
) -> Result<Value> {
    ensure!(
        depth < 128,
        "Spec preparation exceeds maximum schema nesting"
    );
    let Some(schema) = value.as_object_mut() else {
        return Ok(value);
    };
    if schema.get("type").and_then(Value::as_str) == Some("object")
        && !nonempty(schema.get("properties"))
        && !["allOf", "oneOf", "anyOf"]
            .iter()
            .any(|k| schema.contains_key(*k))
    {
        schema
            .entry("additionalProperties")
            .or_insert(Value::Bool(true));
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array)
        && parts.len() == 1
        && parts[0].get("$ref").is_some()
    {
        let part = parts[0].as_object().unwrap().clone();
        schema.remove("allOf");
        schema.extend(part);
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for (field, property) in properties {
            let context = format!("{context}_{field}");
            let mut normalized = normalize(property.take(), &context, schemas, depth + 1)?;
            if normalized.get("type").and_then(Value::as_str) == Some("object")
                && nonempty(normalized.get("properties"))
            {
                let nullable = normalized.get("nullable") == Some(&Value::Bool(true));
                normalized = promote(normalized, &context, schemas);
                if nullable {
                    normalized["nullable"] = true.into();
                }
            }
            *property = normalized;
        }
    }
    for (key, suffix) in [("items", "item"), ("additionalProperties", "value")] {
        if let Some(item) = schema.get_mut(key)
            && item.is_object()
        {
            let context = format!("{context}_{suffix}");
            let mut normalized = normalize(item.take(), &context, schemas, depth + 1)?;
            if nonempty(normalized.get("properties"))
                || (key == "items"
                    && normalized
                        .get("enum")
                        .and_then(Value::as_array)
                        .is_some_and(|e| e.len() > 1))
            {
                normalized = promote(normalized, &context, schemas);
            }
            *item = normalized;
        }
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array)
        && parts.len() > 1
    {
        let mut props = Map::new();
        let mut required = Vec::new();
        let mut valid = true;
        for part in parts {
            let part = if let Some(reference) = part.get("$ref").and_then(Value::as_str) {
                schemas
                    .get(reference.rsplit('/').next().unwrap())
                    .context("Missing allOf reference")?
            } else {
                part
            };
            if part.get("type").and_then(Value::as_str) != Some("object") {
                valid = false;
                break;
            }
            if let Some(p) = part.get("properties").and_then(Value::as_object) {
                props.extend(p.clone());
            }
            if let Some(r) = part.get("required").and_then(Value::as_array) {
                required.extend(r.iter().cloned());
            }
        }
        if valid {
            schema.remove("allOf");
            schema.insert("type".into(), "object".into());
            schema.insert("properties".into(), props.into());
            schema.insert("required".into(), required.into());
        }
    }
    for composition in ["allOf", "oneOf", "anyOf"] {
        if let Some(parts) = schema.get_mut(composition).and_then(Value::as_array_mut) {
            for (i, part) in parts.iter_mut().enumerate() {
                *part = normalize(part.take(), &format!("{context}_{i}"), schemas, depth + 1)?;
            }
        }
    }
    if !schema.contains_key("discriminator")
        && let Some(variants) = schema.get("oneOf").and_then(Value::as_array)
    {
        let mut common: Option<BTreeSet<String>> = None;
        for variant in variants {
            let candidates = variant
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.iter())
                .filter(|(_, v)| {
                    v.get("type").and_then(Value::as_str) == Some("string")
                        && v.get("enum")
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.len() == 1)
                })
                .map(|(k, _)| k.clone())
                .collect::<BTreeSet<_>>();
            common = Some(match common {
                None => candidates,
                Some(previous) => previous.intersection(&candidates).cloned().collect(),
            });
        }
        if let Some(common) = common
            && common.len() == 1
        {
            schema.insert(
                "discriminator".into(),
                json!({"propertyName": common.first().unwrap()}),
            );
        }
    }
    if !schema.contains_key("discriminator")
        && let Some(parts) = schema.get("oneOf").and_then(Value::as_array)
        && parts.len() == 1
    {
        let part = parts[0]
            .as_object()
            .context("oneOf variant must be an object")?
            .clone();
        schema.remove("oneOf");
        schema.extend(part);
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false))
        && nonempty(schema.get("properties"))
    {
        schema.remove("additionalProperties");
    }
    Ok(value)
}

pub fn prepare(mut spec: Value, source: &str, options: &Preparation) -> Result<(Value, Value)> {
    spec["openapi"] = "3.1.0".into();
    let mut schemas = spec
        .get_mut("components")
        .and_then(|c| c.get_mut("schemas"))
        .and_then(Value::as_object_mut)
        .map(std::mem::take)
        .unwrap_or_default();
    let mut excluded = Vec::new();
    let mut included = 0;
    for (path, item) in spec
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .context("Missing paths")?
    {
        let item = item.as_object_mut().context("Invalid path item")?;
        for method in [
            "get", "post", "put", "patch", "delete", "head", "options", "trace",
        ] {
            let Some(op) = item.get_mut(method) else {
                continue;
            };
            let id = op
                .get("operationId")
                .and_then(Value::as_str)
                .context("Missing operationId")?
                .to_owned();
            let content = op
                .get("requestBody")
                .and_then(|r| r.get("content"))
                .and_then(Value::as_object);
            let mut reason = None;
            if let Some(content) = content
                && content.keys().any(|k| {
                    ![
                        "application/json",
                        "application/x-www-form-urlencoded",
                        "application/octet-stream",
                        "multipart/form-data",
                    ]
                    .contains(&k.as_str())
                })
            {
                reason = Some(format!(
                    "Upload request body: {}",
                    content.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            if !op
                .get("responses")
                .and_then(Value::as_object)
                .is_some_and(|r| r.keys().any(|c| c.starts_with('2')))
            {
                reason = Some("No documented 2xx response (redirect or protocol upgrade)".into());
            }
            if let Some(reason) = reason {
                ensure!(
                    options.exclude_unsupported,
                    "Unsupported operation {id}: {reason}"
                );
                excluded.push(json!({"method": method.to_uppercase(), "path": path, "operation_id": id, "reason": reason}));
                item.remove(method);
                continue;
            }
            included += 1;
            if options.normalize_tags
                && let Some(tags) = op.get_mut("tags").and_then(Value::as_array_mut)
            {
                for tag in tags {
                    let parts = tag
                        .as_str()
                        .context("Tag must be a string")?
                        .split("::")
                        .collect::<Vec<_>>();
                    let joined = parts[parts.len().saturating_sub(2)..].join(" ");
                    *tag = joined
                        .strip_prefix("super ")
                        .unwrap_or(&joined)
                        .nfkd()
                        .filter(char::is_ascii)
                        .collect::<String>()
                        .into();
                }
            }
            if let Some(content) = op
                .get_mut("requestBody")
                .and_then(|r| r.get_mut("content"))
                .and_then(Value::as_object_mut)
            {
                for body in content.values_mut() {
                    let value = body.get_mut("schema").map(Value::take).unwrap_or(json!({}));
                    body["schema"] = promote(value, &format!("{id}_request"), &mut schemas);
                }
            }
            if let Some(responses) = op.get_mut("responses").and_then(Value::as_object_mut) {
                responses.retain(|code, _| code.starts_with(['2', '4', '5']));
                for (code, response) in responses {
                    if let Some(body) = response
                        .get_mut("content")
                        .and_then(|c| c.get_mut("application/json"))
                    {
                        let value = body.get_mut("schema").map(Value::take).unwrap_or(json!({}));
                        body["schema"] =
                            promote(value, &format!("{id}_response_{code}"), &mut schemas);
                    }
                }
            }
        }
    }
    let mut processed = BTreeSet::new();
    loop {
        let pending = schemas
            .keys()
            .filter(|k| !processed.contains(*k))
            .cloned()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        for key in pending {
            let value = normalize(schemas[&key].clone(), &key, &mut schemas, 0)?;
            schemas.insert(key.clone(), value);
            processed.insert(key);
        }
    }
    if spec.get("components").is_none() {
        spec["components"] = json!({});
    }
    spec["components"]["schemas"] = schemas.into();
    Ok((
        spec,
        json!({"source": source, "included_operations": included, "excluded_operations": excluded}),
    ))
}
