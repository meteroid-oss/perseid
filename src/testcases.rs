//! The calls the generated tests of an SDK make: one per operation whose arguments the tests
//! can build, answered in-process with a sample of the response the spec declares.

use serde_json::{Value, json};

use crate::{
    api::{FieldType, Resource, Types},
    samples,
};

/// `{index, method, path, path_args, body, response}` for each operation of `resource` the
/// tests call; `index` is the operation's position in `resource.operations`.
pub(crate) fn cases(types: &Types, resource: &Resource) -> Vec<Value> {
    resource
        .operations
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            let mut case = case(types, &serde_json::to_value(op).ok()?)?;
            case["index"] = index.into();
            Some(case)
        })
        .collect()
}

fn case(types: &Types, op: &Value) -> Option<Value> {
    let required = |key: &str| {
        op[key]
            .as_array()
            .is_some_and(|params| params.iter().any(|p| p["required"] == true))
    };
    if required("query_params") || required("header_params") {
        return None;
    }
    // A query in the path template is sent as a query.
    let mut path = op["path"].as_str()?.split('?').next()?.to_owned();
    let mut path_args = Vec::new();
    for param in op["typed_path_params"].as_array()? {
        let name = param["name"].as_str()?;
        if op["path_styles"][name]["type"].is_object() {
            return None;
        }
        let (kind, value) = match param["type"]["id"].as_str()? {
            "Int16" | "UInt16" | "Int32" | "Int64" | "UInt64" => ("integer", "1".to_owned()),
            "Float" | "Double" => ("number", "1.5".to_owned()),
            "Bool" => ("boolean", "true".to_owned()),
            _ => ("string", segment(name)),
        };
        path = path.replace(&format!("{{{name}}}"), &value);
        path_args.push(json!({ "name": name, "kind": kind, "value": value }));
    }
    let method = op["method"].as_str()?.to_uppercase();
    let body = match op["request_body_kind"].as_str()? {
        "none" => Value::Null,
        "json" | "form" if op["request_body_is_list"] != true => {
            let schema = op["request_body_schema_name"].as_str()?;
            json!({ "schema": schema, "json": ascii_json(&samples::minimal(types, schema)) })
        }
        _ => return None,
    };
    Some(json!({
        "method": method,
        "path": path,
        "path_args": path_args,
        "body": body,
        "response": response(types, op)?,
    }))
}

/// `{status, content_type, body}` of the success response of `op`, `body` as text.
fn response(types: &Types, op: &Value) -> Option<Value> {
    let named = |key: &str| op[key].as_str().map(|name| samples::minimal(types, name));
    let (content_type, body) = if op["response_is_event_stream"] == true {
        let data = match named("event_schema_name") {
            Some(event) => ascii_json(&event),
            None => "sample".to_owned(),
        };
        ("text/event-stream", format!("data: {data}\n\n"))
    } else if op["response_is_binary"] == true {
        ("application/octet-stream", "sample".to_owned())
    } else if op["response_is_text"] == true {
        ("text/plain", "sample".to_owned())
    } else if let Some(sample) = named("response_body_schema_name") {
        let sample = match op["response_body_is_list"] == true {
            true => json!([sample]),
            false => sample,
        };
        ("application/json", ascii_json(&sample))
    } else if op["response_body_json_type"].is_object() {
        let ty: FieldType = serde_json::from_value(op["response_body_json_type"].clone()).ok()?;
        (
            "application/json",
            ascii_json(&samples::minimal_of(types, &ty)),
        )
    } else {
        return Some(json!({ "status": 204, "content_type": null, "body": "" }));
    };
    Some(json!({ "status": 200, "content_type": content_type, "body": body }))
}

/// A path segment for the parameter `name`: its name, when it needs no percent-encoding.
fn segment(name: &str) -> String {
    match name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        true => name.to_owned(),
        false => "value".to_owned(),
    }
}

/// Compact JSON text with every character beyond ASCII escaped, which a string literal of any
/// language holds as is.
fn ascii_json(value: &Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Filters;

    fn cases_of(spec: &str) -> Vec<Value> {
        let spec = crate::spec::read(spec, std::path::Path::new(".")).unwrap();
        let filters = Filters {
            include_mode: Default::default(),
            excluded: Default::default(),
            specified: Default::default(),
            pagination: vec![],
            reserved: Default::default(),
            names: Default::default(),
        };
        let api = crate::spec::api(&spec, &filters).unwrap();
        api.resources
            .values()
            .flat_map(|resource| cases(&api.types, resource))
            .collect()
    }

    #[test]
    fn every_petstore_operation_is_called_with_a_sample_body_and_response() {
        let cases = cases_of("tests/fixtures/petstore.yaml");
        let calls: Vec<_> = cases
            .iter()
            .map(|c| format!("{} {} {}", c["method"], c["path"], c["response"]["status"]))
            .collect();
        assert_eq!(
            calls,
            [
                r#""GET" "/pets" 200"#,
                r#""POST" "/pets" 200"#,
                r#""GET" "/pets/pet_id" 200"#,
                r#""DELETE" "/pets/pet_id" 204"#,
            ]
        );
        assert_eq!(cases[1]["body"]["json"], r#"{"name":"sample"}"#);
        assert_eq!(cases[2]["path_args"][0]["value"], "pet_id");
    }

    #[test]
    fn json_text_is_ascii() {
        let text = ascii_json(&json!({ "a": "\u{e9}\u{1f389}" }));
        assert_eq!(text, "{\"a\":\"\\u00e9\\ud83c\\udf89\"}");
    }
}
