//! Converts a Swagger 2.0 document to OpenAPI 3.0 in place, which the rest of the pipeline then
//! reads as any 3.0 spec.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::{
    normalize::{percent_decode, unescape_segment},
    upgrade::{self, ANNOTATIONS, LITERALS, NAME_MAPS},
};

pub(super) const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];
/// Fields of a parameter, a header or an `items` object that describe its value: its schema.
const VALUE_FIELDS: [&str; 17] = [
    "type",
    "format",
    "items",
    "default",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "maxItems",
    "minItems",
    "uniqueItems",
    "enum",
    "multipleOf",
    "x-nullable",
];
const FORM: &str = "application/x-www-form-urlencoded";
const MULTIPART: &str = "multipart/form-data";
const JSON: &str = "application/json";

pub(super) fn is_swagger(doc: &Value) -> bool {
    doc.get("swagger").is_some()
}

/// Where the objects of a 2.0 document moved to: `/definitions/Pet` is `/components/schemas/Pet`.
pub(super) fn fragment(fragment: &str) -> Option<String> {
    [
        ("/definitions/", "/components/schemas/"),
        ("/parameters/", "/components/parameters/"),
        ("/responses/", "/components/responses/"),
    ]
    .into_iter()
    .find_map(|(from, to)| {
        fragment
            .strip_prefix(from)
            .map(|name| format!("{to}{name}"))
    })
}

/// Converts the 2.0-only keywords of the schemas below `value`.
pub(super) fn schemas(value: &mut Value) {
    upgrade::each_schema(value, &mut schema_keywords);
}

pub(super) fn to_3_0(doc: &mut Value) -> Result<()> {
    let version = match &doc["swagger"] {
        Value::String(version) => version.clone(),
        other => other.to_string(),
    };
    if !matches!(version.as_str(), "2.0" | "2") {
        bail!("Swagger {version} is not supported; perseid reads Swagger 2.0 and OpenAPI 3.x");
    }
    let Some(mut old) = doc.as_object_mut().map(std::mem::take) else {
        bail!("the document is not an object");
    };
    let globals = Globals {
        consumes: media_types(old.shift_remove("consumes")).unwrap_or_default(),
        produces: media_types(old.shift_remove("produces")).unwrap_or_default(),
        parameters: object(old.shift_remove("parameters")),
        responses: object(old.shift_remove("responses")),
    };
    let mut new = Map::new();
    new.insert("openapi".into(), json!("3.0.3"));
    old.shift_remove("swagger");
    if let Some(info) = old.shift_remove("info") {
        new.insert("info".into(), info);
    }
    let servers = servers(
        old.shift_remove("host"),
        old.shift_remove("basePath"),
        old.shift_remove("schemes"),
    );
    if !servers.is_empty() {
        new.insert("servers".into(), Value::Array(servers));
    }
    let mut components = Map::new();
    if let Some(definitions) = old.shift_remove("definitions") {
        components.insert("schemas".into(), definitions);
    }
    let parameters: Map<String, Value> = globals
        .parameters
        .iter()
        .filter(|(_, p)| !is_body(p))
        .map(|(name, p)| (name.clone(), parameter(p)))
        .collect();
    if !parameters.is_empty() {
        components.insert("parameters".into(), Value::Object(parameters));
    }
    let responses: Map<String, Value> = globals
        .responses
        .iter()
        .map(|(name, r)| {
            (
                name.clone(),
                response(r.clone(), &globals.produces, &globals),
            )
        })
        .collect();
    if !responses.is_empty() {
        components.insert("responses".into(), Value::Object(responses));
    }
    if let Some(Value::Object(schemes)) = old.shift_remove("securityDefinitions") {
        let schemes = schemes
            .into_iter()
            .map(|(name, scheme)| Ok((name.clone(), security_scheme(scheme, &name)?)))
            .collect::<Result<_>>()?;
        components.insert("securitySchemes".into(), Value::Object(schemes));
    }
    for (key, value) in old {
        let value = match (key.as_str(), value) {
            ("paths", Value::Object(mut paths)) => {
                for item in paths.values_mut() {
                    if let Value::Object(item) = item {
                        path_item(item, &globals);
                    }
                }
                Value::Object(paths)
            }
            (_, value) => value,
        };
        new.insert(key, value);
    }
    if !components.is_empty() {
        new.insert("components".into(), Value::Object(components));
    }
    *doc = Value::Object(new);
    rewrite_refs(doc, false);
    schemas(doc);
    Ok(())
}

struct Globals {
    consumes: Vec<String>,
    produces: Vec<String>,
    /// The shared parameters and responses, as 2.0 declares them.
    parameters: Map<String, Value>,
    responses: Map<String, Value>,
}

impl Globals {
    /// The shared parameter `p` references, or `p` itself.
    fn resolve<'a>(&'a self, p: &'a Value) -> &'a Value {
        p.get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| shared(&self.parameters, r, "#/parameters/"))
            .unwrap_or(p)
    }
}

/// The entry of `map` the local reference `reference` names below `prefix`.
fn shared<'a>(map: &'a Map<String, Value>, reference: &str, prefix: &str) -> Option<&'a Value> {
    let name = percent_decode(reference.strip_prefix(prefix)?);
    if name.contains('/') {
        return None;
    }
    map.get(&unescape_segment(&name))
}

fn object(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// A `consumes` or `produces` list, `None` when absent (the global one applies then): an empty
/// one clears the global one.
fn media_types(value: Option<Value>) -> Option<Vec<String>> {
    let types = value?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some(types)
}

fn essence(media_type: &str) -> &str {
    media_type.split(';').next().unwrap_or_default().trim()
}

fn is_json(media_type: &str) -> bool {
    let essence = essence(media_type);
    essence == JSON || essence.ends_with("+json") || essence == "*/*"
}

/// One server per scheme, `https` first, as the SDK's base URL is the first one; `https` when
/// the spec names none.
fn servers(host: Option<Value>, base: Option<Value>, schemes: Option<Value>) -> Vec<Value> {
    let base = base.as_ref().and_then(Value::as_str).unwrap_or_default();
    let base = base.trim_end_matches('/');
    let Some(host) = host.as_ref().and_then(Value::as_str) else {
        return match base {
            "" => vec![],
            base => vec![json!({ "url": base })],
        };
    };
    let mut schemes: Vec<String> = schemes
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .filter(|s| s == "https" || s == "http")
        .collect();
    schemes.sort_by_key(|s| s != "https");
    schemes.dedup();
    if schemes.is_empty() {
        schemes.push("https".into());
    }
    let host = host.trim_end_matches('/');
    schemes
        .iter()
        .map(|scheme| json!({ "url": format!("{scheme}://{host}{base}") }))
        .collect()
}

fn is_body(p: &Value) -> bool {
    matches!(p["in"].as_str(), Some("body" | "formData"))
}

/// The schema of a parameter, a header or an `items` object, out of its value fields.
fn value_schema(fields: &Map<String, Value>) -> Map<String, Value> {
    let mut schema = Map::new();
    for (key, value) in fields {
        if !VALUE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let value = match (key.as_str(), value) {
            ("items", Value::Object(items)) => Value::Object(value_schema(items)),
            _ => value.clone(),
        };
        schema.insert(key.clone(), value);
    }
    schema
}

/// A path, query or header parameter (or a reference to one), its value fields moved to a
/// `schema` and its `collectionFormat` to a `style`.
fn parameter(p: &Value) -> Value {
    let Value::Object(fields) = p else {
        return p.clone();
    };
    if fields.contains_key("$ref") {
        return p.clone();
    }
    let mut out = Map::new();
    for (key, value) in fields {
        match key.as_str() {
            "x-example" => {
                out.insert("example".into(), value.clone());
            }
            "collectionFormat" => {}
            key if VALUE_FIELDS.contains(&key) => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    let schema = value_schema(fields);
    if let Some((style, explode)) = array_style(fields, &schema) {
        out.insert("style".into(), json!(style));
        out.insert("explode".into(), json!(explode));
    }
    out.insert("schema".into(), Value::Object(schema));
    Value::Object(out)
}

/// The `style` and `explode` of an array parameter or form field, out of its `collectionFormat`;
/// `None` when it is not an array or OpenAPI's default for its location applies.
fn array_style(
    fields: &Map<String, Value>,
    schema: &Map<String, Value>,
) -> Option<(&'static str, bool)> {
    if schema.get("type").and_then(Value::as_str) != Some("array") {
        return None;
    }
    let field = |key: &str| fields.get(key).and_then(Value::as_str);
    let name = field("name").unwrap_or_default();
    let location = field("in").unwrap_or_default();
    let format = field("collectionFormat").unwrap_or("csv");
    let form = matches!(location, "query" | "formData");
    match format {
        "csv" if form => Some(("form", false)),
        "multi" if form => Some(("form", true)),
        "ssv" if form => Some(("spaceDelimited", false)),
        "pipes" if form => Some(("pipeDelimited", false)),
        "csv" => None,
        format => {
            upgrade::warn_once(format!(
                "`collectionFormat: {format}` of the {location} parameter `{name}` has no \
                 OpenAPI 3 equivalent: read as `csv`"
            ));
            form.then_some(("form", false))
        }
    }
}

fn path_item(item: &mut Map<String, Value>, globals: &Globals) {
    let (bodies, shared): (Vec<Value>, Vec<Value>) = match item.get("parameters") {
        Some(Value::Array(params)) => params
            .iter()
            .cloned()
            .partition(|p| is_body(globals.resolve(p))),
        _ => (vec![], vec![]),
    };
    if !shared.is_empty() {
        item.insert(
            "parameters".into(),
            shared.iter().map(parameter).collect::<Vec<_>>().into(),
        );
    } else {
        item.shift_remove("parameters");
    }
    for method in METHODS {
        if let Some(Value::Object(op)) = item.get_mut(method) {
            operation(op, &bodies, globals);
        }
    }
}

/// Converts an operation, its `body` or `formData` parameters (its own, or those of `bodies`, its
/// path's, that it does not override) becoming its request body.
fn operation(op: &mut Map<String, Value>, bodies: &[Value], globals: &Globals) {
    let consumes =
        media_types(op.get("consumes").cloned()).unwrap_or_else(|| globals.consumes.clone());
    let produces =
        media_types(op.get("produces").cloned()).unwrap_or_else(|| globals.produces.clone());
    let own: Vec<Value> = match op.get("parameters") {
        Some(Value::Array(params)) => params.clone(),
        _ => vec![],
    };
    let key = |p: &Value| (p["in"].clone(), p["name"].clone());
    let (own_bodies, params): (Vec<&Value>, Vec<&Value>) =
        own.iter().partition(|p| is_body(globals.resolve(p)));
    let own_bodies: Vec<Value> = own_bodies
        .into_iter()
        .map(|p| globals.resolve(p).clone())
        .collect();
    let overridden: Vec<_> = own_bodies.iter().map(key).collect();
    let own_body = own_bodies.iter().any(|p| p["in"] == "body");
    // An operation's body or form replaces its path's body; its form adds to its path's form.
    let inherited = |p: &Value| match p["in"].as_str() {
        Some("body") => own_bodies.is_empty(),
        _ => !own_body && !overridden.contains(&key(p)),
    };
    let fields: Vec<Value> = bodies
        .iter()
        .map(|p| globals.resolve(p).clone())
        .filter(inherited)
        .chain(own_bodies.iter().cloned())
        .collect();
    let mut body = request_body(&fields, &consumes);
    let mut converted = Map::new();
    for (key, value) in std::mem::take(op) {
        match key.as_str() {
            "consumes" | "produces" | "schemes" => {}
            "parameters" => {
                if !params.is_empty() {
                    let params: Vec<Value> = params.iter().map(|p| parameter(p)).collect();
                    converted.insert(key, params.into());
                }
                if let Some(body) = body.take() {
                    converted.insert("requestBody".into(), body);
                }
            }
            "responses" => {
                let responses = object(Some(value))
                    .into_iter()
                    .map(|(status, r)| (status, response(r, &produces, globals)))
                    .collect();
                converted.insert(key, Value::Object(responses));
            }
            _ => {
                converted.insert(key, value);
            }
        }
    }
    if let Some(body) = body {
        converted.insert("requestBody".into(), body);
    }
    *op = converted;
}

/// The request body of the `body` parameter, or else of the `formData` ones.
fn request_body(params: &[Value], consumes: &[String]) -> Option<Value> {
    if let Some(Value::Object(param)) = params.iter().find(|p| p["in"] == "body") {
        let schema = param.get("schema").cloned().unwrap_or_else(|| json!({}));
        let types = match consumes {
            [] => vec![JSON.to_owned()],
            types => types.to_vec(),
        };
        let mut body = Map::new();
        if let Some(description) = param.get("description") {
            body.insert("description".into(), description.clone());
        }
        let content: Map<String, Value> = types
            .into_iter()
            .map(|t| (t, json!({ "schema": schema })))
            .collect();
        body.insert("content".into(), Value::Object(content));
        if param.get("required") == Some(&Value::Bool(true)) {
            body.insert("required".into(), json!(true));
        }
        for (key, value) in param.iter().filter(|(k, _)| k.starts_with("x-")) {
            body.insert(key.clone(), value.clone());
        }
        return Some(Value::Object(body));
    }
    let fields: Vec<&Map<String, Value>> = params
        .iter()
        .filter(|p| p["in"] == "formData")
        .filter_map(Value::as_object)
        .collect();
    if fields.is_empty() {
        return None;
    }
    let has_file = fields.iter().any(|f| f.get("type") == Some(&json!("file")));
    let mut properties = Map::new();
    let mut required = vec![];
    let mut encoding = Map::new();
    for field in &fields {
        let name = field
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut schema = value_schema(field);
        if let Some(description) = field.get("description") {
            schema.insert("description".into(), description.clone());
        }
        if field.get("required") == Some(&Value::Bool(true)) {
            required.push(json!(name));
        }
        if let Some((style, explode)) = array_style(field, &schema) {
            encoding.insert(
                name.to_owned(),
                json!({ "style": style, "explode": explode }),
            );
        }
        properties.insert(name.to_owned(), Value::Object(schema));
    }
    let mut schema = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        schema["required"] = Value::Array(required.clone());
    }
    let mut types: Vec<&str> = consumes
        .iter()
        .map(|t| essence(t))
        .filter(|t| *t == MULTIPART || (*t == FORM && !has_file))
        .collect();
    if types.is_empty() {
        types.push(if has_file { MULTIPART } else { FORM });
    }
    let content: Map<String, Value> = types
        .into_iter()
        .map(|t| {
            let mut media = json!({ "schema": schema });
            if !encoding.is_empty() {
                media["encoding"] = Value::Object(encoding.clone());
            }
            (t.to_owned(), media)
        })
        .collect();
    let mut body = json!({ "content": content });
    if !required.is_empty() {
        body["required"] = json!(true);
    }
    Some(body)
}

/// A response, its schema served as each of `produces` (JSON when there are none, bytes for a
/// file) with the `examples` of each media type. A reference to a shared response stays one
/// unless the operation produces other media types than the shared ones are converted with.
fn response(r: Value, produces: &[String], globals: &Globals) -> Value {
    let Value::Object(fields) = r else {
        return r;
    };
    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
        if produces == globals.produces.as_slice() {
            return Value::Object(fields);
        }
        return match shared(&globals.responses, reference, "#/responses/") {
            Some(shared) => response(shared.clone(), produces, globals),
            None => Value::Object(fields),
        };
    }
    let examples = object(fields.get("examples").cloned());
    let mut out = Map::new();
    out.insert(
        "description".into(),
        fields.get("description").cloned().unwrap_or(json!("")),
    );
    for (key, value) in fields {
        match key.as_str() {
            "description" | "examples" => {}
            "schema" => {
                let file = value.get("type") == Some(&json!("file"));
                let types: Vec<String> = match produces {
                    types if file && types.iter().all(|t| is_json(t)) => {
                        vec!["application/octet-stream".into()]
                    }
                    [] => vec![JSON.into()],
                    types => types.to_vec(),
                };
                let content: Map<String, Value> = types
                    .into_iter()
                    .map(|t| {
                        let mut media = json!({ "schema": value });
                        let example = examples.get(&t).or_else(|| {
                            examples
                                .iter()
                                .find(|(k, _)| essence(k).eq_ignore_ascii_case(essence(&t)))
                                .map(|(_, example)| example)
                        });
                        if let Some(example) = example {
                            media["example"] = example.clone();
                        }
                        (t, media)
                    })
                    .collect();
                out.insert("content".into(), Value::Object(content));
            }
            "headers" => {
                let headers: Map<String, Value> = object(Some(value))
                    .into_iter()
                    .map(|(name, header)| (name, self::header(header)))
                    .collect();
                out.insert(key, Value::Object(headers));
            }
            _ => {
                out.insert(key, value);
            }
        }
    }
    Value::Object(out)
}

fn header(header: Value) -> Value {
    let Value::Object(fields) = header else {
        return header;
    };
    let mut out: Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| *k == "description" || (k.starts_with("x-") && *k != "x-nullable"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    out.insert("schema".into(), Value::Object(value_schema(&fields)));
    Value::Object(out)
}

fn security_scheme(scheme: Value, name: &str) -> Result<Value> {
    let Value::Object(fields) = scheme else {
        return Ok(scheme);
    };
    let field = |key: &str| fields.get(key).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    match fields.get("type").and_then(Value::as_str) {
        Some("basic") => {
            out.insert("type".into(), json!("http"));
            out.insert("scheme".into(), json!("basic"));
        }
        Some("oauth2") => {
            let (flow, urls): (&str, &[&str]) = match fields.get("flow").and_then(Value::as_str) {
                Some("implicit") => ("implicit", &["authorizationUrl"]),
                Some("password") => ("password", &["tokenUrl"]),
                Some("application") => ("clientCredentials", &["tokenUrl"]),
                Some("accessCode") => ("authorizationCode", &["authorizationUrl", "tokenUrl"]),
                Some(other) => bail!(
                    "OAuth2 security scheme `{name}` has an unknown `flow: {other}`; Swagger 2.0 \
                     flows are implicit, password, application and accessCode"
                ),
                None => bail!("OAuth2 security scheme `{name}` has no `flow`"),
            };
            let mut details: Map<String, Value> = urls
                .iter()
                .map(|url| ((*url).to_owned(), field(url)))
                .collect();
            let scopes = fields.get("scopes").cloned().unwrap_or_else(|| json!({}));
            details.insert("scopes".into(), scopes);
            out.insert("type".into(), json!("oauth2"));
            out.insert("flows".into(), json!({ flow: details }));
        }
        _ => {
            for key in ["type", "name", "in"] {
                out.insert(key.into(), field(key));
            }
        }
    }
    for (key, value) in &fields {
        if key == "description" || key.starts_with("x-") {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn rewrite_refs(value: &mut Value, names: bool) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if !names && key == "$ref" {
                    if let Value::String(reference) = child
                        && let Some(moved) = reference.strip_prefix('#').and_then(fragment)
                    {
                        *reference = format!("#{moved}");
                    }
                } else if names || !(LITERALS.contains(&key.as_str()) || key.starts_with("x-")) {
                    rewrite_refs(child, !names && NAME_MAPS.contains(&key.as_str()));
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| rewrite_refs(i, false)),
        _ => {}
    }
}

/// `type: file` is binary, a `discriminator` names its property in an object, `x-nullable` is
/// spelled `nullable`, and the keywords next to a `$ref`, which 2.0 ignores, are dropped but for
/// annotations.
fn schema_keywords(map: &mut Map<String, Value>) {
    if map.contains_key("$ref") {
        map.retain(|key, _| {
            key == "$ref"
                || key == "nullable"
                || key.starts_with("x-")
                || ANNOTATIONS.contains(&key.as_str())
        });
    }
    if map.get("type") == Some(&json!("file")) {
        map.insert("type".into(), json!("string"));
        map.insert("format".into(), json!("binary"));
    }
    if let Some(Value::String(property)) = map.get("discriminator") {
        let discriminator = json!({ "propertyName": property });
        map.insert("discriminator".into(), discriminator);
    }
    if let Some(nullable) = map.shift_remove("x-nullable") {
        map.entry("nullable").or_insert(nullable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converted(doc: Value) -> Value {
        let mut doc = doc;
        doc["swagger"] = json!("2.0");
        to_3_0(&mut doc).unwrap();
        doc
    }

    fn operation_of(paths: Value) -> Value {
        let doc = converted(json!({ "paths": { "/x": { "post": paths } } }));
        doc["paths"]["/x"]["post"].clone()
    }

    #[test]
    fn servers_come_from_the_host_base_path_and_schemes() {
        let servers = |doc: Value| converted(doc)["servers"].clone();
        assert_eq!(
            servers(
                json!({ "host": "api.example.com", "basePath": "/v1/", "schemes": ["http", "https", "wss"] })
            ),
            json!([{ "url": "https://api.example.com/v1" }, { "url": "http://api.example.com/v1" }])
        );
        assert_eq!(
            servers(json!({ "host": "api.example.com" })),
            json!([{ "url": "https://api.example.com" }])
        );
        assert_eq!(
            servers(json!({ "basePath": "/v1" })),
            json!([{ "url": "/v1" }])
        );
        assert_eq!(servers(json!({ "basePath": "/" })), Value::Null);
    }

    #[test]
    fn shared_objects_move_to_components_and_references_follow() {
        let doc = converted(json!({
            "info": { "title": "T", "version": "1" },
            "paths": { "/pets/{id}": {
                "parameters": [{ "$ref": "#/parameters/id" }],
                "get": { "responses": {
                    "200": { "description": "ok", "schema": { "$ref": "#/definitions/Pet" } },
                    "404": { "$ref": "#/responses/NotFound" }
                } }
            } },
            "definitions": {
                "Pet": { "type": "object", "properties": {
                    "$ref": { "type": "string" },
                    "owner": { "$ref": "#/definitions/Owner" }
                }, "example": { "$ref": "#/definitions/Pet" } },
                "Owner": { "type": "object" }
            },
            "parameters": { "id": { "name": "id", "in": "path", "required": true, "type": "string" } },
            "responses": { "NotFound": { "description": "missing" } }
        }));
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["openapi", "info", "paths", "components"]);
        assert_eq!(doc["openapi"], "3.0.3");
        let item = &doc["paths"]["/pets/{id}"];
        assert_eq!(item["parameters"][0]["$ref"], "#/components/parameters/id");
        let responses = &item["get"]["responses"];
        assert_eq!(
            responses["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
        assert_eq!(responses["404"]["$ref"], "#/components/responses/NotFound");
        let components = &doc["components"];
        let pet = &components["schemas"]["Pet"];
        assert_eq!(
            pet["properties"]["owner"]["$ref"],
            "#/components/schemas/Owner"
        );
        assert_eq!(pet["properties"]["$ref"], json!({ "type": "string" }));
        assert_eq!(pet["example"]["$ref"], "#/definitions/Pet");
        assert_eq!(
            components["parameters"]["id"],
            json!({ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } })
        );
        assert_eq!(
            components["responses"]["NotFound"],
            json!({ "description": "missing" })
        );
    }

    #[test]
    fn body_parameters_become_request_bodies_of_each_consumed_media_type() {
        let doc = converted(json!({
            "consumes": ["application/json", "application/xml"],
            "parameters": { "pet": {
                "name": "pet", "in": "body", "required": true, "description": "The pet.",
                "schema": { "$ref": "#/definitions/Pet" }
            } },
            "paths": {
                "/a": { "post": { "parameters": [{ "$ref": "#/parameters/pet" }], "responses": {} } },
                "/b": { "post": { "consumes": ["application/merge-patch+json"], "parameters": [
                    { "name": "q", "in": "query", "type": "string" },
                    { "name": "patch", "in": "body", "schema": { "type": "object" } }
                ], "responses": {} } }
            }
        }));
        assert_eq!(doc["components"].get("parameters"), None);
        let schema = json!({ "$ref": "#/components/schemas/Pet" });
        assert_eq!(
            doc["paths"]["/a"]["post"],
            json!({
                "requestBody": {
                    "description": "The pet.",
                    "content": {
                        "application/json": { "schema": schema },
                        "application/xml": { "schema": schema }
                    },
                    "required": true
                },
                "responses": {}
            })
        );
        let b = &doc["paths"]["/b"]["post"];
        assert_eq!(b["parameters"].as_array().unwrap().len(), 1);
        assert_eq!(
            b["requestBody"],
            json!({ "content": { "application/merge-patch+json": { "schema": { "type": "object" } } } })
        );
    }

    #[test]
    fn form_data_becomes_a_form_body_or_a_multipart_one_with_files() {
        let form = operation_of(json!({
            "consumes": ["application/x-www-form-urlencoded"],
            "parameters": [
                { "name": "name", "in": "formData", "required": true, "type": "string", "description": "d" },
                { "name": "tags", "in": "formData", "type": "array", "items": { "type": "string" } },
                { "name": "ids", "in": "formData", "type": "array", "items": { "type": "integer" },
                  "collectionFormat": "multi" }
            ],
            "responses": {}
        }));
        assert_eq!(
            form["requestBody"],
            json!({ "content": { FORM: {
                "schema": { "type": "object", "properties": {
                    "name": { "type": "string", "description": "d" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "ids": { "type": "array", "items": { "type": "integer" } }
                }, "required": ["name"] },
                "encoding": {
                    "tags": { "style": "form", "explode": false },
                    "ids": { "style": "form", "explode": true }
                }
            } }, "required": true })
        );
        let upload = operation_of(json!({
            "consumes": ["application/x-www-form-urlencoded", "multipart/form-data; boundary=x"],
            "parameters": [
                { "name": "file", "in": "formData", "type": "file" },
                { "name": "note", "in": "formData", "type": "string" }
            ],
            "responses": {}
        }));
        assert_eq!(
            upload["requestBody"],
            json!({ "content": { MULTIPART: { "schema": { "type": "object", "properties": {
                "file": { "type": "string", "format": "binary" },
                "note": { "type": "string" }
            } } } } })
        );
        let unspecified = operation_of(json!({
            "parameters": [{ "name": "a", "in": "formData", "type": "string" }],
            "responses": {}
        }));
        assert!(unspecified["requestBody"]["content"].get(FORM).is_some());
    }

    #[test]
    fn path_level_bodies_apply_to_each_operation_unless_overridden() {
        let doc = converted(json!({ "paths": { "/x": {
            "parameters": [
                { "name": "id", "in": "path", "required": true, "type": "string" },
                { "name": "a", "in": "formData", "type": "string" }
            ],
            "post": { "responses": {} },
            "put": { "parameters": [{ "name": "a", "in": "formData", "type": "integer" }], "responses": {} }
        } } }));
        let item = &doc["paths"]["/x"];
        assert_eq!(item["parameters"].as_array().unwrap().len(), 1);
        let a = |method: &str| {
            item[method]["requestBody"]["content"][FORM]["schema"]["properties"]["a"]["type"]
                .clone()
        };
        assert_eq!((a("post"), a("put")), (json!("string"), json!("integer")));
    }

    #[test]
    fn responses_are_served_as_the_produced_media_types() {
        let doc = converted(json!({
            "produces": ["application/json"],
            "responses": { "Gone": { "description": "gone", "schema": { "type": "string" } } },
            "paths": { "/x": {
                "get": {
                    "produces": ["application/json", "text/csv"],
                    "responses": {
                        "200": {
                            "description": "ok",
                            "schema": { "type": "string" },
                            "headers": { "X-Rate": { "type": "integer", "description": "r" } },
                            "examples": { "text/csv": "a,b" }
                        },
                        "410": { "$ref": "#/responses/Gone" }
                    }
                },
                "delete": { "responses": {
                    "200": { "description": "the bytes", "schema": { "type": "file" } },
                    "410": { "$ref": "#/responses/Gone" },
                    "default": { "schema": { "type": "string" } }
                } }
            } }
        }));
        let get = &doc["paths"]["/x"]["get"]["responses"];
        assert_eq!(
            get["200"],
            json!({
                "description": "ok",
                "content": {
                    "application/json": { "schema": { "type": "string" } },
                    "text/csv": { "schema": { "type": "string" }, "example": "a,b" }
                },
                "headers": { "X-Rate": { "description": "r", "schema": { "type": "integer" } } }
            })
        );
        assert_eq!(get["410"]["description"], "gone");
        assert!(get["410"]["content"].get("text/csv").is_some());
        let delete = &doc["paths"]["/x"]["delete"]["responses"];
        assert_eq!(
            delete["200"]["content"],
            json!({ "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } })
        );
        assert_eq!(delete["410"]["$ref"], "#/components/responses/Gone");
        assert_eq!(delete["default"]["description"], "");
    }

    #[test]
    fn collection_formats_become_styles() {
        let op = operation_of(json!({ "parameters": [
            { "name": "csv", "in": "query", "type": "array", "items": { "type": "string", "collectionFormat": "csv" } },
            { "name": "multi", "in": "query", "type": "array", "items": { "type": "string" }, "collectionFormat": "multi" },
            { "name": "ssv", "in": "query", "type": "array", "items": { "type": "string" }, "collectionFormat": "ssv" },
            { "name": "pipes", "in": "query", "type": "array", "items": { "type": "string" }, "collectionFormat": "pipes" },
            { "name": "tsv", "in": "query", "type": "array", "items": { "type": "string" }, "collectionFormat": "tsv" },
            { "name": "h", "in": "header", "type": "array", "items": { "type": "string" } },
            { "name": "n", "in": "query", "type": "integer", "x-nullable": true, "x-example": 3 }
        ], "responses": {} }));
        let style = |i: usize| {
            (
                op["parameters"][i]["style"].clone(),
                op["parameters"][i]["explode"].clone(),
            )
        };
        assert_eq!(style(0), (json!("form"), json!(false)));
        assert_eq!(style(1), (json!("form"), json!(true)));
        assert_eq!(style(2), (json!("spaceDelimited"), json!(false)));
        assert_eq!(style(3), (json!("pipeDelimited"), json!(false)));
        assert_eq!(style(4), (json!("form"), json!(false)));
        assert_eq!(style(5), (Value::Null, Value::Null));
        assert_eq!(
            op["parameters"][0]["schema"],
            json!({ "type": "array", "items": { "type": "string" } })
        );
        assert_eq!(
            op["parameters"][6],
            json!({ "name": "n", "in": "query", "example": 3,
                    "schema": { "type": "integer", "nullable": true } })
        );
    }

    #[test]
    fn security_definitions_become_security_schemes() {
        let doc = converted(json!({ "securityDefinitions": {
            "basic": { "type": "basic", "description": "d" },
            "key": { "type": "apiKey", "name": "X-Key", "in": "header" },
            "implicit": { "type": "oauth2", "flow": "implicit", "authorizationUrl": "https://a", "scopes": { "r": "read" } },
            "password": { "type": "oauth2", "flow": "password", "tokenUrl": "https://t" },
            "application": { "type": "oauth2", "flow": "application", "tokenUrl": "https://t", "scopes": {} },
            "accessCode": { "type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a",
                            "tokenUrl": "https://t", "scopes": {}, "x-extra": 1 }
        } }));
        assert_eq!(
            doc["components"]["securitySchemes"],
            json!({
                "basic": { "type": "http", "scheme": "basic", "description": "d" },
                "key": { "type": "apiKey", "name": "X-Key", "in": "header" },
                "implicit": { "type": "oauth2", "flows": { "implicit": {
                    "authorizationUrl": "https://a", "scopes": { "r": "read" } } } },
                "password": { "type": "oauth2", "flows": { "password": {
                    "tokenUrl": "https://t", "scopes": {} } } },
                "application": { "type": "oauth2", "flows": { "clientCredentials": {
                    "tokenUrl": "https://t", "scopes": {} } } },
                "accessCode": { "type": "oauth2", "flows": { "authorizationCode": {
                    "authorizationUrl": "https://a", "tokenUrl": "https://t", "scopes": {} } },
                    "x-extra": 1 }
            })
        );
    }

    #[test]
    fn schemas_lose_their_2_0_keywords() {
        let doc = converted(json!({ "definitions": {
            "Pet": { "type": "object", "discriminator": "kind", "properties": {
                "kind": { "type": "string" },
                "discriminator": { "type": "string", "x-nullable": true },
                "photo": { "type": "file" },
                "owner": { "$ref": "#/definitions/Owner", "type": "object", "description": "d",
                           "x-nullable": true }
            } }
        } }));
        let pet = &doc["components"]["schemas"]["Pet"];
        assert_eq!(pet["discriminator"], json!({ "propertyName": "kind" }));
        let properties = &pet["properties"];
        assert_eq!(
            properties["discriminator"],
            json!({ "type": "string", "nullable": true })
        );
        assert_eq!(
            properties["photo"],
            json!({ "type": "string", "format": "binary" })
        );
        assert_eq!(
            properties["owner"],
            json!({ "$ref": "#/components/schemas/Owner", "description": "d", "nullable": true })
        );
    }

    #[test]
    fn escaped_references_to_shared_objects_resolve() {
        let doc = converted(json!({
            "parameters": { "a/b": { "name": "body", "in": "body", "required": true,
                "schema": { "$ref": "#/definitions/Foo~1Bar" } } },
            "responses": { "Not/Found": { "description": "nf",
                "schema": { "$ref": "#/definitions/Foo~1Bar" } } },
            "definitions": { "Foo/Bar": { "type": "object" } },
            "paths": { "/b": {
                "post": { "produces": ["text/plain"], "parameters": [{ "$ref": "#/parameters/a~1b" }],
                    "responses": { "200": { "$ref": "#/responses/Not~1Found" } } },
                "put": { "parameters": [{ "$ref": "#/parameters/a%7E1b" }], "responses": {} }
            } }
        }));
        let schema = json!({ "$ref": "#/components/schemas/Foo~1Bar" });
        let item = &doc["paths"]["/b"];
        let body = json!({ "content": { JSON: { "schema": schema } }, "required": true });
        assert_eq!(item["post"]["requestBody"], body);
        assert_eq!(item["put"]["requestBody"], body);
        assert_eq!(
            item["post"]["responses"]["200"],
            json!({ "description": "nf", "content": { "text/plain": { "schema": schema } } })
        );
    }

    #[test]
    fn an_operation_body_replaces_its_path_body() {
        let doc = converted(json!({ "paths": {
            "/a": {
                "parameters": [{ "name": "path", "in": "body", "schema": { "type": "integer" } }],
                "post": { "parameters": [{ "name": "op", "in": "body", "schema": { "type": "string" } }],
                    "responses": {} },
                "put": { "parameters": [{ "name": "f", "in": "formData", "type": "string" }],
                    "responses": {} },
                "patch": { "responses": {} }
            },
            "/b": {
                "parameters": [{ "name": "path", "in": "formData", "type": "string" }],
                "post": { "parameters": [{ "name": "op", "in": "body", "schema": { "type": "string" } }],
                    "responses": {} }
            }
        } }));
        let content =
            |path: &str, method: &str| doc["paths"][path][method]["requestBody"]["content"].clone();
        assert_eq!(
            content("/a", "post"),
            json!({ JSON: { "schema": { "type": "string" } } })
        );
        assert_eq!(
            content("/a", "put")[FORM]["schema"]["properties"],
            json!({ "f": { "type": "string" } })
        );
        assert_eq!(
            content("/a", "patch"),
            json!({ JSON: { "schema": { "type": "integer" } } })
        );
        assert_eq!(
            content("/b", "post"),
            json!({ JSON: { "schema": { "type": "string" } } })
        );
    }

    #[test]
    fn empty_media_type_lists_clear_the_global_ones() {
        let doc = converted(json!({
            "consumes": ["application/xml"],
            "produces": ["application/xml"],
            "responses": { "Gone": { "description": "gone", "schema": { "type": "string" } } },
            "paths": { "/x": { "post": {
                "consumes": [],
                "produces": [],
                "parameters": [{ "name": "b", "in": "body", "schema": { "type": "string" } }],
                "responses": {
                    "200": { "description": "ok", "schema": { "type": "string" } },
                    "410": { "$ref": "#/responses/Gone" }
                }
            } } }
        }));
        let op = &doc["paths"]["/x"]["post"];
        let json = json!({ JSON: { "schema": { "type": "string" } } });
        assert_eq!(op["requestBody"]["content"], json);
        assert_eq!(op["responses"]["200"]["content"], json);
        assert_eq!(op["responses"]["410"]["content"], json);
        assert!(
            doc["components"]["responses"]["Gone"]["content"]
                .get("application/xml")
                .is_some()
        );
    }

    #[test]
    fn form_arrays_keep_their_collection_format() {
        let fields = json!([
            { "name": "s", "in": "formData", "type": "array", "items": { "type": "string" },
              "collectionFormat": "ssv" },
            { "name": "p", "in": "formData", "type": "array", "items": { "type": "string" },
              "collectionFormat": "pipes" },
            { "name": "t", "in": "formData", "type": "array", "items": { "type": "string" },
              "collectionFormat": "tsv" }
        ]);
        let encoding = json!({
            "s": { "style": "spaceDelimited", "explode": false },
            "p": { "style": "pipeDelimited", "explode": false },
            "t": { "style": "form", "explode": false }
        });
        let form = operation_of(json!({ "parameters": fields, "responses": {} }));
        assert_eq!(form["requestBody"]["content"][FORM]["encoding"], encoding);
        let mut fields = fields;
        fields
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "f", "in": "formData", "type": "file" }));
        let multipart = operation_of(json!({ "parameters": fields, "responses": {} }));
        assert_eq!(
            multipart["requestBody"]["content"][MULTIPART]["encoding"],
            encoding
        );
    }

    #[test]
    fn examples_match_media_types_with_parameters() {
        let op = operation_of(json!({
            "produces": ["application/json; charset=utf-8"],
            "responses": { "200": { "description": "ok", "schema": { "type": "string" },
                "examples": { "application/json": "a" } } }
        }));
        assert_eq!(
            op["responses"]["200"]["content"]["application/json; charset=utf-8"]["example"],
            "a"
        );
    }

    #[test]
    fn oauth2_schemes_without_a_known_flow_are_rejected() {
        let error = |scheme: Value| {
            let mut doc = json!({ "swagger": "2.0", "securityDefinitions": { "o": scheme } });
            to_3_0(&mut doc).unwrap_err().to_string()
        };
        let unknown = error(json!({ "type": "oauth2", "flow": "weird" }));
        assert!(
            unknown.contains("`o`") && unknown.contains("flow: weird"),
            "{unknown}"
        );
        let missing = error(json!({ "type": "oauth2" }));
        assert!(
            missing.contains("`o`") && missing.contains("no `flow`"),
            "{missing}"
        );
    }

    #[test]
    fn other_versions_are_rejected() {
        let mut doc = json!({ "swagger": "1.2" });
        let err = to_3_0(&mut doc).unwrap_err();
        assert!(
            err.to_string().contains("Swagger 1.2 is not supported"),
            "{err}"
        );
    }
}
