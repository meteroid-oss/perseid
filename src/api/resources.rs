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

/// A defect of the spec itself, as opposed to a construct perseid does not support: it fails
/// the generation instead of skipping the operation.
#[derive(Debug)]
struct SpecError(&'static str);

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for SpecError {}

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
            tracing::warn!(path, "skipping path: `$ref` path items are not supported");
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
                    let operations =
                        &mut get_or_insert_resource(&mut resources, res_path).operations;
                    let stream = op.event_stream_variant();
                    operations.push(op);
                    operations.extend(stream);
                }
                Ok(None) => {}
                Err(e) if e.downcast_ref::<SpecError>().is_some() => errors.push(format!(
                    "operation `{op_id}` ({} {path}): {e:#}",
                    method.to_uppercase()
                )),
                // Whatever perseid does not support skips the operation, not the whole API.
                Err(e) => tracing::warn!(method, path, "skipping the operation: {e:#}"),
            }
        }
    }
    for resource in resources.values_mut() {
        resource.disambiguate_operation_names();
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
        for ty in op
            .path_styles
            .values_mut()
            .filter_map(|p| p.r#type.as_mut())
        {
            resolve_schema_ref_in_field_type_public(ty, string_alias_names);
        }
        for ty in op
            .header_params
            .iter_mut()
            .map(|p| &mut p.r#type)
            .chain(op.typed_path_params.iter_mut().map(|p| &mut p.r#type))
        {
            resolve_schema_ref_in_field_type_public(ty, string_alias_names);
        }
        for ty in op
            .request_body_json_type
            .iter_mut()
            .chain(op.response_body_json_type.iter_mut())
        {
            resolve_schema_ref_in_field_type_public(ty, string_alias_names);
        }
    }
}

/// Marks the query parameters whose values are objects, lists of objects or untyped JSON, which
/// SDKs encode from their JSON value, `filter[name]=x` style.
pub(crate) fn mark_structured_query_params(resources: &mut Resources, types: &Types) {
    fn is_structured(ty: &FieldType, types: &Types, depth: usize) -> bool {
        match ty {
            FieldType::JsonObject | FieldType::Union { .. } | FieldType::Map { .. } => true,
            FieldType::List { inner } | FieldType::Set { inner } => {
                is_structured(inner, types, depth)
            }
            FieldType::SchemaRef { name, .. } => match types.get(name).map(|t| &t.data) {
                Some(TypeData::Alias { target }) if depth < 16 => {
                    is_structured(target, types, depth + 1)
                }
                Some(TypeData::Struct { .. } | TypeData::StructEnum { .. }) => true,
                _ => false,
            },
            _ => false,
        }
    }
    let mut stack: Vec<&mut Resource> = resources.values_mut().collect();
    while let Some(resource) = stack.pop() {
        for op in &mut resource.operations {
            for param in &mut op.query_params {
                param.structured |= is_structured(&param.r#type, types, 0);
            }
        }
        stack.extend(resource.subresources.values_mut());
    }
}

/// Names the unions of the JSON bodies that no schema names, after the operation and avoiding
/// the names of `types`.
pub(crate) fn name_body_unions(resources: &mut Resources, types: &Types) {
    fn visit(resource: &mut Resource, taken: &BTreeSet<String>) {
        for op in &mut resource.operations {
            op.name_body_unions(taken);
        }
        for sub in resource.subresources.values_mut() {
            visit(sub, taken);
        }
    }
    let taken: BTreeSet<String> = types.keys().map(|k| k.to_upper_camel_case()).collect();
    for resource in resources.values_mut() {
        visit(resource, &taken);
    }
}

/// The schemas sent in requests and those received in responses, before following references.
pub(crate) fn request_and_response_roots(
    resources: &Resources,
) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let mut requests = BTreeSet::new();
    let mut responses = BTreeSet::new();
    let mut stack: Vec<&Resource> = resources.values().collect();
    while let Some(resource) = stack.pop() {
        stack.extend(resource.subresources.values());
        for op in &resource.operations {
            requests.extend(op.request_body_schema_name.as_deref());
            let sent = (op.request_body_json_type.iter())
                .chain(op.multipart_fields.iter().map(|f| &f.field.r#type))
                .chain(op.query_params.iter().map(|p| &p.r#type))
                .chain(op.path_styles.values().filter_map(|p| p.r#type.as_ref()))
                .chain(op.header_params.iter().map(|p| &p.r#type));
            for ty in sent {
                requests.extend(ty.referenced_schema());
                requests.extend(ty.union_refs());
            }
            responses.extend(op.response_body_schema_name.as_deref());
            responses.extend(
                op.response_body_json_type
                    .iter()
                    .flat_map(|ty| ty.referenced_schema().into_iter().chain(ty.union_refs())),
            );
            responses.extend(op.error_response_schema_names.iter().map(String::as_str));
        }
    }
    (requests, responses)
}

/// A `pet` resource and a `Pet` schema would both be `Pet` in most SDKs: the resource becomes
/// `pet_api`, as do resources named like a type the SDK already uses.
pub(crate) fn rename_resources_named_like_types(
    resources: &mut Resources,
    types: &Types,
    reserved: &BTreeSet<String>,
) {
    let mut type_names: BTreeSet<String> = types.keys().map(|n| n.to_upper_camel_case()).collect();
    type_names.extend(reserved.iter().cloned());
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
            for ty in operation
                .path_styles
                .values_mut()
                .filter_map(|p| p.r#type.as_mut())
            {
                ty.inline_aliases(aliases);
            }
            for ty in operation
                .header_params
                .iter_mut()
                .map(|p| &mut p.r#type)
                .chain(
                    operation
                        .typed_path_params
                        .iter_mut()
                        .map(|p| &mut p.r#type),
                )
            {
                ty.inline_aliases(aliases);
            }
            for field in &mut operation.multipart_fields {
                field.field.r#type.inline_aliases(aliases);
            }
            for ty in operation
                .request_body_json_type
                .iter_mut()
                .chain(operation.response_body_json_type.iter_mut())
            {
                ty.inline_aliases(aliases);
            }
            operation.untype_unions();
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

    /// Falls back to the full operation id for operations whose short names collide. Names that
    /// still clash are reported by `naming::apply`, once `x-perseid-name` and `[methods]` apply.
    fn disambiguate_operation_names(&mut self) {
        let key = |op: &Operation| op.name.to_snake_case();
        let counts = self.operations.iter().map(key).counts();
        for op in &mut self.operations {
            if counts[&key(op)] > 1 {
                op.name = op.id.clone();
            }
        }
    }

    fn new(name: String) -> Self {
        Self {
            name,
            operations: Vec::new(),
            subresources: BTreeMap::new(),
        }
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
                res.extend(param.r#type.union_refs());
            }
            for param in operation.path_styles.values() {
                if let Some(name) = param.r#type.as_ref().and_then(FieldType::referenced_schema) {
                    res.insert(name);
                }
            }
            for param in &operation.header_params {
                res.extend(param.r#type.referenced_schema());
            }
            if let Some(name) = &operation.request_body_schema_name {
                res.insert(name);
            }
            if let Some(name) = &operation.response_body_schema_name {
                res.insert(name);
            }
            res.extend(
                [
                    &operation.request_body_json_type,
                    &operation.response_body_json_type,
                ]
                .into_iter()
                .flatten()
                .flat_map(|ty| ty.referenced_schema().into_iter().chain(ty.union_refs())),
            );
            if let Some(pagination) = &operation.pagination {
                res.insert(&pagination.item_schema);
            }
            if let Some(name) = &operation.event_schema_name {
                res.insert(name);
            }
            res.extend(
                operation
                    .event_json_type
                    .iter()
                    .flat_map(|ty| ty.referenced_schema().into_iter().chain(ty.union_refs())),
            );
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
    /// An array of files, sent as repeated parts of the same name.
    #[serde(default)]
    is_file_list: bool,
    /// The `contentType` of the part's `encoding`: the media type of the part, or of a file
    /// whose upload does not carry its own. Absent when the spec leaves it to the default or
    /// lists several types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
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
    encodings: &IndexMap<String, openapi::Encoding>,
) -> anyhow::Result<Vec<MultipartField>> {
    let object = resolve_multipart_schema(schema, schemas)?
        .object
        .context("multipart body must declare object properties")?;
    object
        .properties
        .into_iter()
        .map(|(name, mut schema)| {
            let mut resolved = resolve_multipart_schema(schema.clone(), schemas)?;
            let mut is_file_list = false;
            if let Some(items) = resolved.array.as_mut().and_then(|a| a.items.as_mut()) {
                let SingleOrVec::Single(item) = items else {
                    bail!("multipart tuple fields are not supported");
                };
                let mut file = resolve_multipart_schema((**item).clone(), schemas)?;
                if file.format.as_deref() == Some("binary") {
                    // An array of files: `Vec<String>` stands for it in the field's type.
                    file.format = None;
                    **item = Schema::Object(file);
                    is_file_list = true;
                    schema = Schema::Object(resolved.clone());
                }
            }
            let is_file = is_file_list || resolved.format.as_deref() == Some("binary");
            if is_file && !is_file_list {
                resolved.format = None;
                schema = Schema::Object(resolved);
            }
            let content_type = encodings
                .get(&name)
                .and_then(|e| e.content_type.as_deref())
                .map(str::trim)
                // A list of media types, or a wildcard, leaves the choice to the caller.
                .filter(|c| !c.is_empty() && !(is_file && (c.contains(',') || c.contains('*'))))
                .map(str::to_owned);
            let field = Field::from_schema(name.clone(), schema, object.required.contains(&name))?;
            Ok(MultipartField {
                field,
                is_file,
                is_file_list,
                content_type,
            })
        })
        .collect()
}

/// A named HTTP endpoint.
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Operation {
    /// The operation ID from the spec.
    pub(crate) id: String,
    /// The name in code: after the HTTP method and the path within the resource (`list`,
    /// `retrieve`, `create_source`), or the override of `[methods]`/`x-perseid-name`.
    pub(crate) name: String,
    /// `x-perseid-name` of the operation.
    #[serde(skip)]
    pub(crate) x_perseid_name: Option<String>,
    /// Whether this is the `_stream` twin of another operation.
    #[serde(skip)]
    pub(crate) stream: bool,
    /// Description of the operation to use for documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Whether this operation is marked as deprecated.
    deprecated: bool,
    /// The HTTP method.
    ///
    /// Encoded as "get", "post" or such because that's what aide's PathItem iterator gives us.
    pub(crate) method: String,
    /// The operation's endpoint path.
    pub(crate) path: String,
    /// Path parameters.
    ///
    /// SDKs take them as strings, unless `path_styles` types them.
    path_params: Vec<String>,
    /// The path parameters that are not plain `simple` text, by name: other styles, typed
    /// scalars (numbers, booleans, dates, enums), lists, objects and `content` values.
    #[serde(default)]
    pub(crate) path_styles: BTreeMap<String, PathStyle>,
    /// Path parameters with their types, in `path_params` order.
    #[serde(default)]
    pub(crate) typed_path_params: Vec<TypedParam>,
    /// Header and cookie parameters.
    pub(crate) header_params: Vec<HeaderParam>,
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
    /// Properties of a form body encoded `style: deepObject`: lists as `name[]=a&name[]=b`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    form_deep_object: Vec<String>,
    /// Properties of a form body whose lists are comma-separated (`explode: false`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    form_unexploded: Vec<String>,
    /// Media type of a binary body other than `application/octet-stream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_body_content_type: Option<String>,
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
    pub(crate) request_body_json_type: Option<FieldType>,
    /// Name of the union a JSON request body holds, directly or as list items or map values:
    /// the schema when the body is a named union, else `<OperationId>Request`. Set when
    /// `request_body_json_type` contains a union.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_body_union: Option<String>,
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
    pub(crate) response_body_json_type: Option<FieldType>,
    /// Name of the union a JSON response holds, as `request_body_union` for the request: the
    /// schema when the body is a named union, else `<OperationId>Response`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response_body_union: Option<String>,
    /// True if the response is binary (e.g., application/pdf, application/octet-stream).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_is_binary: bool,
    /// True if the response is text (e.g., text/plain, text/html).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_is_text: bool,
    #[serde(default)]
    response_is_event_stream: bool,
    /// Whether the success body may be absent: another success status has no body, such as a
    /// `204`, or the JSON body may be `null`. SDKs return an optional, except for pages.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_may_be_empty: bool,
    /// Schema of the JSON `data` of each event, when the `text/event-stream` response names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) event_schema_name: Option<String>,
    /// Type of each event when `event_schema_name` is an alias, for SDKs without named aliases:
    /// the alias target, a union named after the alias.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_field_type"
    )]
    pub(crate) event_json_type: Option<FieldType>,
    /// Boolean body property the `_stream` twin sets to `true`, such as OpenAI's `stream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream_property: Option<String>,
    /// Schemas of the JSON bodies this operation returns on 4xx/5xx responses.
    ///
    /// Not rendered per operation: collected so that `referenced_components` pulls the error
    /// schemas (e.g. `RestErrorResponse`) into the generated models alongside everything else.
    #[serde(skip)]
    error_response_schema_names: BTreeSet<String>,
    /// Schema of the JSON body of each error response, by status: `404`, `4XX` or `default`.
    #[serde(default)]
    pub(crate) errors: BTreeMap<String, String>,
    /// Security requirement, when it differs from the API-wide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    security: Option<Requirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pagination: Option<Pagination>,
    #[serde(skip)]
    x_pagination: Option<serde_json::Value>,
    /// Whether the JSON response may be an event stream instead, which a `_stream` twin reads.
    #[serde(skip)]
    json_or_event_stream: bool,
    /// Boolean `stream` property of the request body, which asks for the event stream.
    #[serde(skip)]
    body_stream_property: Option<String>,
}

impl Operation {
    /// Whether the request carries a body.
    pub(crate) fn has_body(&self) -> bool {
        self.request_body_kind != RequestBodyKind::None
    }

    /// The schema of the JSON success body, if it is a named one.
    pub(crate) fn response_schema(&self) -> Option<&str> {
        self.response_body_schema_name.as_deref()
    }

    /// Whether the response is a list of items: an array, a paginated or `*List` schema, or an
    /// operation id saying so.
    pub(crate) fn returns_list(&self) -> bool {
        self.response_body_is_list
            || self
                .x_pagination
                .as_ref()
                .is_some_and(|value| value != &serde_json::Value::Bool(false))
            || self
                .response_body_schema_name
                .as_deref()
                .is_some_and(|name| name.ends_with("List") || name.ends_with("Page"))
            || self.id.to_snake_case().starts_with("list")
    }

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
            IncludeMode::OnlySpecified => specified_operations.contains(&op_id),
        };
        if !include_operation || excluded_operations.contains(&op_id) {
            return Ok(None);
        }

        let tag = match op.tags.first() {
            Some(tag) => tag.clone(),
            None => resource_from_path(path),
        };
        let res_path = vec![resource_name(&tag)];
        // `repos/get-content` style ids already carry their resource.
        let op_name = op_id.rsplit('/').next().unwrap_or(&op_id).to_owned();

        let mut path_params = Vec::new();
        let mut path_styles = BTreeMap::new();
        let mut typed_path_params = Vec::new();
        let mut query_params = Vec::new();
        let mut header_params = Vec::new();

        for param in op.parameters {
            let ReferenceOr::Item(param) = param else {
                return Err(SpecError("unresolved `$ref` parameter").into());
            };
            let name = param.parameter_data_ref().name.clone();
            match param {
                openapi::Parameter::Path {
                    parameter_data,
                    style,
                } => {
                    let path_style =
                        path_parameter_style(&parameter_data, style, component_schemas)
                            .with_context(|| format!("path parameter `{name}`"))?;
                    let r#type = path_style
                        .as_ref()
                        .and_then(|style| style.r#type.clone())
                        .or_else(|| FieldType::from_openapi(parameter_data.format.clone()).ok())
                        .unwrap_or(FieldType::String);
                    typed_path_params.push(TypedParam {
                        name: parameter_data.name.clone(),
                        r#type,
                    });
                    path_styles.extend(path_style.map(|style| (name.clone(), style)));
                    path_params.push(parameter_data.name);
                }
                openapi::Parameter::Header {
                    parameter_data,
                    style: openapi::HeaderStyle::Simple,
                } => {
                    enforce_string_parameter(&parameter_data, true)
                        .with_context(|| format!("header parameter `{name}`"))?;
                    // A JSON `content` header is typed by its schema and sent as compact JSON.
                    let json = matches!(&parameter_data.format,
                        openapi::ParameterSchemaOrContent::Content(content)
                            if content.keys().any(|media| is_json_media_type(media)));
                    let r#type = match json {
                        true => {
                            parameter_value(parameter_data.format, true)
                                .with_context(|| format!("header parameter `{name}`"))?
                                .0
                        }
                        false => header_type(parameter_data.format),
                    };
                    header_params.push(HeaderParam {
                        ident: parameter_data.name.clone(),
                        name: parameter_data.name,
                        required: parameter_data.required,
                        cookie: false,
                        json,
                        r#type,
                    });
                }
                // Cookie parameters are typed like headers and sent in the one `Cookie` header.
                openapi::Parameter::Cookie {
                    parameter_data,
                    style: openapi::CookieStyle::Form,
                } => {
                    enforce_string_parameter(&parameter_data, false)
                        .with_context(|| format!("cookie parameter `{name}`"))?;
                    header_params.push(HeaderParam {
                        ident: parameter_data.name.clone(),
                        name: parameter_data.name,
                        required: parameter_data.required,
                        cookie: true,
                        json: false,
                        r#type: header_type(parameter_data.format),
                    });
                }
                openapi::Parameter::Query {
                    parameter_data,
                    style,
                    ..
                } => {
                    let (r#type, json) = parameter_value(parameter_data.format, false)
                        .with_context(|| format!("query parameter `{name}`"))?;
                    // `style: form` explodes arrays (`?tags=a&tags=b`) unless `explode: false`;
                    // the delimited styles join their items unless `explode: true`.
                    let explode = parameter_data.explode.unwrap_or(matches!(
                        style,
                        openapi::QueryStyle::Form | openapi::QueryStyle::DeepObject
                    ));
                    let deep_object = matches!(style, openapi::QueryStyle::DeepObject) && !json;
                    // Exploded delimited styles repeat the name, like `form`.
                    let delimiter = match style {
                        openapi::QueryStyle::PipeDelimited => Some("|"),
                        openapi::QueryStyle::SpaceDelimited => Some(" "),
                        _ => None,
                    }
                    .filter(|_| !explode && !json)
                    .filter(|_| {
                        parameter_data.explode.is_some()
                            || matches!(
                                r#type.non_null(),
                                FieldType::List { .. } | FieldType::Set { .. }
                            )
                    });
                    if delimiter.is_some() && parameter_data.explode.is_some() {
                        ensure!(
                            matches!(
                                r#type.non_null(),
                                FieldType::List { .. } | FieldType::Set { .. }
                            ),
                            "query parameter `{name}`: delimited styles only apply to arrays"
                        );
                    }

                    query_params.push(QueryParam {
                        ident: name.clone(),
                        name,
                        description: super::html::doc(parameter_data.description),
                        required: parameter_data.required,
                        r#type,
                        explode,
                        deep_object,
                        structured: deep_object || json || delimiter.is_some(),
                        delimiter: delimiter.map(str::to_owned),
                        json,
                        typed_union: None,
                    });
                }
            }
        }

        add_undeclared_path_params(path, &mut path_params);
        for name in &path_params[typed_path_params.len().min(path_params.len())..] {
            typed_path_params.push(TypedParam {
                name: name.clone(),
                r#type: FieldType::String,
            });
        }
        disambiguate_parameters(&path_params, &mut query_params, &mut header_params);

        let request_body_optional = op
            .request_body
            .as_ref()
            .and_then(|b| b.as_item())
            .is_some_and(|b| !b.required);
        let mut request = RequestBody::default();
        if let Some(body) = op.request_body {
            let ReferenceOr::Item(body) = body else {
                return Err(SpecError("unresolved `$ref` request body").into());
            };
            request = RequestBody::from_openapi(body, component_schemas).context("request body")?;
        }

        let responses = op.responses.unwrap_or_default();
        let (response, errors) = responses_from_openapi(responses, component_schemas)?;
        let error_response_schema_names = errors.values().cloned().collect();

        let body_stream_property = request
            .schema_name
            .as_deref()
            .filter(|_| response.also_event_stream)
            .and_then(|name| stream_property(name, component_schemas));
        let x_pagination = op.extensions.get("x-pagination").cloned();
        let x_perseid_name = match op.extensions.get("x-perseid-name") {
            None => None,
            Some(serde_json::Value::String(name)) if !name.trim().is_empty() => {
                Some(name.trim().to_owned())
            }
            Some(_) => {
                return Err(SpecError("`x-perseid-name` must be a non-empty string").into());
            }
        };
        let op = Operation {
            x_perseid_name,
            stream: false,
            id: op_id,
            name: op_name,
            description: super::html::doc(operation_doc(op.summary, op.description)),
            deprecated: op.deprecated,
            method: method.to_owned(),
            path: path.to_owned(),
            path_params,
            path_styles,
            typed_path_params,
            header_params,
            query_params,
            request_body_schema_name: request.schema_name,
            request_body_is_list: request.is_list,
            request_body_json_type: request.json_type,
            request_body_union: None,
            request_body_all_optional: request.all_optional,
            request_body_optional,
            request_body_is_form: request.kind == RequestBodyKind::Form,
            request_body_kind: request.kind,
            request_body_content_type: request.content_type,
            form_deep_object: request.form_deep_object,
            form_unexploded: request.form_unexploded,
            multipart_fields: request.multipart_fields,
            response_body_schema_name: response.schema_name,
            response_body_is_list: response.is_list,
            response_body_json_type: response.json_type,
            response_body_union: None,
            response_is_binary: response.kind == ResponseKind::Binary,
            response_is_text: response.kind == ResponseKind::Text,
            response_is_event_stream: response.kind == ResponseKind::EventStream,
            response_may_be_empty: response.may_be_empty,
            event_schema_name: response.event_schema_name,
            event_json_type: None,
            stream_property: None,
            json_or_event_stream: response.also_event_stream,
            body_stream_property,
            error_response_schema_names,
            errors,
            security: None,
            pagination: None,
            x_pagination,
        };
        Ok(Some((res_path, op)))
    }

    /// The `{name}_stream` twin of an operation answering JSON or an event stream, for the
    /// requests that ask for the stream (such as OpenAI's `stream: true`).
    fn event_stream_variant(&self) -> Option<Self> {
        self.json_or_event_stream.then(|| Self {
            name: format!("{}_stream", self.name),
            stream: true,
            response_body_schema_name: None,
            response_body_is_list: false,
            response_body_json_type: None,
            response_body_union: None,
            response_is_event_stream: true,
            response_may_be_empty: false,
            stream_property: self.body_stream_property.clone(),
            json_or_event_stream: false,
            x_pagination: Some(serde_json::Value::Bool(false)),
            ..self.clone()
        })
    }

    /// Replaces alias bodies, which only Rust declares, by their target.
    fn inline_body_aliases(&mut self, aliases: &BTreeMap<String, FieldType>) -> anyhow::Result<()> {
        self.error_response_schema_names
            .retain(|name| !aliases.contains_key(name));
        self.errors.retain(|_, name| !aliases.contains_key(name));
        if let Some(target) = self
            .event_schema_name
            .as_ref()
            .and_then(|name| aliases.get(name))
        {
            self.event_json_type = Some(target.clone());
        }
        if let Some(name) = self.response_body_schema_name.clone()
            && let Some(target) = aliases.get(&name)
        {
            let target = &target.clone();
            if matches!(target.non_null(), FieldType::Union { .. }) {
                self.response_body_union = Some(name.clone());
            }
            match target {
                FieldType::List { inner }
                    if !self.response_body_is_list
                        && matches!(&**inner, FieldType::SchemaRef { .. }) =>
                {
                    let FieldType::SchemaRef { name: item, .. } = &**inner else {
                        unreachable!("matched a schema reference");
                    };
                    self.response_body_schema_name = Some(item.clone());
                    self.response_body_is_list = true;
                }
                FieldType::SchemaRef { name: target, .. } => {
                    self.response_body_schema_name = Some(target.clone());
                }
                target => {
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
            }
        }
        if let Some(name) = self.request_body_schema_name.clone()
            && let Some(target) = aliases.get(&name)
        {
            let target = &target.clone();
            if matches!(target.non_null(), FieldType::Union { .. }) {
                self.request_body_union = Some(name.clone());
            }
            match target {
                FieldType::List { inner }
                    if !self.request_body_is_list
                        && matches!(&**inner, FieldType::SchemaRef { .. }) =>
                {
                    let FieldType::SchemaRef { name: item, .. } = &**inner else {
                        unreachable!("matched a schema reference");
                    };
                    self.request_body_schema_name = Some(item.clone());
                    self.request_body_is_list = true;
                }
                FieldType::SchemaRef { name: target, .. } => {
                    self.request_body_schema_name = Some(target.clone());
                }
                target => {
                    let target = target.clone();
                    self.request_body_json_type = Some(match self.request_body_is_list {
                        true => FieldType::List {
                            inner: Arc::new(target),
                        },
                        false => target,
                    });
                    self.request_body_schema_name = None;
                    self.request_body_is_list = false;
                }
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
                Some(Pagination::resolve(&spec, &candidate, types, true)?)
            }
            None => {
                let mut found = None;
                for rule in rules {
                    let listed = rule.operations.contains(&self.id);
                    if !rule.operations.is_empty() && !listed {
                        continue;
                    }
                    match Pagination::resolve(rule, &candidate, types, listed) {
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
        // A page is always there.
        if self.pagination.is_some() {
            self.response_may_be_empty = false;
        }
        Ok(())
    }

    /// Types the unions of its parameters as untyped JSON, keeping those of query parameters in
    /// `typed_union`. Bodies keep their unions, and multipart fields cannot have any.
    pub(crate) fn untype_unions(&mut self) {
        for param in &mut self.query_params {
            // A JSON `content` parameter is sent as JSON text, whatever its type.
            if matches!(param.r#type, FieldType::Union { .. }) && !param.json {
                param.typed_union = Some(param.r#type.clone());
            }
        }
        let types = self.query_params.iter_mut().map(|p| &mut p.r#type).chain(
            self.multipart_fields
                .iter_mut()
                .map(|f| &mut f.field.r#type),
        );
        for ty in types {
            ty.untype_unions();
        }
    }

    /// Settles the unions of objects that no property tells apart: typed as best match when
    /// `best_match`, else untyped JSON, counting each in `counts`.
    pub(crate) fn settle_object_unions(&mut self, best_match: bool, counts: &mut (usize, usize)) {
        let types = self
            .query_params
            .iter_mut()
            .flat_map(|p| std::iter::once(&mut p.r#type).chain(p.typed_union.as_mut()))
            .chain(self.request_body_json_type.as_mut())
            .chain(self.response_body_json_type.as_mut())
            .chain(self.event_json_type.as_mut());
        for ty in types {
            ty.settle_object_unions(best_match, counts);
        }
    }

    /// Names the unions of its JSON bodies that no schema names, avoiding the `taken` names.
    fn name_body_unions(&mut self, taken: &BTreeSet<String>) {
        let base = self.id.to_upper_camel_case();
        let name = |suffix: &str| {
            let mut name = format!("{base}{suffix}");
            while taken.contains(&name) {
                name.push_str("Body");
            }
            name
        };
        if self.request_body_union.is_none()
            && self
                .request_body_json_type
                .as_ref()
                .is_some_and(FieldType::contains_union)
        {
            self.request_body_union = Some(name("Request"));
        }
        if self.response_body_union.is_none()
            && self
                .response_body_json_type
                .as_ref()
                .is_some_and(FieldType::contains_union)
        {
            self.response_body_union = Some(name("Response"));
        }
    }

    /// Drops the typed unions of parameters naming a schema no model is generated for.
    pub(crate) fn forget_typed_unions_of_unknown_types(&mut self, types: &Types) {
        for param in &mut self.query_params {
            if let Some(ty) = &param.typed_union
                && !ty.union_refs().iter().all(|name| types.contains_key(*name))
            {
                param.typed_union = None;
            }
        }
    }

    pub(crate) fn has_query_or_header_params(&self) -> bool {
        !self.header_params.is_empty() || !self.query_params.is_empty()
    }
}

/// A tag as a resource name, which every SDK can use as an identifier: `1-Click Apps` becomes
/// `one_click_apps`, `Requête` `requete` and `OAuth` `oauth`.
fn resource_name(tag: &str) -> String {
    const DIGITS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let name = super::naming::snake(&deunicode::deunicode(tag));
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    let spelled = name
        .bytes()
        .take(digits)
        .map(|d| DIGITS[usize::from(d - b'0')]);
    match (digits, name[digits..].trim_start_matches('_')) {
        (0, _) => name,
        (_, "") => spelled.collect::<Vec<_>>().join("_"),
        (_, rest) => format!("{}_{rest}", spelled.collect::<Vec<_>>().join("_")),
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
    content_type: Option<String>,
    form_deep_object: Vec<String>,
    form_unexploded: Vec<String>,
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
        let (kind, media) = if let Some(media) = body.content.swap_remove("application/json") {
            (RequestBodyKind::Json, media)
        } else if let Some(media) = body
            .content
            .swap_remove("application/x-www-form-urlencoded")
        {
            (RequestBodyKind::Form, media)
        } else if let Some(media) = body.content.swap_remove("multipart/form-data") {
            let schema = media.schema.context("missing multipart schema")?;
            return Ok(Self {
                kind: RequestBodyKind::Multipart,
                multipart_fields: multipart_fields(schema.json_schema, schemas, &media.encoding)?,
                ..Self::default()
            });
        } else if let Some(content_type) = body.content.keys().next() {
            // Any other media type is sent as the bytes the caller provides.
            let content_type = match content_type.as_str() {
                "*/*" | "application/octet-stream" => None,
                _ if body.content.contains_key("application/octet-stream") => None,
                other => Some(other.to_owned()),
            };
            return Ok(Self {
                kind: RequestBodyKind::Binary,
                content_type,
                ..Self::default()
            });
        } else {
            bail!("request bodies need a content type");
        };
        let mut form_deep_object = Vec::new();
        let mut form_unexploded = Vec::new();
        for (name, encoding) in &media.encoding {
            match encoding.style {
                Some(openapi::QueryStyle::DeepObject) => form_deep_object.push(name.clone()),
                Some(openapi::QueryStyle::Form) | None if !encoding.explode => {
                    form_unexploded.push(name.clone())
                }
                _ => {}
            }
        }
        let Some(schema) = media.schema else {
            ensure!(kind == RequestBodyKind::Json, "a form body needs a schema");
            return Ok(Self {
                kind,
                json_type: Some(FieldType::JsonObject),
                ..Self::default()
            });
        };
        let Schema::Object(obj) = schema.json_schema else {
            ensure!(kind == RequestBodyKind::Json, "a form body needs a schema");
            return Ok(Self {
                kind,
                json_type: Some(FieldType::JsonObject),
                ..Self::default()
            });
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
            ensure!(
                kind == RequestBodyKind::Json,
                "a form body needs a `$ref` to an object schema, not an inline {description}"
            );
            let json_type = FieldType::from_schema_object(obj)?;
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
            form_deep_object,
            form_unexploded,
            ..Self::default()
        })
    }
}

/// The summary, often the only text of an operation, then the description unless it repeats it.
fn operation_doc(summary: Option<String>, description: Option<String>) -> Option<String> {
    let summary = summary.filter(|s| !s.trim().is_empty());
    let description = description.filter(|d| !d.trim().is_empty());
    match (summary, description) {
        (Some(summary), Some(description))
            if !description.starts_with(summary.trim().trim_end_matches('.')) =>
        {
            Some(format!("{}\n\n{description}", summary.trim()))
        }
        (summary, description) => description.or(summary),
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

/// The type of a parameter and whether it is a `content: application/json` one, which is sent
/// as JSON text. A value whose schema has no SDK type, such as an inline object with properties,
/// is untyped JSON when `lenient`, and always for `content`.
fn parameter_value(
    format: openapi::ParameterSchemaOrContent,
    lenient: bool,
) -> anyhow::Result<(FieldType, bool)> {
    let openapi::ParameterSchemaOrContent::Content(content) = format else {
        return match FieldType::from_openapi(format) {
            Ok(ty) => Ok((ty, false)),
            Err(_) if lenient => Ok((FieldType::JsonObject, false)),
            Err(error) => Err(error),
        };
    };
    let (media_type, media) = content
        .into_iter()
        .next()
        .context("`content` needs a media type")?;
    ensure!(
        is_json_media_type(&media_type),
        "`content` of media type `{media_type}` is not supported, only JSON"
    );
    let schema = media.schema.context("`content` needs a schema")?;
    let ty = FieldType::from_openapi(openapi::ParameterSchemaOrContent::Schema(schema))
        .unwrap_or(FieldType::JsonObject);
    Ok((ty, true))
}

fn is_json_media_type(media_type: &str) -> bool {
    let media_type = media_type.to_ascii_lowercase();
    let essence = media_type.split(';').next().unwrap_or_default().trim();
    essence == "application/json" || essence.ends_with("+json")
}

/// Whether a schema is an array or an object once its `$ref`s are followed.
fn is_collection_schema(
    obj: &SchemaObject,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> bool {
    let mut obj = obj;
    for _ in 0..16 {
        if let Some(name) = get_schema_name(obj.reference.as_deref()) {
            match schemas.get(&name).map(|s| &s.json_schema) {
                Some(Schema::Object(target)) => obj = target,
                _ => return false,
            }
            continue;
        }
        return obj.array.is_some()
            || obj.object.is_some()
            || matches!(
                &obj.instance_type,
                Some(SingleOrVec::Single(t))
                    if matches!(**t, InstanceType::Array | InstanceType::Object)
            );
    }
    false
}

/// How a path parameter is serialized, or `None` for a plain `simple` text scalar, which SDKs
/// take as a string. Other scalars keep their type, like the fields they are read from.
fn path_parameter_style(
    parameter_data: &openapi::ParameterData,
    style: openapi::PathStyle,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> anyhow::Result<Option<PathStyle>> {
    let scalar = match &parameter_data.format {
        openapi::ParameterSchemaOrContent::Schema(s) => match &s.json_schema {
            Schema::Object(obj) => is_text(obj, false, 0) && !is_collection_schema(obj, schemas),
            Schema::Bool(_) => bail!("found unexpected `true` schema"),
        },
        openapi::ParameterSchemaOrContent::Content(_) => false,
    };
    let style = match style {
        openapi::PathStyle::Simple => "simple",
        openapi::PathStyle::Label => "label",
        openapi::PathStyle::Matrix => "matrix",
    };
    let explode = parameter_data.explode.unwrap_or(false);
    if scalar {
        let r#type = FieldType::from_openapi(parameter_data.format.clone())
            .ok()
            .filter(|ty| !is_text_type(ty, schemas));
        return Ok((style != "simple" || r#type.is_some()).then(|| PathStyle {
            style: style.to_owned(),
            explode,
            r#type,
        }));
    }
    let (ty, json) = parameter_value(parameter_data.format.clone(), true)?;
    Ok(Some(PathStyle {
        style: if json { "json" } else { style }.to_owned(),
        explode,
        r#type: Some(ty),
    }))
}

/// Whether a scalar parameter is plain text: a string, a union, or a `$ref` to a string.
fn is_text_type(ty: &FieldType, schemas: &IndexMap<String, openapi::SchemaObject>) -> bool {
    let mut ty = ty.clone();
    for _ in 0..16 {
        let FieldType::SchemaRef { name, .. } = &ty else {
            break;
        };
        let Some(Schema::Object(target)) = schemas.get(name).map(|s| &s.json_schema) else {
            return true;
        };
        let Ok(target) = FieldType::from_schema_object(target.clone()) else {
            return true;
        };
        ty = target;
    }
    matches!(
        ty,
        FieldType::String
            | FieldType::Union { .. }
            | FieldType::JsonObject
            | FieldType::SchemaRef { .. }
    )
}

/// The type of a header or cookie: its schema's, with unions and untyped values as text.
fn header_type(format: openapi::ParameterSchemaOrContent) -> FieldType {
    fn text(ty: FieldType) -> FieldType {
        match ty {
            FieldType::Union { .. } | FieldType::JsonObject => FieldType::String,
            FieldType::List { inner } => FieldType::List {
                inner: Arc::new(text((*inner).clone())),
            },
            FieldType::Set { inner } => FieldType::Set {
                inner: Arc::new(text((*inner).clone())),
            },
            ty => ty,
        }
    }
    FieldType::from_openapi(format).map_or(FieldType::String, text)
}

/// Path and header parameters are sent as text: scalars, unions of them and, in headers, lists
/// (comma-separated) or `content` values (compact JSON).
fn enforce_string_parameter(
    parameter_data: &openapi::ParameterData,
    is_header: bool,
) -> anyhow::Result<()> {
    let s = match &parameter_data.format {
        openapi::ParameterSchemaOrContent::Schema(s) => s,
        openapi::ParameterSchemaOrContent::Content(_) if is_header => return Ok(()),
        openapi::ParameterSchemaOrContent::Content(_) => {
            bail!("found unexpected 'content' data format")
        }
    };
    let Schema::Object(obj) = &s.json_schema else {
        bail!("found unexpected `true` schema");
    };
    if is_text(obj, is_header, 0) {
        return Ok(());
    }
    bail!(
        "only scalar values are supported, not {}",
        describe_schema(obj)
    );
}

fn is_text(obj: &SchemaObject, allow_list: bool, depth: usize) -> bool {
    let scalar = |t: &InstanceType| {
        matches!(
            t,
            InstanceType::String
                | InstanceType::Integer
                | InstanceType::Number
                | InstanceType::Boolean
                | InstanceType::Null
        )
    };
    let variants = obj
        .subschemas
        .as_ref()
        .and_then(|s| s.one_of.as_ref().or(s.any_of.as_ref()));
    match (&obj.instance_type, variants) {
        (Some(SingleOrVec::Single(t)), _) if **t == InstanceType::Array => {
            allow_list
                && depth == 0
                && match obj.array.as_ref().and_then(|a| a.items.as_ref()) {
                    Some(SingleOrVec::Single(item)) => match &**item {
                        Schema::Object(item) => is_text(item, false, depth + 1),
                        Schema::Bool(_) => false,
                    },
                    _ => false,
                }
        }
        (Some(SingleOrVec::Single(t)), _) => scalar(t),
        (Some(SingleOrVec::Vec(types)), _) => types.iter().all(scalar),
        (None, Some(variants)) if depth < 8 => variants.iter().all(|v| match v {
            Schema::Object(v) => is_text(v, allow_list, depth + 1),
            Schema::Bool(_) => false,
        }),
        (None, _) => {
            obj.reference.is_some() || obj.enum_values.is_some() || obj.const_value.is_some()
        }
    }
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
    /// A JSON body that may also come as `text/event-stream`, depending on the request.
    also_event_stream: bool,
    /// Named schema of the JSON `data` of each event of a `text/event-stream` body.
    event_schema_name: Option<String>,
    /// The body may be absent: another success status declares none, or the JSON is `null`.
    may_be_empty: bool,
}

/// The boolean `stream` property of the body schema `name`, which switches the response from
/// JSON to an event stream.
fn stream_property(
    name: &str,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> Option<String> {
    let Schema::Object(obj) = &schemas.get(name)?.json_schema else {
        return None;
    };
    let Schema::Object(prop) = obj.object.as_ref()?.properties.get("stream")? else {
        return None;
    };
    let boolean = SingleOrVec::Single(Box::new(InstanceType::Boolean));
    (prop.instance_type.as_ref() == Some(&boolean)
        || prop.instance_type.as_ref().is_some_and(
            |t| matches!(t, SingleOrVec::Vec(v) if v.contains(&InstanceType::Boolean)),
        ))
    .then(|| "stream".to_owned())
}

/// Picks the body the SDK decodes on success: the lowest 2xx status with content, or `default`
/// when no 2xx is declared. Returns it with the JSON error schemas of 4xx, 5xx and `default`,
/// by status.
fn responses_from_openapi(
    responses: openapi::Responses,
    schemas: &IndexMap<String, openapi::SchemaObject>,
) -> anyhow::Result<(ResponseBody, BTreeMap<String, String>)> {
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
                errors.push((status.to_string(), response))
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
            false => errors.push(("default".to_owned(), default)),
        }
    }
    success.sort_by_key(|(code, ..)| *code);

    let mut chosen: Option<(String, ResponseBody)> = None;
    let mut bodiless = false;
    for (_, status, response) in success {
        let status = match status {
            openapi::StatusCode::Code(0) => "default".to_owned(),
            status => status.to_string(),
        };
        let body = ResponseBody::from_openapi(response, schemas)
            .with_context(|| format!("response `{status}`"))?;
        match &chosen {
            _ if body.kind == ResponseKind::None => bodiless = true,
            None => chosen = Some((status, body)),
            Some((first, kept)) if *kept != body => tracing::warn!(
                "responses `{first}` and `{status}` have different bodies, the SDK decodes `{first}`"
            ),
            Some(_) => {}
        }
    }
    let error_schemas = errors
        .into_iter()
        .filter_map(|(status, response)| {
            let schema = response.content.get("application/json")?.schema.as_ref()?;
            let Schema::Object(obj) = &schema.json_schema else {
                return None;
            };
            Some((status, get_schema_name(obj.reference.as_deref())?))
        })
        .collect();
    let mut response = chosen.map(|(_, body)| body).unwrap_or_default();
    // A success without a body, such as `204`, next to one with a body; the body may also be
    // `null` itself.
    response.may_be_empty |= bodiless && response.kind != ResponseKind::None;
    Ok((response, error_schemas))
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
        let event_stream = content.get("text/event-stream");
        let also_event_stream = event_stream.is_some();
        let event_schema_name = event_stream
            .and_then(|media| media.schema.as_ref())
            .and_then(|schema| match &schema.json_schema {
                Schema::Object(obj) => get_schema_name(obj.reference.as_deref()),
                Schema::Bool(_) => None,
            })
            .filter(|name| schemas.contains_key(name));
        if also_event_stream && !content.contains_key("application/json") {
            return Ok(Self {
                event_schema_name,
                ..kind(ResponseKind::EventStream)
            });
        }
        if let Some(json) = content.get("application/json") {
            // Without a schema, or with the `true` one, any JSON value is accepted.
            let Some(Schema::Object(obj)) = json.schema.as_ref().map(|s| &s.json_schema) else {
                return Ok(Self {
                    kind: ResponseKind::Json,
                    json_type: Some(FieldType::JsonObject),
                    also_event_stream,
                    event_schema_name,
                    ..Self::default()
                });
            };
            let (obj, may_be_empty) = peel_nullable(obj);
            let obj = &obj;
            if let Some((schema_name, is_list)) = named_or_list_of_named(obj, schemas) {
                return Ok(Self {
                    kind: ResponseKind::Json,
                    schema_name: Some(schema_name),
                    is_list,
                    may_be_empty,
                    also_event_stream,
                    event_schema_name,
                    ..Self::default()
                });
            }
            let json_type = FieldType::from_schema_object(obj.clone())?;
            return Ok(Self {
                kind: ResponseKind::Json,
                json_type: Some(json_type),
                may_be_empty,
                also_event_stream,
                event_schema_name,
                ..Self::default()
            });
        }
        if content.keys().any(|k| k.starts_with("text/")) {
            return Ok(kind(ResponseKind::Text));
        }
        Ok(kind(ResponseKind::Binary))
    }
}

/// The schema without its `null` alternative, and whether it had one: `oneOf: [X, {type: null}]`
/// or `type: [X, "null"]`.
fn peel_nullable(obj: &SchemaObject) -> (SchemaObject, bool) {
    let variants = obj
        .subschemas
        .as_ref()
        .and_then(|s| s.one_of.as_ref().or(s.any_of.as_ref()));
    if let Some(variants) = variants
        && let Some(Schema::Object(inner)) = super::types::extract_nullable_variant(variants)
    {
        return (inner.clone(), true);
    }
    let null_type = matches!(
        &obj.instance_type,
        Some(SingleOrVec::Vec(types)) if types.contains(&InstanceType::Null)
    );
    (obj.clone(), null_type)
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
pub(crate) struct HeaderParam {
    /// Name on the wire.
    pub(crate) name: String,
    /// Name the SDK derives its identifier from, unique among the operation's parameters.
    ident: String,
    required: bool,
    /// A cookie parameter: sent in the `Cookie` header, percent-encoded, with the others.
    #[serde(default)]
    cookie: bool,
    /// A `content: application/json` header: sent as compact JSON text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    json: bool,
    /// The type of the value, like a query parameter's: lists are comma-separated, unions are
    /// text.
    #[serde(serialize_with = "serialize_field_type")]
    pub(crate) r#type: FieldType,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct TypedParam {
    pub(crate) name: String,
    #[serde(serialize_with = "serialize_field_type")]
    pub(crate) r#type: FieldType,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct QueryParam {
    /// Name on the wire.
    pub(crate) name: String,
    /// Name the SDK derives its identifier from, unique among the operation's parameters.
    pub(crate) ident: String,
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
    /// `style: deepObject`: lists are sent as `name[]=a&name[]=b`.
    #[serde(default)]
    deep_object: bool,
    /// Sent from its JSON value: objects as `name[key]=value`, nested as deep as they go.
    #[serde(default)]
    pub(crate) structured: bool,
    /// `style: pipeDelimited` or `spaceDelimited` without `explode`: list items are joined by
    /// this delimiter instead of a comma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delimiter: Option<String>,
    /// A `content: application/json` parameter: sent as compact JSON text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    json: bool,
    /// The union `type` was before operations untyped it, for SDKs that type such parameters.
    #[serde(
        default,
        serialize_with = "serialize_optional_field_type",
        skip_serializing_if = "Option::is_none"
    )]
    typed_union: Option<FieldType>,
}

/// How a path parameter is serialized, when it is not plain `simple` text.
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct PathStyle {
    /// `simple`, `label`, `matrix`, or `json` for a `content: application/json` parameter.
    style: String,
    /// Whether lists and objects are exploded: `a.b.c` instead of `a,b,c`.
    explode: bool,
    /// The type of the value; plain text has none.
    #[serde(
        default,
        serialize_with = "serialize_optional_field_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) r#type: Option<FieldType>,
}

fn default_explode() -> bool {
    true
}

/// Declares the `{variable}`s of a path template that no `in: path` parameter describes, as
/// required strings, so that they are filled in instead of sent as the literal `{variable}`.
fn add_undeclared_path_params(path: &str, path_params: &mut Vec<String>) {
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + len];
        if !name.is_empty() && !path_params.iter().any(|p| p == name) {
            tracing::warn!(path, name, "undeclared path parameter, typed as a string");
            path_params.push(name.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
}

/// Gives every parameter an identifier unique within the operation. Path parameters keep their
/// name, query parameters then headers and cookies that collide with an earlier one get an
/// `_query`, `_header` or `_cookie` suffix. The wire names are untouched.
fn disambiguate_parameters(
    path_params: &[String],
    query_params: &mut [QueryParam],
    header_params: &mut [HeaderParam],
) {
    let mut taken: BTreeSet<String> = path_params.iter().map(|p| p.to_snake_case()).collect();
    let mut claim = |name: &str, suffix: &str| -> String {
        let mut ident = name.to_owned();
        let mut n = 1;
        while !taken.insert(ident.to_snake_case()) {
            n += 1;
            ident = if n == 2 {
                format!("{name}_{suffix}")
            } else {
                format!("{name}_{suffix}_{}", n - 1)
            };
        }
        ident
    };
    for p in query_params {
        p.ident = claim(&p.name, "query");
    }
    for p in header_params {
        p.ident = claim(&p.name, if p.cookie { "cookie" } else { "header" });
    }
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
        let fields = multipart_fields(schema, &schemas, &IndexMap::new()).unwrap();
        assert!(fields[0].is_file);
        assert_eq!(fields[0].field.name, "file");
    }

    #[test]
    fn multipart_lists_of_files_are_repeated_parts_and_encodings_set_content_types() {
        let schema = serde_json::from_value(json!({"type":"object", "properties":{
            "files":{"type":"array","items":{"type":"string","format":"binary"}},
            "avatar":{"type":"string","format":"binary"},
            "meta":{"type":"object"},
            "many":{"type":"string","format":"binary"},
        }}))
        .unwrap();
        let encodings = serde_json::from_value(json!({
            "files": {"contentType": "image/png"},
            "meta": {"contentType": "application/vnd.meta+json"},
            "many": {"contentType": "image/png, image/jpeg"},
        }))
        .unwrap();
        let fields = multipart_fields(schema, &IndexMap::new(), &encodings).unwrap();
        let by_name = |n: &str| fields.iter().find(|f| f.field.name == n).unwrap();
        let files = by_name("files");
        assert!(files.is_file && files.is_file_list);
        assert!(matches!(files.field.r#type, FieldType::List { .. }));
        assert_eq!(files.content_type.as_deref(), Some("image/png"));
        assert!(by_name("avatar").is_file && !by_name("avatar").is_file_list);
        assert_eq!(
            by_name("meta").content_type.as_deref(),
            Some("application/vnd.meta+json")
        );
        assert_eq!(by_name("many").content_type, None);
        let schema = serde_json::from_value(json!({"type":"object", "properties":{"tags":{"type":"array","items":{"type":"string"}}}})).unwrap();
        let fields = multipart_fields(schema, &IndexMap::new(), &IndexMap::new()).unwrap();
        assert!(!fields[0].is_file);
        assert!(matches!(fields[0].field.r#type, FieldType::List { .. }));
    }

    #[test]
    fn multipart_cyclic_references_fail_without_recursing() {
        let schemas =
            serde_json::from_value(json!({"Cycle": {"$ref":"#/components/schemas/Cycle"}}))
                .unwrap();
        let schema = serde_json::from_value(json!({"$ref":"#/components/schemas/Cycle"})).unwrap();
        assert!(
            multipart_fields(schema, &schemas, &IndexMap::new())
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

    fn responses(value: Value) -> anyhow::Result<(ResponseBody, BTreeMap<String, String>)> {
        let schemas = schemas(json!({
            "Widgets": { "type": "array", "items": { "$ref": "#/components/schemas/Widget" } }
        }));
        responses_from_openapi(serde_json::from_value(value).unwrap(), &schemas)
    }

    fn json_body(schema: Value) -> Value {
        json!({ "description": "", "content": { "application/json": { "schema": schema } } })
    }

    #[test]
    fn operation_docs_start_with_the_summary() {
        let doc = |summary: Option<&str>, description: Option<&str>| {
            operation_doc(summary.map(Into::into), description.map(Into::into))
        };
        assert_eq!(doc(Some("List pets"), None).as_deref(), Some("List pets"));
        assert_eq!(
            doc(Some("List pets."), Some("Newest first.")).as_deref(),
            Some("List pets.\n\nNewest first.")
        );
        assert_eq!(
            doc(Some("List pets"), Some("List pets, newest first.")).as_deref(),
            Some("List pets, newest first.")
        );
        assert_eq!(doc(Some(" "), None), None);
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
        assert_eq!(
            errors,
            BTreeMap::from([
                ("default".into(), "Error".into()),
                ("4XX".into(), "Invalid".into())
            ])
        );
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
    fn json_or_event_stream_responses_are_json_with_a_stream_twin() {
        let content =
            json!({ "application/json": { "schema": widget() }, "text/event-stream": {} });
        let (body, _) =
            responses(json!({ "200": { "description": "", "content": content } })).unwrap();
        assert_eq!(body.kind, ResponseKind::Json);
        assert!(body.also_event_stream);
        let op = serde_json::from_value(json!({ "operationId": "chat", "responses": {
            "200": { "description": "", "content": content } } }))
        .unwrap();
        let schemas = schemas(json!({ "Widget": { "type": "object", "properties": {} } }));
        let (_, op) = Operation::from_openapi(
            "/chat",
            "post",
            op,
            &schemas,
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap()
        .unwrap();
        let stream = op.event_stream_variant().unwrap();
        assert_eq!(
            (stream.name.as_str(), stream.id.as_str()),
            ("chat_stream", "chat")
        );
        assert!(stream.response_is_event_stream && stream.response_body_schema_name.is_none());
        assert_eq!(op.response_body_schema_name.as_deref(), Some("Widget"));
    }

    #[test]
    fn stream_twins_type_their_events_and_ask_for_the_stream() {
        let op = serde_json::from_value(json!({
            "operationId": "chat",
            "requestBody": { "content": { "application/json": { "schema": {
                "$ref": "#/components/schemas/ChatRequest" } } } },
            "responses": { "200": { "description": "", "content": {
                "application/json": { "schema": widget() },
                "text/event-stream": { "schema": { "$ref": "#/components/schemas/Chunk" } },
            } } },
        }))
        .unwrap();
        let schemas = schemas(json!({
            "Widget": { "type": "object", "properties": {} },
            "Chunk": { "type": "object", "properties": {} },
            "ChatRequest": { "type": "object", "properties": { "stream": { "type": "boolean" } } },
        }));
        let (_, op) = Operation::from_openapi(
            "/chat",
            "post",
            op,
            &schemas,
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(op.stream_property, None);
        let stream = op.event_stream_variant().unwrap();
        assert_eq!(stream.event_schema_name.as_deref(), Some("Chunk"));
        assert_eq!(stream.stream_property.as_deref(), Some("stream"));
    }

    #[test]
    fn inlined_event_aliases_keep_their_name_and_type_the_events() {
        let mut op: Operation = serde_json::from_value(json!({
            "id": "events", "name": "events", "method": "get", "path": "/events",
            "deprecated": false, "path_params": [], "path_styles": {}, "typed_path_params": [],
            "header_params": [], "query_params": [], "request_body_all_optional": false,
            "request_body_optional": false, "request_body_kind": "none", "multipart_fields": [],
            "response_is_event_stream": true, "event_schema_name": "Event",
        }))
        .unwrap();
        let union = FieldType::Union {
            variants: vec![],
            mode: Default::default(),
            decode: Default::default(),
            requested: None,
        };
        let aliases = BTreeMap::from([("Event".to_owned(), union.clone())]);
        op.inline_body_aliases(&aliases).unwrap();
        assert_eq!(op.event_schema_name.as_deref(), Some("Event"));
        assert_eq!(op.event_json_type, Some(union));
    }

    #[test]
    fn a_bodiless_success_next_to_a_body_may_leave_it_empty() {
        let json = json!({ "description": "", "content": {
            "application/json": { "schema": widget() } } });
        let (body, _) = responses(json!({ "200": json, "204": { "description": "" } })).unwrap();
        assert!(body.may_be_empty && body.kind == ResponseKind::Json);
        let (body, _) = responses(json!({ "200": json })).unwrap();
        assert!(!body.may_be_empty);
        let (body, _) = responses(json!({ "204": { "description": "" } })).unwrap();
        assert!(!body.may_be_empty);
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

    #[test]
    fn any_json_schema_is_a_body() {
        let json_type = |schema: Value| {
            let (body, _) = responses(json!({ "200": json_body(schema.clone()) })).unwrap();
            let request = request(json!({ "application/json": { "schema": schema } })).unwrap();
            assert_eq!(request.json_type, body.json_type);
            body.json_type
        };
        // No schema: any JSON value.
        let content =
            json!({ "200": { "description": "", "content": { "application/json": {} } } });
        let (body, _) = responses(content).unwrap();
        assert!(matches!(body.json_type, Some(FieldType::JsonObject)));
        let map = json!({ "type": "object", "additionalProperties": widget() });
        assert!(matches!(
            json_type(map),
            Some(FieldType::Map { value_ty }) if matches!(&*value_ty, FieldType::SchemaRef { .. })
        ));
        let nested = json!({ "type": "array", "items": { "type": "array", "items": widget() } });
        assert!(matches!(
            json_type(nested),
            Some(FieldType::List { inner }) if matches!(&*inner, FieldType::List { .. })
        ));
        assert!(matches!(
            json_type(json!({ "type": "string" })),
            Some(FieldType::String)
        ));
        assert!(matches!(
            json_type(json!({ "type": "integer" })),
            Some(FieldType::Int64)
        ));
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
        let text = request(json!({ "text/plain": { "schema": { "type": "string" } } })).unwrap();
        assert_eq!(
            (text.kind, text.content_type.as_deref()),
            (RequestBodyKind::Binary, Some("text/plain"))
        );
        let raw = request(json!({ "application/octet-stream": {}, "image/png": {} })).unwrap();
        assert_eq!(
            (raw.kind, raw.content_type),
            (RequestBodyKind::Binary, None)
        );
    }

    #[test]
    fn form_encodings_list_deep_objects_and_unexploded_properties() {
        let encoding = json!({
            "expand": { "style": "deepObject", "explode": true },
            "codes": { "style": "form", "explode": false },
            "tags": { "style": "form", "explode": true }
        });
        let body = request(json!({ "application/x-www-form-urlencoded": {
            "schema": widget(), "encoding": encoding
        } }))
        .unwrap();
        assert_eq!(body.kind, RequestBodyKind::Form);
        assert_eq!(body.form_deep_object, ["expand"]);
        assert_eq!(body.form_unexploded, ["codes"]);
    }

    #[test]
    fn tags_starting_with_digits_are_spelled_out() {
        assert_eq!(
            resource_name("1-Click Applications"),
            "one_click_applications"
        );
        assert_eq!(resource_name("2FA"), "two_fa");
        assert_eq!(resource_name("42"), "four_two");
        assert_eq!(resource_name("Pets"), "pets");
        assert_eq!(resource_name("Petite Requête"), "petite_requete");
        assert_eq!(resource_name("OAuth Apps"), "oauth_apps");
        assert_eq!(resource_name("IPAddresses"), "ip_addresses");
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
        resource.disambiguate_operation_names();
        let names: Vec<_> = resource
            .operations
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert_eq!(names, ["repos/get", "gists/get", "list"]);
    }

    #[test]
    fn unsupported_parameters_name_the_parameter() {
        let op = serde_json::from_value(json!({
            "operationId": "op",
            "parameters": [{ "name": "ids", "in": "query", "content": { "text/csv": { "schema": { "type": "string" } } } }]
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
        assert!(error.downcast_ref::<SpecError>().is_none());
        let error = format!("{error:#}");
        assert!(error.starts_with("query parameter `ids`"), "{error}");
    }

    fn parameter(param: Value) -> anyhow::Result<Operation> {
        let op = json!({ "operationId": "op", "parameters": [param] });
        let (_, op) = Operation::from_openapi(
            "/x/{id}",
            "get",
            serde_json::from_value(op).unwrap(),
            &IndexMap::new(),
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )?
        .unwrap();
        Ok(op)
    }

    #[test]
    fn unions_of_scalars_lists_and_content_are_text_parameters() {
        let id = json!({ "name": "id", "in": "path", "required": true,
            "schema": { "anyOf": [{ "type": "integer" }, { "type": "string" }] } });
        let op = parameter(id).unwrap();
        assert_eq!(op.path_params, ["id"]);
        assert!(matches!(
            op.typed_path_params[0].r#type,
            FieldType::Union { .. }
        ));
        let list = json!({ "name": "beta", "in": "header",
            "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(list).unwrap();
        assert_eq!(op.header_params[0].name, "beta");
        assert!(matches!(op.header_params[0].r#type, FieldType::List { .. }));
        let filter = json!({ "name": "X-Filter", "in": "header",
            "content": { "application/json": { "schema": { "type": "object" } } } });
        let op = parameter(filter).unwrap();
        assert_eq!(op.header_params[0].name, "X-Filter");
        assert!(op.header_params[0].json);
        let either = json!({ "name": "X-Either", "in": "header",
            "schema": { "anyOf": [{ "type": "integer" }, { "type": "string" }] } });
        let op = parameter(either).unwrap();
        assert!(matches!(op.header_params[0].r#type, FieldType::String));
        let limit = json!({ "name": "session", "in": "cookie", "schema": { "type": "integer" } });
        let op = parameter(limit).unwrap();
        assert!(op.header_params[0].cookie);
        assert!(matches!(op.header_params[0].r#type, FieldType::Int64));
        let nested = json!({ "name": "rows", "in": "header", "schema": { "type": "array",
            "items": { "type": "array", "items": { "type": "string" } } } });
        assert!(parameter(nested).is_err());
    }

    #[test]
    fn deep_object_query_parameters_are_structured() {
        let expand = json!({ "name": "expand", "in": "query", "style": "deepObject",
            "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(expand).unwrap();
        let param = &op.query_params[0];
        assert!(param.deep_object && param.structured && param.explode);
        let pipes = json!({ "name": "ids", "in": "query", "style": "pipeDelimited",
            "explode": false, "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(pipes).unwrap();
        let param = &op.query_params[0];
        assert!(param.structured && !param.explode);
        assert_eq!(param.delimiter.as_deref(), Some("|"));
        let spaces = json!({ "name": "ids", "in": "query", "style": "spaceDelimited",
            "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(spaces).unwrap();
        assert_eq!(op.query_params[0].delimiter.as_deref(), Some(" "));
        assert!(op.query_params[0].structured && !op.query_params[0].explode);
        let exploded = json!({ "name": "ids", "in": "query", "style": "spaceDelimited",
            "explode": true, "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(exploded).unwrap();
        assert!(op.query_params[0].delimiter.is_none() && !op.query_params[0].structured);
        let scalar = json!({ "name": "id", "in": "query", "style": "pipeDelimited",
            "explode": false, "schema": { "type": "string" } });
        assert!(parameter(scalar).is_err());
    }

    #[test]
    fn json_content_query_parameters_are_typed_and_structured() {
        let filter = json!({ "name": "filter", "in": "query",
            "content": { "application/json": { "schema": { "type": "object",
                "properties": { "a": { "type": "string" } } } } } });
        let op = parameter(filter).unwrap();
        assert!(op.query_params[0].json && op.query_params[0].structured);
        assert!(!matches!(op.query_params[0].r#type, FieldType::String));
    }

    #[test]
    fn path_parameters_keep_their_style() {
        let plain = json!({ "name": "id", "in": "path", "required": true,
            "schema": { "type": "string" } });
        assert!(parameter(plain).unwrap().path_styles.is_empty());
        let int = json!({ "name": "id", "in": "path", "required": true,
            "schema": { "type": "integer" } });
        let op = parameter(int).unwrap();
        assert_eq!(op.path_styles["id"].style, "simple");
        assert!(matches!(
            op.path_styles["id"].r#type,
            Some(FieldType::Int64)
        ));
        let union = json!({ "name": "id", "in": "path", "required": true,
            "schema": { "anyOf": [{ "type": "integer" }, { "type": "string" }] } });
        assert!(parameter(union).unwrap().path_styles.is_empty());
        let label = json!({ "name": "id", "in": "path", "required": true, "style": "label",
            "schema": { "type": "string" } });
        let op = parameter(label).unwrap();
        assert_eq!(op.path_styles["id"].style, "label");
        assert!(op.path_styles["id"].r#type.is_none());
        let matrix = json!({ "name": "id", "in": "path", "required": true, "style": "matrix",
            "explode": true, "schema": { "type": "array", "items": { "type": "string" } } });
        let op = parameter(matrix).unwrap();
        let style = &op.path_styles["id"];
        assert!(style.explode && matches!(style.r#type, Some(FieldType::List { .. })));
        let content = json!({ "name": "id", "in": "path", "required": true,
            "content": { "application/json": { "schema": { "type": "object" } } } });
        assert_eq!(parameter(content).unwrap().path_styles["id"].style, "json");
    }

    #[test]
    fn same_name_parameters_get_distinct_identifiers() {
        let op = json!({ "operationId": "op", "parameters": [
            { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } },
            { "name": "id", "in": "query", "schema": { "type": "string" } },
            { "name": "v", "in": "query", "schema": { "type": "string" } },
            { "name": "v", "in": "header", "schema": { "type": "string" } },
            { "name": "X-Id", "in": "header", "schema": { "type": "string" } },
            { "name": "x_id", "in": "query", "schema": { "type": "string" } },
            { "name": "v", "in": "cookie", "schema": { "type": "string" } },
        ] });
        let (_, op) = Operation::from_openapi(
            "/x/{id}",
            "get",
            serde_json::from_value(op).unwrap(),
            &IndexMap::new(),
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap()
        .unwrap();
        let queries: Vec<_> = op
            .query_params
            .iter()
            .map(|p| (p.name.as_str(), p.ident.as_str()))
            .collect();
        assert_eq!(queries, [("id", "id_query"), ("v", "v"), ("x_id", "x_id")]);
        let headers: Vec<_> = op
            .header_params
            .iter()
            .map(|p| (p.name.as_str(), p.ident.as_str()))
            .collect();
        assert_eq!(
            headers,
            [
                ("v", "v_header"),
                ("X-Id", "X-Id_header"),
                ("v", "v_cookie")
            ]
        );
        assert_eq!(
            op.header_params
                .iter()
                .map(|p| p.cookie)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(op.path_params, ["id"]);
    }

    #[test]
    fn undeclared_path_variables_become_string_parameters() {
        let mut params = vec!["a".to_owned()];
        add_undeclared_path_params("/x/{a}/y/{thing}/{thing}", &mut params);
        assert_eq!(params, ["a", "thing"]);
    }

    #[test]
    fn untyped_union_parameters_keep_their_union_for_sdks_that_type_it() {
        let ids = json!({ "name": "ids", "in": "query", "schema": { "oneOf": [
            { "type": "string" }, { "type": "array", "items": { "type": "string" } }] } });
        let mut op = parameter(ids).unwrap();
        op.untype_unions();
        let param = &op.query_params[0];
        assert_eq!(param.r#type, FieldType::JsonObject);
        assert!(matches!(param.typed_union, Some(FieldType::Union { .. })));
        let named = json!({ "name": "state", "in": "query", "schema": { "oneOf": [
            { "$ref": "#/components/schemas/State" }, { "type": "integer" }] } });
        let mut op = parameter(named).unwrap();
        op.untype_unions();
        op.forget_typed_unions_of_unknown_types(&Types::new());
        assert!(op.query_params[0].typed_union.is_none());
    }

    #[test]
    fn union_bodies_keep_their_union_and_get_a_name() {
        let op = json!({
            "operationId": "create_thing",
            "requestBody": { "content": { "application/json": { "schema": { "oneOf": [
                { "type": "string" }, { "type": "array", "items": { "type": "string" } }] } } } },
            "responses": { "200": { "description": "", "content": { "application/json": {
                "schema": { "type": "array", "items": { "oneOf": [
                    { "type": "string" }, { "type": "integer" }] } } } } } }
        });
        let (_, mut op) = Operation::from_openapi(
            "/x",
            "post",
            serde_json::from_value(op).unwrap(),
            &IndexMap::new(),
            IncludeMode::OnlyPublic,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap()
        .unwrap();
        op.untype_unions();
        op.name_body_unions(&BTreeSet::from(["CreateThingResponse".to_owned()]));
        assert!(matches!(
            op.request_body_json_type,
            Some(FieldType::Union { .. })
        ));
        assert_eq!(op.request_body_union.as_deref(), Some("CreateThingRequest"));
        assert!(matches!(
            op.response_body_json_type,
            Some(FieldType::List { .. })
        ));
        assert_eq!(
            op.response_body_union.as_deref(),
            Some("CreateThingResponseBody")
        );
    }
}

#[cfg(test)]
mod optional_response_tests {
    use serde_json::{Value, json};

    use super::*;

    fn pick(value: Value) -> ResponseBody {
        let schemas: IndexMap<String, openapi::SchemaObject> = IndexMap::new();
        responses_from_openapi(serde_json::from_value(value).unwrap(), &schemas)
            .unwrap()
            .0
    }

    fn json_body(schema: Value) -> Value {
        json!({ "description": "", "content": { "application/json": { "schema": schema } } })
    }

    fn item() -> Value {
        json!({ "$ref": "#/components/schemas/Item" })
    }

    #[test]
    fn a_null_alternative_may_leave_the_body_empty() {
        let body = pick(json!({
            "200": json_body(json!({ "oneOf": [item(), { "type": "null" }] }))
        }));
        assert_eq!(body.schema_name.as_deref(), Some("Item"));
        assert!(body.may_be_empty);
        let body = pick(json!({
            "200": json_body(json!({ "anyOf": [{ "type": "null" }, item()] }))
        }));
        assert_eq!(
            (body.schema_name.as_deref(), body.may_be_empty),
            (Some("Item"), true)
        );
        let body = pick(json!({ "200": json_body(json!({ "type": ["string", "null"] })) }));
        assert_eq!(
            (body.json_type, body.may_be_empty),
            (Some(FieldType::String), true)
        );
    }

    #[test]
    fn a_success_without_a_body_next_to_one_with_may_leave_it_empty() {
        let body = pick(json!({
            "200": json_body(item()),
            "204": { "description": "gone" }
        }));
        assert_eq!(body.schema_name.as_deref(), Some("Item"));
        assert!(body.may_be_empty);
    }

    #[test]
    fn only_null_or_bodiless_successes_empty_a_body() {
        assert!(!pick(json!({ "200": json_body(item()) })).may_be_empty);
        let none = pick(json!({ "204": { "description": "" } }));
        assert!(!none.may_be_empty && none.kind == ResponseKind::None);
        let binary = pick(json!({
            "200": { "description": "", "content": { "application/pdf": { "schema": { "type": "string" } } } },
            "204": { "description": "" }
        }));
        // Any body, binary too, may be left out by a bodiless success.
        assert!(binary.may_be_empty);
    }
}
