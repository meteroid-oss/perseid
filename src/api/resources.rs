use std::collections::{BTreeMap, BTreeSet};

use aide::openapi::{self, ReferenceOr};
use anyhow::{Context as _, bail, ensure};
use heck::ToSnakeCase as _;
use indexmap::IndexMap;
use schemars::schema::{InstanceType, Schema};
use serde::{Deserialize, Serialize};

use crate::spec::IncludeMode;

use super::{
    get_schema_name,
    types::{Field, FieldType, resolve_schema_ref_in_field_type_public, serialize_field_type},
};

/// The API operations of the API client we generate.
///
/// Intermediate representation of `paths` from the spec.
pub(crate) type Resources = BTreeMap<String, Resource>;

pub(crate) fn from_openapi(
    paths: openapi::Paths,
    component_schemas: &IndexMap<String, openapi::SchemaObject>,
    include_mode: IncludeMode,
    excluded_operations: &BTreeSet<String>,
    specified_operations: &BTreeSet<String>,
) -> anyhow::Result<Resources> {
    let mut resources = BTreeMap::new();

    for (path, pi) in paths {
        let path_item = pi
            .into_item()
            .context("$ref paths are currently not supported")?;

        if !path_item.parameters.is_empty() {
            tracing::info!("parameters at the path item level are not currently supported");
            continue;
        }

        for (method, op) in path_item {
            if let Some((res_path, op)) = Operation::from_openapi(
                &path,
                method,
                op,
                component_schemas,
                include_mode,
                excluded_operations,
                specified_operations,
            ) {
                let resource = get_or_insert_resource(&mut resources, res_path);
                resource.operations.push(op);
            }
        }
    }

    Ok(resources)
}

pub(crate) fn referenced_components(resources: &Resources) -> impl Iterator<Item = &str> {
    resources.values().flat_map(Resource::referenced_components)
}

/// Resolve SchemaRef inner types in resources (operation query params).
/// This allows to_java() and similar methods to properly convert string aliases to their base types.
pub(crate) fn resolve_schema_refs_in_resources(
    resources: &mut Resources,
    string_alias_names: &BTreeSet<String>,
) {
    for resource in resources.values_mut() {
        resolve_schema_refs_in_resource(resource, string_alias_names);
    }
}

fn resolve_schema_refs_in_resource(resource: &mut Resource, string_alias_names: &BTreeSet<String>) {
    // Resolve in subresources
    for sub in resource.subresources.values_mut() {
        resolve_schema_refs_in_resource(sub, string_alias_names);
    }

    // Resolve in operations
    for op in &mut resource.operations {
        for field in &mut op.multipart_fields {
            resolve_schema_ref_in_field_type_public(&mut field.field.r#type, string_alias_names);
        }
        for param in &mut op.query_params {
            resolve_schema_ref_in_field_type_public(&mut param.r#type, string_alias_names);
        }
    }
}

fn get_or_insert_resource(resources: &mut Resources, path: Vec<String>) -> &mut Resource {
    let mut path_iter = path.into_iter();
    let mut name = path_iter.next().expect("path must be non-empty");
    let mut r = resources
        .entry(name.clone())
        .or_insert_with(|| Resource::new(name.clone()));

    for sub_name in path_iter {
        name.push('.');
        name.push_str(&sub_name);

        r = r
            .subresources
            .entry(sub_name)
            .or_insert_with(|| Resource::new(name.clone()));
    }

    r
}

/// A named group of [`Operation`]s.
#[derive(Deserialize, Serialize)]
pub(crate) struct Resource {
    pub name: String,
    pub operations: Vec<Operation>,
    pub subresources: Resources,
}

impl Resource {
    pub(crate) fn inline_aliases(
        &mut self,
        aliases: &BTreeMap<String, FieldType>,
    ) -> anyhow::Result<()> {
        for operation in &mut self.operations {
            let body_schemas = [
                &operation.request_body_schema_name,
                &operation.response_body_schema_name,
            ];
            for name in body_schemas.into_iter().flatten() {
                ensure!(
                    !aliases.contains_key(name),
                    "alias schema `{name}` cannot be used as a request or response body"
                );
            }
            for param in &mut operation.query_params {
                param.r#type.inline_aliases(aliases);
            }
            for field in &mut operation.multipart_fields {
                field.field.r#type.inline_aliases(aliases);
            }
        }
        for resource in self.subresources.values_mut() {
            resource.inline_aliases(aliases)?;
        }
        Ok(())
    }

    fn new(name: String) -> Self {
        Self {
            name,
            operations: Vec::new(),
            subresources: BTreeMap::new(),
        }
    }

    pub(crate) fn requires_streaming_runtime(&self) -> bool {
        self.operations.iter().any(|op| {
            op.response_is_event_stream
                || matches!(
                    op.request_body_kind,
                    RequestBodyKind::Binary | RequestBodyKind::Multipart
                )
        }) || self
            .subresources
            .values()
            .any(Self::requires_streaming_runtime)
    }

    pub(crate) fn referenced_components(&self) -> BTreeSet<&str> {
        let mut res = BTreeSet::new();

        for resource in self.subresources.values() {
            res.extend(resource.referenced_components());
        }

        for operation in &self.operations {
            for field in &operation.multipart_fields {
                if let Some(name) = field.field.r#type.referenced_schema() {
                    res.insert(name);
                }
            }
            for param in &operation.query_params {
                if let Some(name) = param.r#type.referenced_schema() {
                    res.insert(name);
                }
            }
            if let Some(name) = &operation.request_body_schema_name {
                res.insert(name);
            }
            if let Some(name) = &operation.response_body_schema_name {
                res.insert(name);
            }
            res.extend(
                operation
                    .error_response_schema_names
                    .iter()
                    .map(String::as_str),
            );
        }

        res
    }
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RequestBodyKind {
    #[default]
    None,
    Json,
    Form,
    Binary,
    Multipart,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct MultipartField {
    #[serde(flatten)]
    pub(crate) field: Field,
    is_file: bool,
}

fn resolve_multipart_schema(
    mut schema: Schema,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> anyhow::Result<schemars::schema::SchemaObject> {
    let mut visited = BTreeSet::new();
    loop {
        let Schema::Object(object) = schema else {
            bail!("multipart fields must have typed schemas");
        };
        if let Some(reference) = &object.reference {
            if !visited.insert(reference.clone()) {
                bail!("cyclic multipart schema reference");
            }
            let name =
                get_schema_name(Some(reference)).context("invalid multipart schema reference")?;
            schema = schemas
                .get(&name)
                .context("missing multipart schema")?
                .json_schema
                .clone();
        } else {
            return Ok(object);
        }
    }
}

fn multipart_fields(
    schema: Schema,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> anyhow::Result<Vec<MultipartField>> {
    let object = resolve_multipart_schema(schema, schemas)?
        .object
        .context("multipart body must declare object properties")?;
    object
        .properties
        .into_iter()
        .map(|(name, mut schema)| {
            let mut resolved = resolve_multipart_schema(schema.clone(), schemas)?;
            if resolved.array.is_some() {
                bail!("multipart array fields require an explicit encoding implementation");
            }
            let is_file = resolved.format.as_deref() == Some("binary");
            if is_file {
                resolved.format = None;
                schema = Schema::Object(resolved);
            }
            let field = Field::from_schema(name.clone(), schema, object.required.contains(&name))?;
            Ok(MultipartField { field, is_file })
        })
        .collect()
}

/// A named HTTP endpoint.
#[derive(Deserialize, Serialize)]
pub(crate) struct Operation {
    /// The operation ID from the spec.
    pub(crate) id: String,
    /// The name to use for the operation in code.
    pub(crate) name: String,
    /// Description of the operation to use for documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Whether this operation is marked as deprecated.
    deprecated: bool,
    /// The HTTP method.
    ///
    /// Encoded as "get", "post" or such because that's what aide's PathItem iterator gives us.
    method: String,
    /// The operation's endpoint path.
    path: String,
    /// Path parameters.
    ///
    /// Only required string-typed parameters are currently supported.
    path_params: Vec<String>,
    /// Header parameters.
    ///
    /// Only string-typed parameters are currently supported.
    header_params: Vec<HeaderParam>,
    /// Query parameters.
    pub(crate) query_params: Vec<QueryParam>,
    /// Name of the request body type, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) request_body_schema_name: Option<String>,
    /// Some request bodies are required, but all the fields are optional (i.e. the CLI can omit
    /// this from the argument list).
    /// Only useful when `request_body_schema_name` is `Some`.
    request_body_all_optional: bool,
    #[serde(default)]
    request_body_optional: bool,
    /// True if the request body is application/x-www-form-urlencoded instead of JSON.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    request_body_is_form: bool,
    #[serde(default)]
    request_body_kind: RequestBodyKind,
    #[serde(default)]
    pub(crate) multipart_fields: Vec<MultipartField>,
    /// Name of the response body type, if any (only for JSON responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    response_body_schema_name: Option<String>,
    /// True if the response is binary (e.g., application/pdf, application/octet-stream).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_is_binary: bool,
    /// True if the response is text (e.g., text/plain, text/html).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_is_text: bool,
    #[serde(default)]
    response_is_event_stream: bool,
    /// Schemas of the JSON bodies this operation returns on 4xx/5xx responses.
    ///
    /// Not rendered per operation: collected so that `referenced_components` pulls the error
    /// schemas (e.g. `RestErrorResponse`) into the generated models alongside everything else.
    #[serde(skip)]
    error_response_schema_names: BTreeSet<String>,
}

impl Operation {
    #[tracing::instrument(
        name = "operation_from_openapi",
        skip_all,
        fields(path = path, method = method, op_id),
    )]
    fn from_openapi(
        path: &str,
        method: &str,
        op: openapi::Operation,
        component_schemas: &IndexMap<String, aide::openapi::SchemaObject>,
        include_mode: IncludeMode,
        excluded_operations: &BTreeSet<String>,
        specified_operations: &BTreeSet<String>,
    ) -> Option<(Vec<String>, Self)> {
        let Some(op_id) = op.operation_id else {
            // ignore operations without an operationId
            return None;
        };
        tracing::Span::current().record("op_id", &op_id);

        // verbose, but very easy to understand
        let x_internal = op
            .extensions
            .get("x-internal")
            .is_some_and(|val| val == true);
        let include_operation = match include_mode {
            IncludeMode::OnlyPublic => !x_internal,
            IncludeMode::PublicAndInternal => true,
            IncludeMode::OnlyInternal => x_internal,
            IncludeMode::OnlySpecified => specified_operations.contains(&op_id),
        };
        if !include_operation || excluded_operations.contains(&op_id) {
            return None;
        }

        // Use tags as resource name, operation ID as operation name
        let tag = op.tags.first().cloned().unwrap_or_else(|| {
            // Derive resource from path: /api/v1/customers -> customers
            path.split('/')
                .find(|s| !s.is_empty() && *s != "api" && *s != "v1")
                .unwrap_or("default")
                .to_owned()
        });
        // Convert tag to resource name (e.g., "checkout-sessions" -> "checkout_sessions")
        let resource_name = tag.to_snake_case();
        let res_path = vec![resource_name];
        let op_name = op_id.clone();

        let mut path_params = Vec::new();
        let mut query_params = Vec::new();
        let mut header_params = Vec::new();

        for param in op.parameters {
            match param {
                ReferenceOr::Reference { .. } => {
                    tracing::error!("$ref parameters are not currently supported");
                    return None;
                }
                ReferenceOr::Item(openapi::Parameter::Path {
                    parameter_data,
                    style: openapi::PathStyle::Simple,
                }) => {
                    assert!(parameter_data.required, "no optional path params");
                    if let Err(e) = enforce_string_parameter(&parameter_data) {
                        tracing::error!("unsupported path parameter: {e}");
                        return None;
                    }

                    path_params.push(parameter_data.name);
                }
                ReferenceOr::Item(openapi::Parameter::Header {
                    parameter_data,
                    style: openapi::HeaderStyle::Simple,
                }) => {
                    if let Err(e) = enforce_string_parameter(&parameter_data) {
                        tracing::error!("unsupported header parameter: {e}");
                        return None;
                    }

                    header_params.push(HeaderParam {
                        name: parameter_data.name,
                        required: parameter_data.required,
                    });
                }
                ReferenceOr::Item(openapi::Parameter::Query {
                    parameter_data,
                    allow_reserved: false,
                    style: openapi::QueryStyle::Form,
                    allow_empty_value: None,
                }) => {
                    let name = parameter_data.name;
                    let _guard = tracing::info_span!("field_type_from_openapi", name).entered();
                    let r#type = match FieldType::from_openapi(parameter_data.format) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::error!("unsupported query parameter type: {e}");
                            return None;
                        }
                    };

                    // For `style: form`, OpenAPI's default is `explode: true` — arrays are
                    // serialized as repeated parameters (e.g. `?tags=a&tags=b`). When
                    // `explode: false`, arrays are comma-joined (e.g. `?tags=a,b`).
                    let explode = parameter_data.explode.unwrap_or(true);

                    query_params.push(QueryParam {
                        name,
                        description: parameter_data.description,
                        required: parameter_data.required,
                        r#type,
                        explode,
                    });
                }
                ReferenceOr::Item(parameter) => {
                    tracing::error!(
                        ?parameter,
                        "this kind of parameter is not currently supported"
                    );
                    return None;
                }
            }
        }

        let request_body_all_optional = op
            .request_body
            .as_ref()
            .map(|r| {
                match r {
                    ReferenceOr::Reference { .. } => {
                        unimplemented!("reference")
                    }
                    ReferenceOr::Item(body) => {
                        if let Some(mt) = body
                            .content
                            .get("application/json")
                            .or_else(|| body.content.get("application/x-www-form-urlencoded"))
                        {
                            match mt.schema.as_ref().map(|so| &so.json_schema) {
                                Some(Schema::Object(schemars::schema::SchemaObject {
                                    object: Some(ov),
                                    ..
                                })) => {
                                    return ov.required.is_empty();
                                }
                                Some(Schema::Object(schemars::schema::SchemaObject {
                                    reference: Some(s),
                                    ..
                                })) => {
                                    if let Some(Schema::Object(schemars::schema::SchemaObject {
                                        object: Some(ov),
                                        ..
                                    })) = component_schemas
                                        .get(
                                            &get_schema_name(Some(s)).expect("schema should exist"),
                                        )
                                        .map(|so| &so.json_schema)
                                    {
                                        return ov.required.is_empty();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                false
            })
            .unwrap_or_default();

        let request_body_optional = op
            .request_body
            .as_ref()
            .and_then(|b| b.as_item())
            .is_some_and(|b| !b.required);
        let mut request_body_is_form = false;
        let mut request_body_kind = RequestBodyKind::None;
        let mut multipart_fields_out = Vec::new();
        let request_body_schema_name = op.request_body.and_then(|b| match b {
            ReferenceOr::Item(mut req_body) => {
                assert!(req_body.extensions.is_empty());
                assert_eq!(req_body.content.len(), 1);
                if req_body
                    .content
                    .swap_remove("application/octet-stream")
                    .is_some()
                {
                    request_body_kind = RequestBodyKind::Binary;
                    return None;
                }
                if let Some(body) = req_body.content.swap_remove("multipart/form-data") {
                    request_body_kind = RequestBodyKind::Multipart;
                    if !body.encoding.is_empty() {
                        tracing::error!("custom multipart encoding is not supported");
                        return None;
                    }
                    match body
                        .schema
                        .context("missing multipart schema")
                        .and_then(|s| multipart_fields(s.json_schema, component_schemas))
                    {
                        Ok(fields) => multipart_fields_out = fields,
                        Err(error) => tracing::error!(%error, "unsupported multipart body"),
                    }
                    return None;
                }
                let is_form = req_body
                    .content
                    .contains_key("application/x-www-form-urlencoded");
                let body = req_body
                    .content
                    .swap_remove("application/json")
                    .or_else(|| {
                        req_body
                            .content
                            .swap_remove("application/x-www-form-urlencoded")
                    })
                    .expect("should have JSON or form-urlencoded body");
                request_body_is_form = is_form;
                request_body_kind = if is_form {
                    RequestBodyKind::Form
                } else {
                    RequestBodyKind::Json
                };
                assert!(body.extensions.is_empty());
                match body.schema.expect("no body schema?!").json_schema {
                    Schema::Bool(_) => {
                        tracing::error!("unexpected bool schema");
                        None
                    }
                    Schema::Object(obj) => {
                        if !obj.is_ref() {
                            tracing::error!(?obj, "unexpected non-$ref json body schema");
                        }
                        get_schema_name(obj.reference.as_deref())
                    }
                }
            }
            ReferenceOr::Reference { .. } => {
                tracing::error!("$ref request bodies are not currently supported");
                None
            }
        });

        let (response_body_schema_name, response_kind, error_response_schema_names) = op
            .responses
            .map(|r| {
                assert_eq!(r.default, None);
                assert!(r.extensions.is_empty());
                let error_response_schema_names: BTreeSet<String> = r
                    .responses
                    .iter()
                    .filter(|(st, _)| matches!(st, openapi::StatusCode::Code(400..)))
                    .filter_map(|(_, resp)| response_body_info(resp.clone()).0)
                    .collect();
                let mut success_responses = r.responses.into_iter().filter(|(st, _)| {
                    match st {
                        openapi::StatusCode::Code(c) => match c {
                            0..100 => tracing::error!("invalid status code < 100"),
                            100..200 => tracing::error!("what is this? status code {c}..."),
                            200..300 => return true,
                            300..400 => tracing::error!("what is this? status code {c}..."),
                            400.. => {}
                        },
                        openapi::StatusCode::Range(_) => {
                            tracing::error!("unsupported status code range");
                        }
                    }

                    false
                });

                let (_, resp) = success_responses
                    .next()
                    .expect("every operation must have one success response");
                let (schema_name, kind) = response_body_info(resp);
                for (_, resp) in success_responses {
                    let (other_name, other_kind) = response_body_info(resp);
                    assert_eq!(schema_name, other_name);
                    assert_eq!(kind, other_kind);
                }

                (schema_name, kind, error_response_schema_names)
            })
            .unwrap_or((None, ResponseKind::None, BTreeSet::new()));

        let op = Operation {
            id: op_id,
            name: op_name,
            description: op.description,
            deprecated: op.deprecated,
            method: method.to_owned(),
            path: path.to_owned(),
            path_params,
            header_params,
            query_params,
            request_body_schema_name,
            request_body_all_optional,
            request_body_optional,
            request_body_is_form,
            request_body_kind,
            multipart_fields: multipart_fields_out,
            response_body_schema_name,
            response_is_binary: response_kind == ResponseKind::Binary,
            response_is_text: response_kind == ResponseKind::Text,
            response_is_event_stream: response_kind == ResponseKind::EventStream,
            error_response_schema_names,
        };
        Some((res_path, op))
    }

    pub(crate) fn has_query_or_header_params(&self) -> bool {
        !self.header_params.is_empty() || !self.query_params.is_empty()
    }
}

fn enforce_string_parameter(parameter_data: &openapi::ParameterData) -> anyhow::Result<()> {
    let openapi::ParameterSchemaOrContent::Schema(s) = &parameter_data.format else {
        bail!("found unexpected 'content' data format");
    };
    let Schema::Object(obj) = &s.json_schema else {
        bail!("found unexpected `true` schema");
    };

    // Check for direct string type
    if matches!(obj.instance_type, Some(schemars::schema::SingleOrVec::Single(ref t)) if matches!(**t, InstanceType::String | InstanceType::Integer | InstanceType::Number | InstanceType::Boolean))
    {
        return Ok(());
    }

    // Handle OpenAPI 3.1 type arrays like ["string", "null"]
    if let Some(schemars::schema::SingleOrVec::Vec(types)) = &obj.instance_type {
        let has_string = types.contains(&InstanceType::String);
        let all_string_or_null = types
            .iter()
            .all(|t| *t == InstanceType::String || *t == InstanceType::Null);
        if has_string && all_string_or_null {
            return Ok(());
        }
    }

    // Handle oneOf patterns like: oneOf: [{type: null}, {$ref: ...}] or oneOf: [{type: null}, {type: string}]
    if let Some(ref subschemas) = obj.subschemas
        && let Some(ref one_of) = subschemas.one_of
        && one_of.len() == 2
    {
        for schema in one_of {
            if let Schema::Object(inner_obj) = schema {
                // Check if this is a null type - skip it
                let is_null = match &inner_obj.instance_type {
                    Some(schemars::schema::SingleOrVec::Single(t)) => **t == InstanceType::Null,
                    _ => false,
                };
                if is_null {
                    continue;
                }

                // Check if this is a string type
                if inner_obj.instance_type == Some(InstanceType::String.into()) {
                    return Ok(());
                }

                // Check if this is a $ref (path params with refs are typically string IDs)
                if inner_obj.reference.is_some() {
                    return Ok(());
                }
            }
        }
    }

    // If instance_type is None but there's a $ref, accept it (typically string IDs)
    if obj.instance_type.is_none() && obj.reference.is_some() {
        return Ok(());
    }

    bail!("unsupported path parameter type `{:?}`", obj.instance_type);
}

/// Response body type for code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ResponseKind {
    #[default]
    None,
    Json,
    Binary,
    Text,
    EventStream,
}

/// Returns (schema_name, response_kind) for a response.
fn response_body_info(resp: ReferenceOr<openapi::Response>) -> (Option<String>, ResponseKind) {
    match resp {
        ReferenceOr::Item(mut resp_body) => {
            assert!(resp_body.extensions.is_empty());
            if resp_body.content.is_empty() {
                return (None, ResponseKind::None);
            }

            if resp_body.content.contains_key("text/event-stream") {
                return (None, ResponseKind::EventStream);
            }

            // Check for binary response types
            // Document downloads are returned as raw bytes; e-invoice XML stays byte-exact
            // (it can be signed or hashed), so it is not decoded as text.
            let binary_types = [
                "application/pdf",
                "application/xml",
                "application/octet-stream",
                "image/png",
                "image/jpeg",
                "image/gif",
                "application/zip",
                "application/gzip",
            ];
            for binary_type in binary_types {
                if resp_body.content.contains_key(binary_type) {
                    tracing::info!(content_type = binary_type, "detected binary response");
                    return (None, ResponseKind::Binary);
                }
            }

            // Check for text response types
            let text_types = ["text/plain", "text/html", "text/csv", "text/x-sh"];
            for text_type in text_types {
                if resp_body.content.contains_key(text_type) {
                    tracing::info!(content_type = text_type, "detected text response");
                    return (None, ResponseKind::Text);
                }
            }

            // Handle JSON responses
            let Some(json_body) = resp_body.content.swap_remove("application/json") else {
                tracing::error!(
                    content_types = ?resp_body.content.keys().collect::<Vec<_>>(),
                    "unsupported response body content type"
                );
                return (None, ResponseKind::None);
            };
            assert!(json_body.extensions.is_empty());
            let schema_name = match json_body.schema.expect("no json body schema?!").json_schema {
                Schema::Bool(_) => {
                    tracing::error!("unexpected bool schema");
                    None
                }
                Schema::Object(obj) => {
                    if !obj.is_ref() {
                        tracing::error!(?obj, "unexpected non-$ref json body schema");
                    }
                    get_schema_name(obj.reference.as_deref())
                }
            };
            (schema_name, ResponseKind::Json)
        }
        ReferenceOr::Reference { .. } => {
            tracing::error!("$ref response bodies are not currently supported");
            (None, ResponseKind::None)
        }
    }
}

#[derive(Deserialize, Serialize)]
struct HeaderParam {
    name: String,
    required: bool,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct QueryParam {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    required: bool,
    #[serde(serialize_with = "serialize_field_type")]
    pub(crate) r#type: FieldType,
    /// Whether array values are exploded into repeated query parameters
    /// (`?tags=a&tags=b`) or comma-joined (`?tags=a,b`). Defaults to `true`
    /// per OpenAPI's `style: form` default.
    #[serde(default = "default_explode")]
    pub(crate) explode: bool,
}

fn default_explode() -> bool {
    true
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn multipart_resolves_referenced_binary_fields() {
        let schemas =
            serde_json::from_value(json!({"File": {"type":"string", "format":"binary"}})).unwrap();
        let schema = serde_json::from_value(json!({"type":"object", "required":["file"], "properties":{"file":{"$ref":"#/components/schemas/File"}}})).unwrap();
        let fields = multipart_fields(schema, &schemas).unwrap();
        assert!(fields[0].is_file);
        assert_eq!(fields[0].field.name, "file");
    }

    #[test]
    fn multipart_rejects_array_encodings_instead_of_emitting_json_files() {
        let schema = serde_json::from_value(json!({"type":"object", "properties":{"files":{"type":"array","items":{"type":"string","format":"binary"}}}})).unwrap();
        assert!(
            multipart_fields(schema, &IndexMap::new())
                .err()
                .unwrap()
                .to_string()
                .contains("array fields")
        );
    }

    #[test]
    fn multipart_cyclic_references_fail_without_recursing() {
        let schemas =
            serde_json::from_value(json!({"Cycle": {"$ref":"#/components/schemas/Cycle"}}))
                .unwrap();
        let schema = serde_json::from_value(json!({"$ref":"#/components/schemas/Cycle"})).unwrap();
        assert!(
            multipart_fields(schema, &schemas)
                .err()
                .unwrap()
                .to_string()
                .contains("cyclic")
        );
    }
}
