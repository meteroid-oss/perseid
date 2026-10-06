//! The operations an SDK's README shows: a plain call, a paginated list and an event stream,
//! picked from the API with arguments every language can write as literals.

use heck::ToSnakeCase as _;
use serde_json::{Map, Value, json};

use crate::api::Api;

/// `{call, list, stream, create}`: a plain call, a paginated list, an event stream and a call
/// with body fields, each `null` when no operation fits.
pub(crate) fn examples(api: &Api) -> Value {
    serde_json::to_value(api).map_or(Value::Null, |model| pick(&model))
}

fn pick(model: &Value) -> Value {
    let types = &model["types"];
    let mut ops = Vec::new();
    collect(&model["resources"], &mut ops);
    let first = |tiers: &[&dyn Fn(&Value) -> bool]| {
        tiers.iter().find_map(|tier| {
            ops.iter()
                .filter(|(_, op)| tier(op))
                .find_map(|(resource, op)| example(types, resource, op))
        })
    };
    let get = |op: &Value| op["method"] == "get";
    let paginated = |op: &Value| op.get("pagination").is_some_and(|p| !p.is_null());
    let streams = |op: &Value| op["response_is_event_stream"] == true;
    let plain = |op: &Value| !paginated(op) && !streams(op);
    let json_response = |op: &Value| {
        op.get("response_body_schema_name").is_some() || op.get("response_body_json_type").is_some()
    };
    let has_path = |op: &Value| op["path_params"].as_array().is_some_and(|p| !p.is_empty());
    let no_body = |op: &Value| op["request_body_kind"] == "none";
    let call = first(&[
        &|op| get(op) && plain(op) && json_response(op) && has_path(op) && no_body(op),
        &|op| get(op) && plain(op) && json_response(op) && no_body(op),
        &|op| plain(op) && json_response(op),
        &|op| plain(op),
    ]);
    let list = first(&[&|op| paginated(op) && !streams(op)]);
    let stream = first(&[
        &|op| streams(op) && op.get("event_schema_name").is_some(),
        &|op| streams(op),
    ]);
    let call = call.or_else(|| list.clone());
    let create = (ops.iter())
        .filter(|(_, op)| plain(op))
        .filter_map(|(resource, op)| example(types, resource, op))
        .find(|ex| {
            ex["body"]["fields"]
                .as_array()
                .is_some_and(|f| !f.is_empty())
        });
    json!({ "call": call, "list": list, "stream": stream, "create": create })
}

/// Every `(resource, operation)`, resources in order, subresources after their parent's.
fn collect<'a>(resources: &'a Value, out: &mut Vec<(&'a str, &'a Value)>) {
    let resources: Vec<&Value> = match resources {
        Value::Array(list) => list.iter().collect(),
        Value::Object(map) => map.values().collect(),
        _ => vec![],
    };
    for resource in resources {
        let name = resource["name"].as_str().unwrap_or_default();
        for op in resource["operations"].as_array().into_iter().flatten() {
            out.push((name, op));
        }
        collect(&resource["subresources"], out);
    }
}

/// The example of `op`, when its required arguments can all be written as literals.
fn example(types: &Value, resource: &str, op: &Value) -> Option<Value> {
    let required = |key: &str| {
        op[key]
            .as_array()
            .is_some_and(|params| params.iter().any(|p| p["required"] == true))
    };
    if required("query_params") || required("header_params") {
        return None;
    }
    let mut path_args = Vec::new();
    for param in op["typed_path_params"].as_array().into_iter().flatten() {
        let name = param["name"].as_str()?;
        path_args.push(path_literal(op, types, name, &param["type"])?);
    }
    let body = match op["request_body_kind"].as_str() {
        Some("none") => Value::Null,
        Some("json") => json_body(types, op)?,
        _ => return None,
    };
    let schema = |key: &str| op.get(key).cloned().unwrap_or(Value::Null);
    let result = match op["response_body_is_list"] == true {
        true => Value::Null,
        false => schema("response_body_schema_name"),
    };
    Some(json!({
        "resource": resource,
        "operation": op,
        "path_args": path_args,
        "body": body,
        "result": result,
        "item": op["pagination"]["item_schema"],
    }))
}

/// A required JSON body of a named object schema, with its required fields as literals.
fn json_body(types: &Value, op: &Value) -> Option<Value> {
    if op["request_body_optional"] == true || op["request_body_is_list"] == true {
        return None;
    }
    let name = op["request_body_schema_name"].as_str()?;
    let schema = &types[name];
    if schema["kind"] != "struct" {
        return None;
    }
    let defaults = schema["discriminator_defaults"].as_object();
    // Python takes the fields as keyword arguments, unless one is named like a parameter.
    let mut taken = vec!["self".to_owned(), "t".to_owned()];
    let params = ["path_params", "query_params", "header_params"]
        .iter()
        .flat_map(|key| op[key].as_array().into_iter().flatten())
        .filter_map(|p| {
            (p.as_str())
                .or_else(|| p["ident"].as_str())
                .or_else(|| p["name"].as_str())
        });
    taken.extend(params.map(|p| p.to_snake_case()));
    let mut fields = Vec::new();
    for field in schema["fields"].as_array()? {
        let name = field["name"].as_str()?;
        if field["flatten"] == true || taken.contains(&name.to_snake_case()) {
            return None;
        }
        if field["required"] != true {
            continue;
        }
        let defaulted = defaults.is_some_and(|d| d.contains_key(name));
        if field["nullable"] == true || defaulted || op["stream_property"] == name {
            return None;
        }
        let example = &field["example"];
        let id = field["type"]["id"].as_str()?;
        let value = match id {
            "String" => match example.as_str().filter(|s| plain_text(s)) {
                Some(example) => json!(example),
                None => text(name),
            },
            "Int16" | "UInt16" | "Int32" | "Int64" | "UInt64" => {
                json!(example.as_i64().filter(|n| *n >= 0).unwrap_or(1))
            }
            "Bool" => json!(example.as_bool().unwrap_or(true)),
            _ => return None,
        };
        fields.push(literal(name, id, value)?);
    }
    Some(json!({ "schema": name, "fields": fields }))
}

/// The argument for the path parameter `name` of type `ty`, when every language can write
/// it as a literal: text, or a scalar sent in the `simple` style, among which the first
/// value of a string enum of `types`.
pub(crate) fn path_literal(op: &Value, types: &Value, name: &str, ty: &Value) -> Option<Value> {
    let style = &op["path_styles"][name];
    if style["type"].is_object() && style["style"] != "simple" {
        return None;
    }
    let id = ty["id"].as_str()?;
    let schema = &types[ty["name"].as_str().unwrap_or_default()];
    let value = match id {
        "String" | "SchemaRef" if !style["type"].is_object() => text(name),
        "SchemaRef" if schema["kind"] == "string_enum" => {
            let value = schema["values"]
                .get(0)
                .filter(|v| v.as_str().is_some_and(plain_text))?;
            let mut lit = literal(name, "Enum", value.clone())?;
            lit["schema"] = ty["name"].clone();
            return Some(lit);
        }
        "Int16" | "UInt16" | "Int32" | "Int64" | "UInt64" => json!(1),
        "Float" | "Double" => json!(1.5),
        "Bool" => json!(true),
        "Uuid" => json!("3fa85f64-5717-4562-b3fc-2c963f66afa6"),
        "Date" => json!("2024-01-02"),
        _ => return None,
    };
    literal(name, id, value)
}

/// `{name, kind, type, int64, unsigned64, value}`: `kind` is the JSON type of `value`, `type`
/// the `FieldType` id it is passed as.
pub(crate) fn literal(name: &str, id: &str, value: Value) -> Option<Value> {
    let kind = match id {
        "String" | "SchemaRef" => "string",
        "Int16" | "UInt16" | "Int32" | "Int64" | "UInt64" => "integer",
        "Float" | "Double" => "number",
        "Bool" => "boolean",
        "Uuid" => "uuid",
        "Date" => "date",
        "Enum" => "enum",
        _ => return None,
    };
    let mut map = Map::new();
    map.insert("name".into(), name.into());
    map.insert("kind".into(), kind.into());
    map.insert("type".into(), id.into());
    map.insert("int64".into(), matches!(id, "Int64" | "UInt64").into());
    // Java holds an unsigned 64-bit integer in a `BigInteger`.
    map.insert("unsigned64".into(), (id == "UInt64").into());
    map.insert("value".into(), value);
    Some(Value::Object(map))
}

/// The example value of a string named `name`: its name, when a literal can hold it as is.
fn text(name: &str) -> Value {
    json!(if plain_text(name) { name } else { "string" })
}

/// Text every language's string literal holds as is.
fn plain_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && text
            .chars()
            .all(|c| c == ' ' || (c.is_ascii_alphanumeric() || "-_.,:;!?@/+=()".contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Filters;

    fn examples_of(spec: &str) -> Value {
        let spec = crate::spec::read(spec, std::path::Path::new(".")).unwrap();
        let filters = Filters {
            include_mode: Default::default(),
            excluded: Default::default(),
            specified: Default::default(),
            pagination: vec![],
            reserved: Default::default(),
            names: Default::default(),
            uuid_strings: false,
        };
        let api = crate::spec::api(&spec, &filters).unwrap();
        examples(&api)
    }

    fn summary(example: &Value) -> String {
        format!(
            "{}.{}",
            example["resource"].as_str().unwrap(),
            example["operation"]["name"].as_str().unwrap()
        )
    }

    #[test]
    fn a_retrieve_is_preferred_and_streams_and_lists_are_found() {
        let petstore = examples_of("tests/fixtures/petstore.yaml");
        assert_eq!(summary(&petstore["call"]), "pets.retrieve");
        assert_eq!(petstore["call"]["path_args"][0]["value"], "pet_id");
        assert_eq!(petstore["call"]["result"], "Pet");
        assert!(petstore["list"].is_null() && petstore["stream"].is_null());

        let features = examples_of("tests/fixtures/features.yaml");
        assert_eq!(
            summary(&features["call"]),
            "encoding.retrieve_scenario_path"
        );
        assert_eq!(features["call"]["path_args"][0]["value"], "segment");
        assert_eq!(summary(&features["list"]), "errors.list_scenarios_pages");
        assert_eq!(features["list"]["item"], "Widget");
        let stream = &features["stream"];
        assert_eq!(summary(stream), "streaming.create_completion_stream");
        assert_eq!(stream["body"]["schema"], "CompletionRequest");
        assert_eq!(
            stream["body"]["fields"],
            json!([{ "name": "prompt", "kind": "string", "type": "String", "int64": false,
                "unsigned64": false, "value": "prompt" }])
        );
    }

    #[test]
    fn operations_whose_arguments_are_not_literals_are_skipped() {
        let realworld = examples_of("tests/fixtures/realworld.yaml");
        let call = summary(&realworld["call"]);
        assert_ne!(call, "issues.retrieve", "its path parameter is an integer");
        assert_eq!(call, "charges.retrieve");
        let torture = examples_of("tests/fixtures/torture.yaml");
        assert_ne!(
            summary(&torture["create"]),
            "class.reserved",
            "its body has a `self` field, which Python cannot take as a keyword argument"
        );

        let model = json!({
            "types": {
                "Message": { "kind": "struct", "fields": [
                    { "name": "parts", "type": { "id": "List" }, "required": true, "nullable": false },
                ] },
            },
            "resources": [{ "name": "chat", "subresources": {}, "operations": [
                { "name": "create_stream", "method": "post", "request_body_kind": "json",
                  "request_body_schema_name": "Message", "response_is_event_stream": true,
                  "path_params": [], "typed_path_params": [], "query_params": [], "header_params": [] },
                { "name": "watch", "method": "get", "request_body_kind": "none",
                  "response_is_event_stream": true, "path_params": [], "typed_path_params": [],
                  "header_params": [], "query_params": [{ "name": "topic", "required": true }] },
            ] }],
        });
        assert_eq!(
            pick(&model),
            json!({ "call": null, "list": null, "stream": null, "create": null })
        );
    }

    #[test]
    fn spec_examples_become_literals_when_every_language_can_write_them() {
        assert!(plain_text("Hello there"));
        assert!(!plain_text("say \"hi\""));
        assert!(!plain_text("${HOME}"));
        assert!(!plain_text("é"));
    }
}
