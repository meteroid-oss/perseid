use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use aide::openapi::{self, ReferenceOr};
use anyhow::{Context as _, bail, ensure};
use heck::{ToSnakeCase as _, ToUpperCamelCase as _};
use indexmap::IndexMap;
use itertools::Itertools as _;
use schemars::schema::{InstanceType, Schema, SchemaObject, SingleOrVec};
use serde::{Deserialize, Serialize};

use crate::spec::IncludeMode;

use super::{
    get_schema_name,
    pagination::{Candidate, Pagination},
    security::{Requirement, Security},
    types::{
        Field, FieldType, TypeData, Types, resolve_schema_ref_in_field_type_public,
        serialize_field_type, serialize_optional_field_type,
    },
};
use crate::config;

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
) -> (Resources, Vec<String>) {
    let mut resources = BTreeMap::new();
    let mut errors = Vec::new();

    for (path, pi) in paths {
        let Some(path_item) = pi.into_item() else {
            errors.push(format!(
                "path `{path}`: `$ref` path items are not supported"
            ));
            continue;
        };
        for (method, op) in path_item {
            let op_id = op.operation_id.clone().unwrap_or_default();
            let _span = tracing::warn_span!("operation", name = %op_id).entered();
            match Operation::from_openapi(
                &path,
                method,
                op,
                component_schemas,
                include_mode,
                excluded_operations,
                specified_operations,
            ) {
                Ok(Some((res_path, op))) => {
                    get_or_insert_resource(&mut resources, res_path)
                        .operations
                        .push(op);
                }
                Ok(None) => {}
                Err(e) => errors.push(format!(
                    "operation `{op_id}` ({} {path}): {e:#}",
                    method.to_uppercase()
                )),
            }
        }
    }
    for resource in resources.values_mut() {
        if let Err(e) = resource.disambiguate_operation_names() {
            errors.push(format!("{e:#}"));
        }
    }

    (resources, errors)
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

/// Query parameters whose values are objects, which form-style query strings cannot carry.
pub(crate) fn object_query_params(resources: &Resources, types: &Types) -> Vec<String> {
    fn is_object(ty: &FieldType, types: &Types, depth: usize) -> bool {
        match ty {
            FieldType::JsonObject | FieldType::Map { .. } => true,
            FieldType::List { inner } | FieldType::Set { inner } => is_object(inner, types, depth),
            FieldType::SchemaRef { name, .. } => match types.get(name).map(|t| &t.data) {
                Some(TypeData::Alias { target }) if depth < 16 => {
                    is_object(target, types, depth + 1)
                }
                Some(TypeData::Struct { .. } | TypeData::StructEnum { .. }) => true,
                _ => false,
            },
            _ => false,
        }
    }
    let mut errors = Vec::new();
    let mut stack: Vec<&Resource> = resources.values().collect();
    while let Some(resource) = stack.pop() {
        stack.extend(resource.subresources.values());
        for op in &resource.operations {
            for param in op
                .query_params
                .iter()
                .filter(|p| is_object(&p.r#type, types, 0))
            {
                errors.push(format!(
                    "operation `{}` ({} {}): query parameter `{}`: object values are not supported",
                    op.id,
                    op.method.to_uppercase(),
                    op.path,
                    param.name
                ));
            }
        }
    }
    errors
}

/// A `pet` resource and a `Pet` schema would both be `Pet` in most SDKs: the resource becomes
/// `pet_api`.
pub(crate) fn rename_resources_named_like_types(resources: &mut Resources, types: &Types) {
    let type_names: BTreeSet<String> = types.keys().map(|n| n.to_upper_camel_case()).collect();
    let clashing: Vec<String> = resources
        .keys()
        .filter(|name| type_names.contains(&name.to_upper_camel_case()))
        .cloned()
        .collect();
    for name in clashing {
        let mut resource = resources.remove(&name).expect("listed above");
        let renamed = format!("{name}_api");
        resource.name.clone_from(&renamed);
        resources.insert(renamed, resource);
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
#[derive(Clone, Deserialize, Serialize)]
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
            operation
                .inline_body_aliases(aliases)
                .with_context(|| format!("operation `{}`", operation.id))?;
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

    pub(crate) fn resolve_extensions(
        &mut self,
        security: &Security,
        rules: &[config::Pagination],
        types: &Types,
    ) -> anyhow::Result<()> {
        for op in &mut self.operations {
            op.security = security.override_for(&op.id);
            op.resolve_pagination(rules, types)
                .with_context(|| format!("pagination of `{}`", op.id))?;
        }
        for resource in self.subresources.values_mut() {
            resource.resolve_extensions(security, rules, types)?;
        }
        Ok(())
    }

    /// Falls back to the full operation id for operations whose short names collide.
    fn disambiguate_operation_names(&mut self) -> anyhow::Result<()> {
        let key = |op: &Operation| op.name.to_snake_case();
        let counts = self.operations.iter().map(key).counts();
        for op in &mut self.operations {
            if counts[&key(op)] > 1 {
                op.name = op.id.clone();
            }
        }
        let mut seen = BTreeMap::new();
        for op in &self.operations {
            if let Some(other) = seen.insert(key(op), &op.id) {
                bail!(
                    "operations `{other}` and `{}` both become `{}` in resource `{}`",
                    op.id,
                    key(op),
                    self.name
                );
            }
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

    /// Ids of the operations with multipart or binary uploads, or event stream responses.
    pub(crate) fn streaming_operations(&self) -> Vec<&str> {
        let own = self.operations.iter().filter(|op| {
            op.response_is_event_stream
                || matches!(
                    op.request_body_kind,
                    RequestBodyKind::Binary | RequestBodyKind::Multipart
                )
        });
        own.map(|op| op.id.as_str())
            .chain(
                self.subresources
                    .values()
                    .flat_map(Self::streaming_operations),
            )
            .collect()
    }

    /// Schemas sent as the body of a PATCH operation, where `null` and absent differ.
    pub(crate) fn patch_bodies(&self) -> BTreeSet<&str> {
        let own = self.operations.iter().filter(|op| op.method == "patch");
        own.filter_map(|op| op.request_body_schema_name.as_deref())
            .chain(self.subresources.values().flat_map(Self::patch_bodies))
            .collect()
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
            if let Some(pagination) = &operation.pagination {
                res.insert(&pagination.item_schema);
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RequestBodyKind {
    #[default]
    None,
    Json,
    Form,
    Binary,
    Multipart,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
#[derive(Clone, Deserialize, Serialize)]
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
    /// True if the request body is a JSON array of `request_body_schema_name`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    request_body_is_list: bool,
    /// Type of a JSON request body that is not a named schema, e.g. a list of strings.
    #[serde(
        default,
        serialize_with = "serialize_optional_field_type",
        skip_serializing_if = "Option::is_none"
    )]
    request_body_json_type: Option<FieldType>,
    /// Name of the response body type, if any (only for JSON responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    response_body_schema_name: Option<String>,
    /// True if the response is a JSON array of `response_body_schema_name`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_body_is_list: bool,
    /// Type of a JSON response that is not a named schema or a list of one, e.g. a map of
    /// integers. It only holds values whose SDK type is their JSON value.
    #[serde(
        default,
        serialize_with = "serialize_optional_field_type",
        skip_serializing_if = "Option::is_none"
    )]
    response_body_json_type: Option<FieldType>,
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
    /// Security requirement, when it differs from the API-wide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    security: Option<Requirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pagination: Option<Pagination>,
    #[serde(skip)]
    x_pagination: Option<serde_json::Value>,
}

impl Operation {
    fn from_openapi(
        path: &str,
        method: &str,
        op: openapi::Operation,
        component_schemas: &IndexMap<String, aide::openapi::SchemaObject>,
        include_mode: IncludeMode,
        excluded_operations: &BTreeSet<String>,
        specified_operations: &BTreeSet<String>,
    ) -> anyhow::Result<Option<(Vec<String>, Self)>> {
        let op_id = op
            .operation_id
            .context("missing operationId (derived ids are added while reading the spec)")?;

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
            return Ok(None);
        }

        let tag = match op.tags.first() {
            Some(tag) => tag.clone(),
            None => resource_from_path(path),
        };
        let res_path = vec![tag.to_snake_case()];
        // `repos/get-content` style ids already carry their resource.
        let op_name = op_id.rsplit('/').next().unwrap_or(&op_id).to_owned();

        let mut path_params = Vec::new();
        let mut query_params = Vec::new();
        let mut header_params = Vec::new();

        for param in op.parameters {
            let ReferenceOr::Item(param) = param else {
                bail!("unresolved `$ref` parameter");
            };
            let name = param.parameter_data_ref().name.clone();
            match param {
                openapi::Parameter::Path {
                    parameter_data,
                    style: openapi::PathStyle::Simple,
                } => {
                    enforce_string_parameter(&parameter_data)
                        .with_context(|| format!("path parameter `{name}`"))?;
                    path_params.push(parameter_data.name);
                }
                openapi::Parameter::Header {
                    parameter_data,
                    style: openapi::HeaderStyle::Simple,
                } => {
                    enforce_string_parameter(&parameter_data)
                        .with_context(|| format!("header parameter `{name}`"))?;
                    header_params.push(HeaderParam {
                        name: parameter_data.name,
                        required: parameter_data.required,
                    });
                }
                openapi::Parameter::Query {
                    parameter_data,
                    style: openapi::QueryStyle::Form,
                    ..
                } => {
                    let r#type = FieldType::from_openapi(parameter_data.format)
                        .with_context(|| format!("query parameter `{name}`"))?;
                    // `style: form` explodes arrays (`?tags=a&tags=b`) unless `explode: false`.
                    let explode = parameter_data.explode.unwrap_or(true);

                    query_params.push(QueryParam {
                        name,
                        description: parameter_data.description,
                        required: parameter_data.required,
                        r#type,
                        explode,
                    });
                }
                openapi::Parameter::Query { style, .. } => {
                    let style = serde_json::to_value(style).unwrap_or_default();
                    bail!("query parameter `{name}`: style {style} is not supported")
                }
                openapi::Parameter::Path { style, .. } => {
                    let style = serde_json::to_value(style).unwrap_or_default();
                    bail!("path parameter `{name}`: style {style} is not supported")
                }
                openapi::Parameter::Cookie { .. } => {
                    bail!("cookie parameter `{name}` is not supported")
                }
            }
        }

        let request_body_optional = op
            .request_body
            .as_ref()
            .and_then(|b| b.as_item())
            .is_some_and(|b| !b.required);
        let mut request = RequestBody::default();
        if let Some(body) = op.request_body {
            let ReferenceOr::Item(body) = body else {
                bail!("unresolved `$ref` request body");
            };
            request = RequestBody::from_openapi(body, component_schemas).context("request body")?;
        }

        let responses = op.responses.unwrap_or_default();
        let (response, error_response_schema_names) =
            responses_from_openapi(responses, component_schemas)?;

        let x_pagination = op.extensions.get("x-pagination").cloned();
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
            request_body_schema_name: request.schema_name,
            request_body_is_list: request.is_list,
            request_body_json_type: request.json_type,
            request_body_all_optional: request.all_optional,
            request_body_optional,
            request_body_is_form: request.kind == RequestBodyKind::Form,
            request_body_kind: request.kind,
            multipart_fields: request.multipart_fields,
            response_body_schema_name: response.schema_name,
            response_body_is_list: response.is_list,
            response_body_json_type: response.json_type,
            response_is_binary: response.kind == ResponseKind::Binary,
            response_is_text: response.kind == ResponseKind::Text,
            response_is_event_stream: response.kind == ResponseKind::EventStream,
            error_response_schema_names,
            security: None,
            pagination: None,
            x_pagination,
        };
        Ok(Some((res_path, op)))
    }

    /// Replaces alias bodies, which only Rust declares, by their target.
    fn inline_body_aliases(&mut self, aliases: &BTreeMap<String, FieldType>) -> anyhow::Result<()> {
        self.error_response_schema_names
            .retain(|name| !aliases.contains_key(name));
        if let Some(name) = self.response_body_schema_name.clone()
            && let Some(target) = aliases.get(&name)
        {
            match target {
                FieldType::List { inner } if !self.response_body_is_list => {
                    let FieldType::SchemaRef { name: item, .. } = &**inner else {
                        bail!("response schema `{name}` is not supported");
                    };
                    self.response_body_schema_name = Some(item.clone());
                    self.response_body_is_list = true;
                }
                target if target.is_plain_json() => {
                    let target = target.clone();
                    self.response_body_json_type = Some(match self.response_body_is_list {
                        true => FieldType::List {
                            inner: Arc::new(target),
                        },
                        false => target,
                    });
                    self.response_body_schema_name = None;
                    self.response_body_is_list = false;
                }
                _ => bail!("response schema `{name}` is not supported"),
            }
        }
        if let Some(name) = self.request_body_schema_name.clone()
            && let Some(target) = aliases.get(&name)
        {
            match target {
                FieldType::List { inner } if !self.request_body_is_list => {
                    let FieldType::SchemaRef { name: item, .. } = &**inner else {
                        bail!("request body schema `{name}` is not supported");
                    };
                    self.request_body_schema_name = Some(item.clone());
                    self.request_body_is_list = true;
                }
                target if target.is_plain_json() && !self.request_body_is_list => {
                    self.request_body_json_type = Some(target.clone());
                    self.request_body_schema_name = None;
                }
                _ => bail!("request body schema `{name}` is not supported"),
            }
        }
        Ok(())
    }

    /// Applies `x-pagination`, or the first perseid.toml rule matching this operation.
    fn resolve_pagination(
        &mut self,
        rules: &[config::Pagination],
        types: &Types,
    ) -> anyhow::Result<()> {
        let candidate = Candidate {
            query_params: self
                .query_params
                .iter()
                .map(|p| (p.name.as_str(), &p.r#type))
                .collect(),
            response: self.response_body_schema_name.as_deref(),
        };
        self.pagination = match self.x_pagination.take() {
            Some(serde_json::Value::Bool(false)) => None,
            Some(value) => {
                let spec: config::Pagination =
                    serde_json::from_value(value).context("invalid x-pagination")?;
                ensure!(
                    spec.operations.is_empty(),
                    "`operations` only applies to perseid.toml"
                );
                Some(Pagination::resolve(&spec, &candidate, types)?)
            }
            None => {
                let mut found = None;
                for rule in rules {
                    let listed = rule.operations.contains(&self.id);
                    if !rule.operations.is_empty() && !listed {
                        continue;
                    }
                    match Pagination::resolve(rule, &candidate, types) {
                        Ok(pagination) => {
                            found = Some(pagination);
                            break;
                        }
                        Err(error) if listed => return Err(error),
                        Err(_) => {}
                    }
                }
                found
            }
        };
        Ok(())
    }

    pub(crate) fn has_query_or_header_params(&self) -> bool {
        !self.header_params.is_empty() || !self.query_params.is_empty()
    }
}

/// The resource of an untagged operation: `/api/v2/things/{id}` belongs to `things`.
fn resource_from_path(path: &str) -> String {
    let is_version = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_digit())
            || s.strip_prefix(['v', 'V'])
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    path.split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{') && *s != "api" && !is_version(s))
        .map(|s| s.split('.').next().unwrap_or(s))
        .find(|s| !s.is_empty())
        .unwrap_or("default")
        .to_owned()
}

#[derive(Debug, Default)]
struct RequestBody {
    kind: RequestBodyKind,
    schema_name: Option<String>,
    is_list: bool,
    json_type: Option<FieldType>,
    all_optional: bool,
    multipart_fields: Vec<MultipartField>,
}

impl RequestBody {
    fn from_openapi(
        mut body: openapi::RequestBody,
        schemas: &IndexMap<String, openapi::SchemaObject>,
    ) -> anyhow::Result<Self> {
        let content_types = body.content.keys().join("`, `");
        let (kind, media) = if let Some(media) = body.content.swap_remove("application/json") {
            (RequestBodyKind::Json, media)
        } else if let Some(media) = body
            .content
            .swap_remove("application/x-www-form-urlencoded")
        {
            (RequestBodyKind::Form, media)
        } else if let Some(media) = body.content.swap_remove("multipart/form-data") {
            ensure!(
                media.encoding.is_empty(),
                "custom multipart encodings are not supported"
            );
            let schema = media.schema.context("missing multipart schema")?;
            return Ok(Self {
                kind: RequestBodyKind::Multipart,
                multipart_fields: multipart_fields(schema.json_schema, schemas)?,
                ..Self::default()
            });
        } else if body
            .content
            .swap_remove("application/octet-stream")
            .is_some()
        {
            return Ok(Self {
                kind: RequestBodyKind::Binary,
                ..Self::default()
            });
        } else {
            bail!("content type `{content_types}` is not supported");
        };
        let Some(schema) = media.schema else {
            ensure!(kind == RequestBodyKind::Json, "a form body needs a schema");
            return Ok(Self {
                kind,
                json_type: Some(FieldType::JsonObject),
                ..Self::default()
            });
        };
        let Schema::Object(obj) = schema.json_schema else {
            bail!("boolean schemas are not supported");
        };
        if kind == RequestBodyKind::Json
            && let Some((name, is_list)) = named_or_list_of_named(&obj, schemas)
            && is_list
        {
            return Ok(Self {
                kind,
                schema_name: Some(name),
                is_list,
                ..Self::default()
            });
        }
        let Some(name) = get_schema_name(obj.reference.as_deref()) else {
            let description = describe_schema(&obj);
            let json_type = FieldType::from_schema_object(obj)?;
            ensure!(
                kind == RequestBodyKind::Json && json_type.is_plain_json(),
                "only object schemas, lists of them and plain JSON values are supported, not \
                 {description}"
            );
            return Ok(Self {
                kind,
                json_type: Some(json_type),
                ..Self::default()
            });
        };
        let all_optional = match schemas.get(&name).map(|s| &s.json_schema) {
            Some(Schema::Object(obj)) => obj.object.as_ref().is_none_or(|o| o.required.is_empty()),
            _ => false,
        };
        Ok(Self {
            kind,
            schema_name: Some(name),
            all_optional,
            ..Self::default()
        })
    }
}

/// A short description of an unsupported schema for error messages, never a dump of it.
fn describe_schema(obj: &SchemaObject) -> String {
    if let Some(subschemas) = &obj.subschemas {
        if subschemas.one_of.is_some() {
            return "a `oneOf` union".into();
        }
        if subschemas.any_of.is_some() {
            return "an `anyOf` union".into();
        }
    }
    match &obj.instance_type {
        Some(SingleOrVec::Single(t)) => format!("type `{}`", instance_type_name(t)),
        Some(SingleOrVec::Vec(types)) => format!(
            "type `{}`",
            types.iter().map(instance_type_name).join(" | ")
        ),
        None => "an untyped schema".into(),
    }
}

fn instance_type_name(t: &InstanceType) -> &'static str {
    match t {
        InstanceType::Null => "null",
        InstanceType::Boolean => "boolean",
        InstanceType::Object => "object",
        InstanceType::Array => "array",
        InstanceType::Number => "number",
        InstanceType::String => "string",
        InstanceType::Integer => "integer",
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

    bail!(
        "only scalar values are supported, not {}",
        describe_schema(obj)
    );
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

#[derive(Debug, Default, PartialEq)]
struct ResponseBody {
    kind: ResponseKind,
    schema_name: Option<String>,
    is_list: bool,
    json_type: Option<FieldType>,
}

/// Picks the body the SDK decodes on success: the lowest 2xx status with content, or `default`
/// when no 2xx is declared. Returns it with the JSON error schemas of 4xx, 5xx and `default`.
fn responses_from_openapi(
    responses: openapi::Responses,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> anyhow::Result<(ResponseBody, BTreeSet<String>)> {
    let mut success = Vec::new();
    let mut errors = Vec::new();
    for (status, response) in responses.responses {
        let ReferenceOr::Item(response) = response else {
            bail!("response `{status}`: unresolved `$ref` response");
        };
        match status {
            openapi::StatusCode::Code(code @ 200..300) => success.push((code, status, response)),
            openapi::StatusCode::Range(2) => success.push((299, status, response)),
            openapi::StatusCode::Code(400..) | openapi::StatusCode::Range(4 | 5) => {
                errors.push(response)
            }
            // Informational and redirect responses have no body for the SDK to decode.
            _ => {}
        }
    }
    if let Some(default) = responses.default {
        let ReferenceOr::Item(default) = default else {
            bail!("response `default`: unresolved `$ref` response");
        };
        match success.is_empty() {
            true => success.push((0, openapi::StatusCode::Code(0), default)),
            false => errors.push(default),
        }
    }
    success.sort_by_key(|(code, ..)| *code);

    let mut chosen: Option<(String, ResponseBody)> = None;
    for (_, status, response) in success {
        let status = match status {
            openapi::StatusCode::Code(0) => "default".to_owned(),
            status => status.to_string(),
        };
        let body = ResponseBody::from_openapi(response, schemas)
            .with_context(|| format!("response `{status}`"))?;
        match &chosen {
            _ if body.kind == ResponseKind::None => {}
            None => chosen = Some((status, body)),
            Some((first, kept)) if *kept != body => tracing::warn!(
                "responses `{first}` and `{status}` have different bodies, the SDK decodes `{first}`"
            ),
            Some(_) => {}
        }
    }
    let error_schemas = errors
        .into_iter()
        .filter_map(|response| {
            let schema = response.content.get("application/json")?.schema.as_ref()?;
            let Schema::Object(obj) = &schema.json_schema else {
                return None;
            };
            get_schema_name(obj.reference.as_deref())
        })
        .collect();
    Ok((
        chosen.map(|(_, body)| body).unwrap_or_default(),
        error_schemas,
    ))
}

impl ResponseBody {
    fn from_openapi(
        response: openapi::Response,
        schemas: &IndexMap<String, openapi::SchemaObject>,
    ) -> anyhow::Result<Self> {
        let content = response.content;
        let kind = |kind| Self {
            kind,
            ..Self::default()
        };
        if content.is_empty() {
            return Ok(Self::default());
        }
        if content.contains_key("text/event-stream") {
            return Ok(kind(ResponseKind::EventStream));
        }
        if let Some(json) = content.get("application/json") {
            let Some(schema) = &json.schema else {
                bail!("JSON response without a schema");
            };
            let Schema::Object(obj) = &schema.json_schema else {
                bail!("boolean schemas are not supported");
            };
            if let Some((schema_name, is_list)) = named_or_list_of_named(obj, schemas) {
                return Ok(Self {
                    kind: ResponseKind::Json,
                    schema_name: Some(schema_name),
                    is_list,
                    json_type: None,
                });
            }
            let json_type = FieldType::from_schema_object(obj.clone())?;
            ensure!(
                json_type.is_plain_json(),
                "only object schemas, lists of them and plain JSON values are supported, not {}",
                describe_schema(obj)
            );
            return Ok(Self {
                kind: ResponseKind::Json,
                json_type: Some(json_type),
                ..Self::default()
            });
        }
        if content.keys().any(|k| k.starts_with("text/")) {
            return Ok(kind(ResponseKind::Text));
        }
        Ok(kind(ResponseKind::Binary))
    }
}

/// `$ref: X` or `{type: array, items: {$ref: X}}`, following references to array components.
fn named_or_list_of_named(
    obj: &SchemaObject,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> Option<(String, bool)> {
    let items = |obj: &SchemaObject| match obj.array.as_ref()?.items.as_ref()? {
        SingleOrVec::Single(item) => match &**item {
            Schema::Object(item) => get_schema_name(item.reference.as_deref()),
            Schema::Bool(_) => None,
        },
        SingleOrVec::Vec(_) => None,
    };
    let Some(mut name) = get_schema_name(obj.reference.as_deref()) else {
        return Some((items(obj)?, true));
    };
    for _ in 0..16 {
        match schemas.get(&name).map(|s| &s.json_schema) {
            Some(Schema::Object(target)) if target.reference.is_some() => {
                name = get_schema_name(target.reference.as_deref())?;
            }
            Some(Schema::Object(target))
                if target.instance_type == Some(InstanceType::Array.into()) =>
            {
                return Some((items(target)?, true));
            }
            _ => break,
        }
    }
    Some((name, false))
}

#[derive(Clone, Deserialize, Serialize)]
struct HeaderParam {
    name: String,
    required: bool,
}

#[derive(Clone, Deserialize, Serialize)]
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn schemas(value: Value) -> IndexMap<String, openapi::SchemaObject> {
        serde_json::from_value(value).unwrap()
    }

    fn responses(value: Value) -> anyhow::Result<(ResponseBody, BTreeSet<String>)> {
        let schemas = schemas(json!({
            "Widgets": { "type": "array", "items": { "$ref": "#/components/schemas/Widget" } }
        }));
        responses_from_openapi(serde_json::from_value(value).unwrap(), &schemas)
    }

    fn json_body(schema: Value) -> Value {
        json!({ "description": "", "content": { "application/json": { "schema": schema } } })
    }

    fn widget() -> Value {
        json!({ "$ref": "#/components/schemas/Widget" })
    }

    #[test]
    fn default_is_an_error_response_next_to_a_success() {
        let (body, errors) = responses(json!({
            "200": json_body(widget()),
            "default": json_body(json!({ "$ref": "#/components/schemas/Error" })),
            "4XX": json_body(json!({ "$ref": "#/components/schemas/Invalid" })),
            "304": { "description": "not modified" }
        }))
        .unwrap();
        assert_eq!(body.schema_name.as_deref(), Some("Widget"));
        assert_eq!(errors, BTreeSet::from(["Error".into(), "Invalid".into()]));
    }

    #[test]
    fn default_is_the_success_response_without_a_2xx() {
        let (body, errors) = responses(json!({ "default": json_body(widget()) })).unwrap();
        assert_eq!(body.schema_name.as_deref(), Some("Widget"));
        assert!(errors.is_empty());
    }

    #[test]
    fn the_lowest_2xx_with_a_body_is_decoded() {
        let (body, _) = responses(json!({
            "204": { "description": "" },
            "202": json_body(widget()),
            "201": { "description": "" }
        }))
        .unwrap();
        assert_eq!(body.kind, ResponseKind::Json);
        assert_eq!(body.schema_name.as_deref(), Some("Widget"));
        let (body, _) = responses(json!({ "2XX": json_body(widget()) })).unwrap();
        assert_eq!(body.schema_name.as_deref(), Some("Widget"));
    }

    #[test]
    fn lists_of_named_schemas_and_plain_json_are_response_bodies() {
        let (body, _) =
            responses(json!({ "200": json_body(json!({ "type": "array", "items": widget() })) }))
                .unwrap();
        assert_eq!(
            (body.schema_name.as_deref(), body.is_list),
            (Some("Widget"), true)
        );
        let widgets = json!({ "$ref": "#/components/schemas/Widgets" });
        let (body, _) = responses(json!({ "200": json_body(widgets) })).unwrap();
        assert_eq!(
            (body.schema_name.as_deref(), body.is_list),
            (Some("Widget"), true)
        );
        let counts = json!({ "type": "object", "additionalProperties": { "type": "integer" } });
        let (body, _) = responses(json!({ "200": json_body(counts) })).unwrap();
        assert!(matches!(body.json_type, Some(FieldType::Map { .. })));
        let error = responses(json!({ "200": json_body(json!({
            "type": "object", "properties": { "a": { "type": "string" } }
        })) }))
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("response `200`"), "{error:#}");
    }

    #[test]
    fn non_json_responses_are_text_or_binary() {
        let content =
            |media: &str| json!({ "200": { "description": "", "content": { media: {} } } });
        let (body, _) = responses(content("text/csv")).unwrap();
        assert_eq!(body.kind, ResponseKind::Text);
        let (body, _) = responses(content("audio/mpeg")).unwrap();
        assert_eq!(body.kind, ResponseKind::Binary);
    }

    fn request(content: Value) -> anyhow::Result<RequestBody> {
        let schemas = schemas(json!({ "Widget": { "type": "object", "properties": {} } }));
        let body = serde_json::from_value(json!({ "content": content })).unwrap();
        RequestBody::from_openapi(body, &schemas)
    }

    #[test]
    fn request_bodies_prefer_json_and_accept_lists_and_plain_json() {
        let body = request(json!({
            "application/xml": { "schema": widget() },
            "application/json": { "schema": widget() }
        }))
        .unwrap();
        assert_eq!(body.kind, RequestBodyKind::Json);
        assert!(body.all_optional);
        let list = json!({ "type": "array", "items": widget() });
        let body = request(json!({ "application/json": { "schema": list } })).unwrap();
        assert_eq!(
            (body.schema_name.as_deref(), body.is_list),
            (Some("Widget"), true)
        );
        let labels = json!({ "type": "array", "items": { "type": "string" } });
        let body = request(json!({ "application/json": { "schema": labels } })).unwrap();
        assert!(matches!(body.json_type, Some(FieldType::List { .. })));
        let error = request(json!({ "text/plain": { "schema": { "type": "string" } } }));
        assert!(error.unwrap_err().to_string().contains("`text/plain`"));
    }

    #[test]
    fn untagged_operations_group_by_their_first_resource_segment() {
        assert_eq!(resource_from_path("/api/v2/things/{id}/parts"), "things");
        assert_eq!(
            resource_from_path("/2010-04-01/Accounts/{Sid}.json"),
            "Accounts"
        );
        assert_eq!(resource_from_path("/"), "default");
    }

    fn operation(id: &str) -> Operation {
        let op = serde_json::from_value(json!({ "operationId": id })).unwrap();
        let (_, op) = Operation::from_openapi(
            "/x",
            "get",
            op,
            &IndexMap::new(),
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap()
        .unwrap();
        op
    }

    #[test]
    fn prefixed_operation_ids_keep_their_short_name_unless_it_collides() {
        let mut resource = Resource::new("repos".into());
        resource.operations = vec![
            operation("repos/get"),
            operation("gists/get"),
            operation("repos/list"),
        ];
        resource.disambiguate_operation_names().unwrap();
        let names: Vec<_> = resource
            .operations
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert_eq!(names, ["repos/get", "gists/get", "list"]);
    }

    #[test]
    fn unsupported_parameters_are_errors_naming_the_parameter() {
        let op = serde_json::from_value(json!({
            "operationId": "op",
            "parameters": [{ "name": "ids", "in": "header", "schema": { "type": "array", "items": { "type": "string" } } }]
        }))
        .unwrap();
        let error = Operation::from_openapi(
            "/x",
            "get",
            op,
            &IndexMap::new(),
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .err()
        .unwrap();
        assert_eq!(
            format!("{error:#}"),
            "header parameter `ids`: only scalar values are supported, not type `array`"
        );
    }
}
