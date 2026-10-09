//! The operations an SDK's README shows: a plain call, a paginated list, an event stream and a
//! download, picked from the API with arguments every language can write as literals.

use heck::ToSnakeCase as _;
use serde_json::{Map, Value, json};

use crate::api::Api;

/// `{call, list, stream, download, create}`: a plain call, a paginated list, an event stream, a
/// binary response and a call with body fields, each `null` when no operation fits.
pub(crate) fn examples(api: &Api) -> Value {
    serde_json::to_value(api).map_or(Value::Null, |model| pick(&model))
}

/// Whether an operation and its example fit a tier of the examples a README prefers.
type Tier<'a> = &'a dyn Fn(&Value, &Value) -> bool;

fn pick(model: &Value) -> Value {
    let types = &model["types"];
    let mut ops = Vec::new();
    collect(&model["resources"], &mut ops);
    let examples: Vec<(&Value, Value)> = (ops.iter())
        .filter_map(|(resource, op)| Some((*op, example(types, resource, op)?)))
        .collect();
    // Examples whose body nests no list or struct come first, as they read more easily.
    let flat = |ex: &Value| {
        (ex["body"]["fields"].as_array().into_iter().flatten())
            .all(|f| f["kind"] != "list" && f["kind"] != "object")
    };
    let first = |tiers: &[Tier]| {
        [true, false].into_iter().find_map(|only_flat| {
            tiers.iter().find_map(|tier| {
                (examples.iter())
                    .find(|(op, ex)| tier(op, ex) && (flat(ex) || !only_flat))
                    .map(|(_, ex)| ex.clone())
            })
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
        &|op, _| get(op) && plain(op) && json_response(op) && has_path(op) && no_body(op),
        &|op, _| get(op) && plain(op) && json_response(op) && no_body(op),
        &|op, _| plain(op) && json_response(op),
        &|op, _| plain(op),
    ]);
    let list = first(&[&|op, _| paginated(op) && !streams(op)]);
    let stream = first(&[
        &|op, _| streams(op) && op.get("event_schema_name").is_some(),
        &|op, _| streams(op),
    ]);
    let binary = |op: &Value| op["response_is_binary"] == true;
    let download = first(&[&|op, _| binary(op) && get(op), &|op, _| binary(op)]);
    let call = call.or_else(|| list.clone());
    let create = first(&[&|op, ex| {
        plain(op)
            && ex["body"]["fields"]
                .as_array()
                .is_some_and(|f| !f.is_empty())
    }]);
    json!({ "call": call, "list": list, "stream": stream, "download": download, "create": create })
}

/// Every `(resource, operation)`, child resources after their parent.
pub(crate) fn collect<'a>(resources: &'a Value, out: &mut Vec<(&'a Value, &'a Value)>) {
    let resources: Vec<&Value> = match resources {
        Value::Array(list) => list.iter().collect(),
        Value::Object(map) => map.values().collect(),
        _ => vec![],
    };
    for resource in resources {
        for op in resource["operations"].as_array().into_iter().flatten() {
            out.push((resource, op));
        }
    }
}

/// The example of `op`, when its required arguments can all be written as literals.
fn example(types: &Value, resource: &Value, op: &Value) -> Option<Value> {
    build(types, resource, op, false).map(|(example, _)| example)
}

/// The example of `op` with the arguments literals can hold, and whether it holds every
/// required one.
pub(crate) fn partial_example(
    types: &Value,
    resource: &Value,
    op: &Value,
) -> Option<(Value, bool)> {
    build(types, resource, op, true)
}

fn build(types: &Value, resource: &Value, op: &Value, partial: bool) -> Option<(Value, bool)> {
    let mut fill = Fill {
        types,
        partial,
        complete: true,
    };
    let required = |key: &str| {
        op[key]
            .as_array()
            .is_some_and(|params| params.iter().any(|p| p["required"] == true))
    };
    if required("query_params") || required("header_params") {
        fill.missing()?;
    }
    let mut path_args = Vec::new();
    for param in op["typed_path_params"].as_array().into_iter().flatten() {
        match path_literal(op, types, param) {
            Some(arg) => path_args.push(arg),
            None => fill.missing()?,
        }
    }
    let body = match op["request_body_kind"].as_str()? {
        "none" => Value::Null,
        "json" => fill.json_body(op)?,
        _ => {
            fill.missing()?;
            Value::Null
        }
    };
    let schema = |key: &str| op.get(key).cloned().unwrap_or(Value::Null);
    let result = match op["response_body_is_list"] == true {
        true => Value::Null,
        false => schema("response_body_schema_name"),
    };
    let example = json!({
        "resource": resource["name"],
        "resource_path": resource["path"],
        "operation": op,
        "path_args": path_args,
        "body": body,
        "result": result,
        "item": op["pagination"]["item_schema"],
    });
    Some((example, fill.complete))
}

/// How many structs deep an example nests at most.
const MAX_DEPTH: usize = 3;

/// Writes the literals of an example. Where no literal fits, it gives up, or with `partial`
/// leaves the argument out; either way the example is no longer `complete`.
struct Fill<'a> {
    types: &'a Value,
    partial: bool,
    complete: bool,
}

impl Fill<'_> {
    /// Marks an argument missing: `None` to give up, unless `partial`.
    fn missing(&mut self) -> Option<()> {
        self.complete = false;
        self.partial.then_some(())
    }

    /// A JSON body of a named object schema, with its required fields as literals.
    fn json_body(&mut self, op: &Value) -> Option<Value> {
        if op["request_body_optional"] == true {
            self.missing()?;
        }
        let name = op["request_body_schema_name"].as_str();
        let Some(name) = name
            .filter(|n| op["request_body_is_list"] != true && self.types[*n]["kind"] == "struct")
        else {
            self.missing()?;
            return Some(Value::Null);
        };
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
        let fields = self.types[name]["fields"].as_array().into_iter().flatten();
        if fields
            .filter_map(|f| f["name"].as_str())
            .any(|f| taken.contains(&f.to_snake_case()))
        {
            self.missing()?;
        }
        let mut fields = self.struct_fields(name, &[])?;
        if fields.iter().any(|f| op["stream_property"] == f["name"]) {
            self.missing()?;
            fields.retain(|f| op["stream_property"] != f["name"]);
        }
        Some(json!({ "schema": name, "fields": fields }))
    }

    /// The required fields of the struct `name` as literals, `outer` the structs it is nested in.
    fn struct_fields(&mut self, name: &str, outer: &[&str]) -> Option<Vec<Value>> {
        let schema = &self.types[name];
        if schema["kind"] != "struct" || outer.contains(&name) || outer.len() >= MAX_DEPTH {
            return None;
        }
        let outer = [outer, &[name]].concat();
        let defaults = schema["discriminator_defaults"].as_object();
        let mut fields = Vec::new();
        for field in schema["fields"].as_array()? {
            let name = field["name"].as_str()?;
            if field["flatten"] == true {
                self.missing()?;
                continue;
            }
            if field["required"] != true {
                continue;
            }
            let defaulted = defaults.is_some_and(|d| d.contains_key(name));
            if field["nullable"] == true || defaulted {
                self.missing()?;
                continue;
            }
            match self.value(name, &field["type"], &field["example"], &outer) {
                Some(value) => fields.push(value),
                None => self.missing()?,
            }
        }
        Some(fields)
    }

    /// The literal of a value named `name` of type `ty`, its spec `example` when one fits: a
    /// scalar, an enum, a list of one item or a struct of its required fields.
    fn value(&mut self, name: &str, ty: &Value, example: &Value, outer: &[&str]) -> Option<Value> {
        let id = ty["id"].as_str()?;
        let schema = &self.types[ty["name"].as_str().unwrap_or_default()];
        let value = match (id, schema["kind"].as_str()) {
            ("String", _) | ("SchemaRef", Some("string_alias")) => text(name, example),
            ("Int16" | "UInt16" | "Int32" | "Int64" | "UInt64", _) => {
                json!(example.as_i64().filter(|n| *n >= 0).unwrap_or(1))
            }
            ("Float" | "Double", _) => json!(1.5),
            ("Bool", _) => json!(example.as_bool().unwrap_or(true)),
            ("Uuid", _) => json!("3fa85f64-5717-4562-b3fc-2c963f66afa6"),
            ("Date", _) => json!("2024-01-02"),
            ("List", _) => {
                let item = self.value(name, &ty["inner"], &example[0], outer)?;
                let item_type = item_type(&item);
                return Some(json!({
                    "name": name, "kind": "list", "type": "List", "item": item_type, "items": [item],
                }));
            }
            ("SchemaRef", Some("string_enum")) => {
                let values = schema["values"].as_array()?;
                let value = (values.iter().find(|v| *v == example))
                    .or(values.first())
                    .filter(|v| v.as_str().is_some_and(plain_text))?;
                let mut lit = literal(name, "Enum", value.clone())?;
                lit["schema"] = ty["name"].clone();
                return Some(lit);
            }
            ("SchemaRef", Some("struct")) => {
                let schema = ty["name"].as_str()?;
                let fields = self.struct_fields(schema, outer)?;
                return Some(json!({
                    "name": name, "kind": "object", "type": "SchemaRef", "schema": schema,
                    "fields": fields,
                }));
            }
            _ => return None,
        };
        literal(name, id, value)
    }
}

/// `{kind, type, schema, item}` of the literal `lit`, which names its type.
fn item_type(lit: &Value) -> Value {
    let mut ty = Map::new();
    for key in ["kind", "type", "schema", "item"] {
        if let Some(value) = lit.get(key) {
            ty.insert(key.into(), value.clone());
        }
    }
    Value::Object(ty)
}

/// The argument for the path parameter `param`, when every language can write it as a
/// literal: text, its spec example when it has one, or a scalar sent in the `simple` style,
/// among which the first value of a string enum of `types`.
pub(crate) fn path_literal(op: &Value, types: &Value, param: &Value) -> Option<Value> {
    let (name, ty) = (param["name"].as_str()?, &param["type"]);
    let style = &op["path_styles"][name];
    if style["type"].is_object() && style["style"] != "simple" {
        return None;
    }
    let id = ty["id"].as_str()?;
    let schema = &types[ty["name"].as_str().unwrap_or_default()];
    let value = match id {
        "String" | "SchemaRef" if !style["type"].is_object() => text(name, &param["example"]),
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

/// The example value of a string named `name`: its spec `example`, else its name, when a
/// literal can hold it as is.
fn text(name: &str, example: &Value) -> Value {
    let text = example.as_str().filter(|e| plain_text(e));
    json!(text.unwrap_or(if plain_text(name) { name } else { "string" }))
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
        examples(&api_of(
            &crate::spec::read(spec, std::path::Path::new(".")).unwrap(),
        ))
    }

    fn api_of(spec: &str) -> Api {
        let filters = Filters {
            include_mode: Default::default(),
            excluded: Default::default(),
            specified: Default::default(),
            pagination: vec![],
            detect_pagination: true,
            reserved: Default::default(),
            names: Default::default(),
            resources: Default::default(),
            models: Default::default(),
            uuid_strings: false,
        };
        crate::spec::api(spec, &filters).unwrap()
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
        assert!(petstore["download"].is_null());

        let features = examples_of("tests/fixtures/features.yaml");
        assert_eq!(
            summary(&features["call"]),
            "encoding.retrieve_scenario_path"
        );
        assert_eq!(features["call"]["path_args"][0]["value"], "segment");
        assert_eq!(summary(&features["list"]), "errors.list_scenarios_pages");
        assert_eq!(features["list"]["item"], "Widget");
        assert_eq!(summary(&features["download"]), "content.download_blob");
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
                    { "name": "parts", "type": { "id": "List", "inner": { "id": "JsonObject" } },
                      "required": true, "nullable": false },
                ] },
            },
            "resources": [{ "name": "chat", "path": ["chat"], "operations": [
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
            json!({ "call": null, "list": null, "stream": null, "download": null, "create": null })
        );
    }

    #[test]
    fn required_lists_and_structs_are_built_from_their_required_fields() {
        let field = |name: &str, ty: Value| json!({ "name": name, "type": ty, "required": true });
        let op = |name: &str, body: &str| {
            json!({ "name": name, "method": "post", "request_body_kind": "json",
                "request_body_schema_name": body, "path_params": [], "typed_path_params": [],
                "query_params": [], "header_params": [] })
        };
        let line = json!({ "id": "SchemaRef", "name": "Line" });
        let model = |ops: Value| {
            json!({
                "types": {
                    "Order": { "kind": "struct", "fields": [
                        field("lines", json!({ "id": "List", "inner": line })),
                        { "name": "note", "type": { "id": "String" }, "required": false },
                        { "name": "tags", "type": { "id": "List", "inner": { "id": "String" } },
                          "required": true, "nullable": false, "example": ["gift"] },
                    ] },
                    "Line": { "kind": "struct", "fields": [
                        field("size", json!({ "id": "SchemaRef", "name": "Size" })),
                        { "name": "parent", "type": line, "required": false },
                    ] },
                    "Size": { "kind": "string_enum", "values": ["small", "large"] },
                    "Node": { "kind": "struct", "fields": [
                        field("next", json!({ "id": "SchemaRef", "name": "Node" })),
                    ] },
                    "Note": { "kind": "struct", "fields": [
                        field("text", json!({ "id": "String" })),
                    ] },
                },
                "resources": [{ "name": "orders", "path": ["orders"], "operations": ops }],
            })
        };
        let flat = pick(&model(json!([
            op("create", "Order"),
            op("annotate", "Note")
        ])));
        assert_eq!(
            summary(&flat["create"]),
            "orders.annotate",
            "flat bodies come first"
        );

        let nested = pick(&model(json!([op("link", "Node"), op("create", "Order")])));
        let create = &nested["create"];
        assert_eq!(summary(create), "orders.create", "`Node` nests itself");
        let size = json!({ "name": "size", "kind": "enum", "type": "Enum", "int64": false,
            "unsigned64": false, "value": "small", "schema": "Size" });
        assert_eq!(
            create["body"]["fields"],
            json!([
                { "name": "lines", "kind": "list", "type": "List",
                  "item": { "kind": "object", "type": "SchemaRef", "schema": "Line" },
                  "items": [{ "name": "lines", "kind": "object", "type": "SchemaRef",
                    "schema": "Line", "fields": [size] }] },
                { "name": "tags", "kind": "list", "type": "List",
                  "item": { "kind": "string", "type": "String" },
                  "items": [{ "name": "tags", "kind": "string", "type": "String", "int64": false,
                    "unsigned64": false, "value": "gift" }] },
            ])
        );
    }

    #[test]
    fn spec_examples_become_literals_when_every_language_can_write_them() {
        assert!(plain_text("Hello there"));
        assert!(!plain_text("say \"hi\""));
        assert!(!plain_text("${HOME}"));
        assert!(!plain_text("é"));
    }

    #[test]
    fn path_parameters_take_their_spec_example() {
        let id = |example: Value| {
            json!({ "name": "id", "in": "path", "required": true,
                "schema": { "$ref": "#/components/schemas/CustomerId" }, "example": example })
        };
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "Shop", "version": "1" },
            "paths": {
                "/customers/{id}": { "get": {
                    "operationId": "get_customer", "tags": ["customers"],
                    "parameters": [id(Value::Null)],
                    "responses": { "200": { "description": "ok" } },
                } },
                "/carts/{id}": { "get": {
                    "operationId": "get_cart", "tags": ["carts"],
                    "parameters": [id(json!("cart \"7\""))],
                    "responses": { "200": { "description": "ok" } },
                } },
            },
            "components": { "schemas": {
                "CustomerId": { "type": "string", "examples": ["cus_7n42"] },
            } },
        });
        let model = serde_json::to_value(api_of(&spec.to_string())).unwrap();
        let mut ops = Vec::new();
        collect(&model["resources"], &mut ops);
        let arg = |id: &str| {
            let (resource, op) = ops.iter().find(|(_, op)| op["id"] == id).unwrap();
            let (example, complete) = partial_example(&model["types"], resource, op).unwrap();
            assert!(complete);
            example["path_args"][0]["value"].clone()
        };
        assert_eq!(arg("get_customer"), "cus_7n42", "the schema's example");
        assert_eq!(arg("get_cart"), "id", "no literal holds the quotes as is");
    }

    #[test]
    fn partial_examples_leave_out_what_no_literal_holds() {
        let model = json!({
            "types": {
                "Order": { "kind": "struct", "fields": [
                    { "name": "sku", "type": { "id": "String" }, "required": true },
                    { "name": "meta", "type": { "id": "JsonObject" }, "required": true },
                ] },
            },
            "resources": [{ "name": "orders", "path": ["orders"], "operations": [
                { "id": "create_order", "name": "create", "method": "post",
                  "request_body_kind": "json", "request_body_schema_name": "Order",
                  "path_params": ["shop"], "header_params": [],
                  "typed_path_params": [{ "name": "shop", "type": { "id": "String" } }],
                  "query_params": [{ "name": "dry_run", "ident": "dry_run", "required": true }] },
                { "id": "upload", "name": "upload", "method": "post",
                  "request_body_kind": "multipart", "path_params": [], "typed_path_params": [],
                  "query_params": [], "header_params": [] },
            ] }],
        });
        assert_eq!(
            pick(&model)["create"],
            Value::Null,
            "READMEs show complete calls only"
        );
        let ops = model["resources"][0]["operations"].as_array().unwrap();
        let orders = &model["resources"][0];
        let (create, complete) = partial_example(&model["types"], orders, &ops[0]).unwrap();
        assert!(!complete);
        assert_eq!(create["path_args"][0]["value"], "shop");
        assert_eq!(
            create["body"],
            json!({ "schema": "Order", "fields": [{ "name": "sku", "kind": "string",
                "type": "String", "int64": false, "unsigned64": false, "value": "sku" }] })
        );
        let (upload, complete) = partial_example(&model["types"], orders, &ops[1]).unwrap();
        assert!(!complete && upload["body"].is_null());
    }
}
