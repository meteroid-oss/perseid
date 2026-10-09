//! Validation keywords of fields and parameters, which SDKs leave to the server and programs
//! wrapping them, such as CLIs, check input against and document.

use std::collections::BTreeMap;

use schemars::schema::{InstanceType, Schema, SchemaObject, SingleOrVec};
use serde::{Deserialize, Serialize};

use super::{
    FieldType,
    types::{extract_nullable_variant, implied_type},
};

/// Bounds are numbers of the schema, `exclusive_*` in the OpenAPI 3.1 form whichever version
/// the spec uses: `exclusiveMinimum: true` and `minimum: 1` are `exclusive_minimum: 1`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub(crate) struct Constraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minimum: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) maximum: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exclusive_minimum: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exclusive_maximum: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) multiple_of: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) min_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) min_items: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_items: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) unique_items: bool,
    /// The `format` of a string that the type does not already tell: `email`, `hostname`,
    /// `password`... but not `date-time` or `uuid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
    /// The constraints of the items of a list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) items: Option<Box<Constraints>>,
}

impl Constraints {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The constraints of `obj`, with those of the schema it makes nullable or is the only
    /// `allOf` part of.
    pub(crate) fn from_schema(obj: &SchemaObject) -> Self {
        let own = Self::own(obj);
        match wrapped(obj) {
            Some(inner) => own.or(Self::from_schema(inner)),
            None => own,
        }
    }

    fn own(obj: &SchemaObject) -> Self {
        let number = obj.number.as_deref();
        let bound = |pick: fn(&schemars::schema::NumberValidation) -> Option<f64>| {
            number.and_then(pick).and_then(json_number)
        };
        let string = obj.string.as_deref();
        let array = obj.array.as_deref();
        let items = match array.and_then(|a| a.items.as_ref()) {
            Some(SingleOrVec::Single(item)) => match &**item {
                Schema::Object(item) => Some(Self::from_schema(item)),
                Schema::Bool(_) => None,
            },
            _ => None,
        };
        Self {
            minimum: bound(|n| n.minimum),
            maximum: bound(|n| n.maximum),
            exclusive_minimum: bound(|n| n.exclusive_minimum),
            exclusive_maximum: bound(|n| n.exclusive_maximum),
            multiple_of: bound(|n| n.multiple_of),
            min_length: string.and_then(|s| s.min_length),
            max_length: string.and_then(|s| s.max_length),
            pattern: string.and_then(|s| s.pattern.clone()),
            min_items: array.and_then(|a| a.min_items),
            max_items: array.and_then(|a| a.max_items),
            unique_items: array.and_then(|a| a.unique_items) == Some(true),
            format: obj.format.clone().filter(|format| {
                is_string(obj) && FieldType::of_string_format(Some(format)) == FieldType::String
            }),
            items: items.filter(|c| !c.is_empty()).map(Box::new),
        }
    }

    /// These constraints, with those of `other` they leave unset.
    pub(crate) fn or(self, other: Self) -> Self {
        let items = match (self.items, other.items) {
            (Some(own), Some(other)) => Some(Box::new(own.or(*other))),
            (own, other) => own.or(other),
        };
        Self {
            minimum: self.minimum.or(other.minimum),
            maximum: self.maximum.or(other.maximum),
            exclusive_minimum: self.exclusive_minimum.or(other.exclusive_minimum),
            exclusive_maximum: self.exclusive_maximum.or(other.exclusive_maximum),
            multiple_of: self.multiple_of.or(other.multiple_of),
            min_length: self.min_length.or(other.min_length),
            max_length: self.max_length.or(other.max_length),
            pattern: self.pattern.or(other.pattern),
            min_items: self.min_items.or(other.min_items),
            max_items: self.max_items.or(other.max_items),
            unique_items: self.unique_items || other.unique_items,
            format: self.format.or(other.format),
            items,
        }
    }

    /// These constraints, with those of the alias schema `ty` references, and of the alias
    /// its list items reference.
    pub(crate) fn inherit(&mut self, ty: &FieldType, aliases: &BTreeMap<String, Constraints>) {
        let referenced = |ty: &FieldType| match ty.non_null() {
            FieldType::SchemaRef { name, .. } => aliases.get(name).cloned(),
            _ => None,
        };
        if let Some(alias) = referenced(ty) {
            *self = std::mem::take(self).or(alias);
        }
        if let FieldType::List { inner } | FieldType::Set { inner } = ty.non_null()
            && let Some(alias) = referenced(inner)
        {
            let items = self.items.take().map(|items| *items).unwrap_or_default();
            self.items = Some(Box::new(items.or(alias)));
        }
    }
}

/// The schema `obj` makes nullable (`oneOf: [X, {type: null}]`) or is the only `allOf` part of.
fn wrapped(obj: &SchemaObject) -> Option<&SchemaObject> {
    let sub = obj.subschemas.as_deref()?;
    let part = match (&sub.all_of, sub.one_of.as_ref().or(sub.any_of.as_ref())) {
        (Some(all_of), _) if all_of.len() == 1 => &all_of[0],
        (None, Some(variants)) => extract_nullable_variant(variants)?,
        _ => return None,
    };
    match part {
        Schema::Object(obj) => Some(obj),
        Schema::Bool(_) => None,
    }
}

fn is_string(obj: &SchemaObject) -> bool {
    match &obj.instance_type {
        Some(SingleOrVec::Single(ty)) => **ty == InstanceType::String,
        Some(SingleOrVec::Vec(types)) => {
            types.contains(&InstanceType::String)
                && types
                    .iter()
                    .all(|t| matches!(t, InstanceType::String | InstanceType::Null))
        }
        None => implied_type(obj) == Some(InstanceType::String),
    }
}

/// `n` as written in specs: `1` rather than `1.0` for a whole number.
fn json_number(n: f64) -> Option<serde_json::Number> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if n.fract() == 0.0 && n.abs() <= EXACT {
        Some((n as i64).into())
    } else {
        serde_json::Number::from_f64(n)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::spec::Filters;

    /// The model of a spec of version `openapi` with one operation, `GET /things/{id}` of tag
    /// `things`, taking `parameters` and returning a `Thing` of `properties`.
    fn model(openapi: &str, parameters: Value, properties: Value, schemas: Value) -> Value {
        let mut schemas = schemas;
        schemas["Thing"] = json!({ "type": "object", "properties": properties });
        let spec = json!({
            "openapi": openapi,
            "info": { "title": "T", "version": "1" },
            "paths": { "/things/{id}": { "get": {
                "operationId": "get_thing", "tags": ["things"], "parameters": parameters,
                "responses": { "200": { "description": "", "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/Thing" } } } } },
            } } },
            "components": { "schemas": schemas },
        });
        serde_json::to_value(api(&spec)).unwrap()
    }

    /// The model of `spec`, upgraded to OpenAPI 3.1 as specs read from files are.
    fn api(spec: &Value) -> crate::api::Api {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("openapi.json"), spec.to_string()).unwrap();
        let spec = crate::spec::read("openapi.json", dir.path()).unwrap();
        crate::spec::api(&spec, &Filters::default()).unwrap()
    }

    fn id_param() -> Value {
        json!({ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } })
    }

    fn fields(model: &Value) -> serde_json::Map<String, Value> {
        let fields = model["types"]["Thing"]["fields"].as_array().unwrap();
        (fields.iter())
            .map(|f| (f["name"].as_str().unwrap().to_owned(), f.clone()))
            .collect()
    }

    fn constraints_of(openapi: &str, property: Value) -> Value {
        let model = model(
            openapi,
            json!([id_param()]),
            json!({ "p": property }),
            json!({}),
        );
        fields(&model)["p"]["constraints"].clone()
    }

    #[test]
    fn exclusive_bounds_read_the_same_in_openapi_3_0_and_3_1() {
        let legacy = json!({ "type": "integer", "minimum": 1, "exclusiveMinimum": true,
            "maximum": 9, "exclusiveMaximum": false });
        let current = json!({ "type": "integer", "exclusiveMinimum": 1, "maximum": 9 });
        let expected = json!({ "exclusive_minimum": 1, "maximum": 9 });
        assert_eq!(constraints_of("3.0.3", legacy), expected);
        assert_eq!(constraints_of("3.1.0", current), expected);
        let legacy = json!({ "type": "number", "maximum": 0.5, "exclusiveMaximum": true });
        let current = json!({ "type": "number", "exclusiveMaximum": 0.5 });
        assert_eq!(
            constraints_of("3.0.3", legacy),
            json!({ "exclusive_maximum": 0.5 })
        );
        assert_eq!(
            constraints_of("3.1.0", current),
            json!({ "exclusive_maximum": 0.5 })
        );
    }

    #[test]
    fn numbers_keep_their_bounds_and_step() {
        for openapi in ["3.0.3", "3.1.0"] {
            let number = json!({ "type": "number", "minimum": -1.5, "maximum": 100,
                "multipleOf": 0.25 });
            assert_eq!(
                constraints_of(openapi, number),
                json!({ "minimum": -1.5, "maximum": 100, "multiple_of": 0.25 })
            );
        }
    }

    #[test]
    fn strings_keep_their_lengths_pattern_and_untyped_format() {
        for openapi in ["3.0.3", "3.1.0"] {
            let string = json!({ "type": "string", "minLength": 1, "maxLength": 9,
                "pattern": "^[a-z]+$", "format": "email" });
            assert_eq!(
                constraints_of(openapi, string),
                json!({ "min_length": 1, "max_length": 9, "pattern": "^[a-z]+$",
                    "format": "email" })
            );
            for typed in ["date-time", "date", "uuid", "uri", "decimal"] {
                let string = json!({ "type": "string", "format": typed });
                assert_eq!(constraints_of(openapi, string), Value::Null, "{typed}");
            }
            let int = json!({ "type": "integer", "format": "int32" });
            assert_eq!(constraints_of(openapi, int), Value::Null);
        }
        let nullable = json!({ "type": "string", "nullable": true, "format": "ipv4" });
        assert_eq!(
            constraints_of("3.0.3", nullable),
            json!({ "format": "ipv4" })
        );
        let nullable = json!({ "type": ["string", "null"], "format": "ipv4" });
        assert_eq!(
            constraints_of("3.1.0", nullable),
            json!({ "format": "ipv4" })
        );
    }

    #[test]
    fn arrays_keep_their_sizes_and_the_constraints_of_their_items() {
        for openapi in ["3.0.3", "3.1.0"] {
            let array = json!({ "type": "array", "minItems": 1, "maxItems": 3,
                "uniqueItems": true, "items": { "type": "integer", "minimum": 0 } });
            assert_eq!(
                constraints_of(openapi, array),
                json!({ "min_items": 1, "max_items": 3, "unique_items": true,
                    "items": { "minimum": 0 } })
            );
        }
    }

    #[test]
    fn references_to_aliases_carry_their_constraints() {
        let schemas = json!({
            "Email": { "type": "string", "format": "email", "maxLength": 254 },
            "Small": { "type": "integer", "minimum": 0, "maximum": 9 },
            "Digit": { "$ref": "#/components/schemas/Small", "description": "A digit." },
        });
        let email = json!({ "$ref": "#/components/schemas/Email" });
        let properties = json!({
            "email": email,
            "short_email": { "allOf": [email], "maxLength": 20 },
            "emails": { "type": "array", "items": email },
            "digit": { "$ref": "#/components/schemas/Digit" },
        });
        let parameters = json!([id_param(),
            { "name": "email", "in": "query", "schema": email },
            { "name": "digits", "in": "query",
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Digit" } } },
        ]);
        let model = model("3.1.0", parameters, properties, schemas);
        let fields = fields(&model);
        let email = json!({ "max_length": 254, "format": "email" });
        assert_eq!(fields["email"]["constraints"], email);
        assert_eq!(
            fields["short_email"]["constraints"],
            json!({ "max_length": 20, "format": "email" })
        );
        assert_eq!(fields["emails"]["constraints"], json!({ "items": email }));
        assert_eq!(
            fields["digit"]["constraints"],
            json!({ "minimum": 0, "maximum": 9 })
        );
        assert_eq!(model["types"]["Email"]["constraints"], email);
        let query = &model["resources"][0]["operations"][0]["query_params"];
        assert_eq!(query[0]["constraints"], email);
        assert_eq!(
            query[1]["constraints"],
            json!({ "items": { "minimum": 0, "maximum": 9 } })
        );
    }

    #[test]
    fn inlined_aliases_keep_their_constraints() {
        let schemas = json!({ "Small": { "type": "integer", "minimum": 0, "maximum": 9 } });
        let properties = json!({ "n": { "$ref": "#/components/schemas/Small" } });
        let spec = json!({
            "openapi": "3.1.0", "info": { "title": "T", "version": "1" },
            "paths": { "/things": { "get": { "operationId": "get_thing", "responses": {
                "200": { "description": "", "content": { "application/json": {
                    "schema": { "$ref": "#/components/schemas/Thing" } } } } } } } },
            "components": { "schemas": { "Small": schemas["Small"],
                "Thing": { "type": "object", "properties": properties } } },
        });
        let mut api = api(&spec);
        api.inline_aliases().unwrap();
        let model = serde_json::to_value(api).unwrap();
        let n = &model["types"]["Thing"]["fields"][0];
        assert_eq!(n["type"]["id"], "Int64");
        assert_eq!(n["constraints"], json!({ "minimum": 0, "maximum": 9 }));
    }

    #[test]
    fn parameters_keep_their_constraints_default_example_and_deprecation() {
        for openapi in ["3.0.3", "3.1.0"] {
            let parameters = json!([
                { "name": "id", "in": "path", "required": true, "style": "label",
                    "description": "The <b>id</b>.",
                    "schema": { "type": "string", "minLength": 2 } },
                { "name": "limit", "in": "query", "deprecated": true, "example": 10,
                    "schema": { "type": "integer", "minimum": 1, "default": 20 } },
                { "name": "X-Trace", "in": "header",
                    "schema": { "type": "string", "pattern": "^[0-9]+$" } },
            ]);
            let model = model(openapi, parameters, json!({}), json!({}));
            let op = &model["resources"][0]["operations"][0];
            let path = &op["typed_path_params"][0];
            assert_eq!(op["path_styles"]["id"]["style"], "label");
            assert_eq!(path["description"], "The **id**.");
            assert_eq!(path["constraints"], json!({ "min_length": 2 }));
            let limit = &op["query_params"][0];
            assert_eq!(limit["deprecated"], true);
            assert_eq!(limit["default"], 20);
            assert_eq!(limit["example"], 10);
            assert_eq!(limit["constraints"], json!({ "minimum": 1 }));
            let header = &op["header_params"][0];
            assert_eq!(header["constraints"], json!({ "pattern": "^[0-9]+$" }));
            assert!(header.get("deprecated").is_none() && header.get("default").is_none());
        }
    }

    #[test]
    fn fields_keep_their_first_example() {
        let properties = json!({
            "listed": { "type": "string", "examples": ["a", "b"] },
            "single": { "type": "string", "example": "c", "examples": ["d"] },
        });
        let model = model("3.1.0", json!([id_param()]), properties, json!({}));
        let fields = fields(&model);
        assert_eq!(fields["listed"]["example"], "a");
        assert_eq!(fields["single"]["example"], "c");
    }

    #[test]
    fn unconstrained_values_serialize_no_constraints() {
        let model = model(
            "3.1.0",
            json!([id_param()]),
            json!({ "p": { "type": "string" } }),
            json!({}),
        );
        assert!(fields(&model)["p"].get("constraints").is_none());
        let op = &model["resources"][0]["operations"][0];
        let path = op["typed_path_params"][0].as_object().unwrap();
        assert_eq!(path.keys().collect::<Vec<_>>(), ["name", "type"]);
    }
}
