use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, btree_map},
    sync::Arc,
};

use aide::openapi;
use anyhow::{Context as _, bail, ensure};
use indexmap::IndexMap;
use schemars::schema::{
    InstanceType, ObjectValidation, Schema, SchemaObject, SingleOrVec, SubschemaValidation,
};
use serde::{Deserialize, Serialize};

use crate::spec::IncludeMode;

use heck::ToUpperCamelCase as _;

use super::{
    get_schema_name,
    resources::{self, Resource, Resources},
};

/// Named types referenced by API operations.
///
/// Intermediate representation of (some) `components` from the spec.
pub(crate) type Types = BTreeMap<String, Type>;

pub(crate) fn from_referenced_components(
    res: &Resources,
    schemas: &mut IndexMap<String, openapi::SchemaObject>,
    webhooks: &[String],
    include_mode: IncludeMode,
) -> (Types, Vec<String>) {
    let mut referenced_components: Vec<&str> = match include_mode {
        IncludeMode::OnlyPublic | IncludeMode::PublicAndInternal | IncludeMode::OnlyInternal => {
            webhooks.iter().map(|s| &**s).collect()
        }
        IncludeMode::OnlySpecified => vec![],
    };
    referenced_components.extend(resources::referenced_components(res));

    let mut types = BTreeMap::new();
    let mut errors = Vec::new();
    let mut visited = BTreeSet::new();
    let mut add_type = |schema_name: &str, extra_components: &mut BTreeSet<_>| {
        if !visited.insert(schema_name.to_owned()) {
            return;
        }
        let _span = tracing::warn_span!("schema", name = schema_name).entered();
        let Some(s) = schemas.swap_remove(schema_name) else {
            errors.push(format!(
                "schema `{schema_name}` is referenced but not defined"
            ));
            return;
        };
        let Schema::Object(obj) = s.json_schema else {
            errors.push(format!(
                "schema `{schema_name}`: boolean schemas are not supported"
            ));
            return;
        };
        match Type::from_schema(schema_name.to_owned(), obj) {
            Ok(ty) => {
                extra_components.extend(
                    ty.referenced_components()
                        .into_iter()
                        .filter(|&c| c != schema_name && !types.contains_key(c))
                        .map(ToOwned::to_owned),
                );
                types.insert(schema_name.to_owned(), ty);
            }
            Err(e) => errors.push(format!("schema `{schema_name}`: {e:#}")),
        }
    };

    let mut extra_components: BTreeSet<_> = referenced_components
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    while let Some(c) = extra_components.pop_first() {
        add_type(&c, &mut extra_components);
    }
    // Resolve SchemaRef inner types for string alias detection
    // This allows to_XXX_typename() methods to check if a reference is to a string alias
    resolve_schema_refs(&mut types);

    (types, errors)
}

/// Schemas whose names become the same type name (`a.b` and `AB` are both `AB`).
pub(crate) fn clashing_type_names(types: &Types) -> Vec<String> {
    let mut seen = BTreeMap::new();
    let mut errors = Vec::new();
    for name in types.keys() {
        if let Some(other) = seen.insert(name.to_upper_camel_case(), name) {
            errors.push(format!(
                "schemas `{other}` and `{name}` both become the type `{}`",
                name.to_upper_camel_case()
            ));
        }
    }
    errors
}

/// Types a discriminated union whose variants are not all objects as untyped JSON, since no
/// SDK can tag a list or a scalar with a discriminator property.
pub(crate) fn untag_unions_with_non_object_variants(types: &mut Types) {
    fn is_object(types: &Types, name: &str, depth: usize) -> bool {
        match types.get(name).map(|t| &t.data) {
            Some(TypeData::Struct { .. } | TypeData::StructEnum { .. }) => true,
            Some(TypeData::Alias { target }) if depth < 16 => match &**target {
                FieldType::SchemaRef { name, .. } => is_object(types, name, depth + 1),
                _ => false,
            },
            _ => false,
        }
    }
    let non_objects: BTreeSet<String> = types
        .keys()
        .filter(|name| !is_object(types, name, 0))
        .cloned()
        .collect();
    for (name, ty) in types.iter_mut() {
        let TypeData::StructEnum {
            repr: StructEnumRepr::InternallyTagged { variants },
            ..
        } = &ty.data
        else {
            continue;
        };
        let offending = variants.iter().find_map(|v| match &v.content {
            EnumVariantType::Ref {
                schema_ref: Some(target),
                ..
            } if non_objects.contains(target) => Some(target.clone()),
            _ => None,
        });
        if let Some(target) = offending {
            let _span = tracing::warn_span!("schema", name = %name).entered();
            tracing::warn!(
                "variant `{target}` is not an object, so the union is typed as an untyped JSON value"
            );
            ty.data = TypeData::Alias {
                target: Box::new(FieldType::JsonObject),
            };
        }
    }
}

/// Fields and enum values that would share an identifier in every SDK, such as `type` and
/// `@type`.
pub(crate) fn clashing_identifiers(types: &Types) -> Vec<String> {
    let mut errors = Vec::new();
    let mut check = |names: Vec<String>, case: &str, owner: String| {
        if let Err(e) = crate::template::ident::idents(&names, case, None, &owner) {
            errors.push(e.detail().unwrap_or_default().to_owned());
        }
    };
    for (name, ty) in types {
        let field_names = |fields: &[Field]| fields.iter().map(|f| f.name.clone()).collect();
        match &ty.data {
            TypeData::Struct { fields } => {
                check(field_names(fields), "snake", format!("schema `{name}`"))
            }
            TypeData::StringEnum { values } => {
                check(values.clone(), "pascal", format!("schema `{name}`"))
            }
            TypeData::StructEnum { fields, repr, .. } => {
                check(field_names(fields), "snake", format!("schema `{name}`"));
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                let values = variants.iter().map(|v| v.name.clone()).collect();
                check(values, "pascal", format!("schema `{name}`"));
                for variant in variants {
                    if let EnumVariantType::Struct { fields } = &variant.content {
                        let owner = format!("schema `{name}`, variant `{}`", variant.name);
                        check(field_names(fields), "snake", owner);
                    }
                }
            }
            TypeData::IntegerEnum { .. } | TypeData::StringAlias | TypeData::Alias { .. } => {}
        }
    }
    errors
}

/// Resolve SchemaRef inner types by looking up the referenced type.
/// This allows type conversion methods to detect string aliases and return the appropriate type.
fn resolve_schema_refs(types: &mut Types) {
    let string_alias_names = collect_string_alias_names(types);

    // Update SchemaRef inner fields for string alias references
    for ty in types.values_mut() {
        resolve_schema_refs_in_type(ty, &string_alias_names);
    }
}

/// Collect the names of all string alias types.
pub(crate) fn collect_string_alias_names(types: &Types) -> BTreeSet<String> {
    types
        .iter()
        .filter(|(_, ty)| matches!(ty.data, TypeData::StringAlias))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Resolve a SchemaRef field type if it references a string alias.
/// This is public so it can be used by the resources module.
pub(crate) fn resolve_schema_ref_in_field_type_public(
    field_type: &mut FieldType,
    string_alias_names: &BTreeSet<String>,
) {
    resolve_schema_ref_in_field_type(field_type, string_alias_names);
}

fn resolve_schema_refs_in_type(ty: &mut Type, string_alias_names: &BTreeSet<String>) {
    match &mut ty.data {
        TypeData::Alias { target } => resolve_schema_ref_in_field_type(target, string_alias_names),
        TypeData::Struct { fields } => {
            for field in fields {
                resolve_schema_ref_in_field_type(&mut field.r#type, string_alias_names);
            }
        }
        TypeData::StructEnum { fields, repr, .. } => {
            for field in fields {
                resolve_schema_ref_in_field_type(&mut field.r#type, string_alias_names);
            }
            match repr {
                StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants } => {
                    for variant in variants {
                        if let EnumVariantType::Struct { fields } = &mut variant.content {
                            for field in fields {
                                resolve_schema_ref_in_field_type(
                                    &mut field.r#type,
                                    string_alias_names,
                                );
                            }
                        }
                    }
                }
            }
        }
        TypeData::StringEnum { .. } | TypeData::IntegerEnum { .. } | TypeData::StringAlias => {}
    }
}

fn resolve_schema_ref_in_field_type(
    field_type: &mut FieldType,
    string_alias_names: &BTreeSet<String>,
) {
    match field_type {
        FieldType::SchemaRef { name, inner }
            if string_alias_names.contains(name) && inner.is_none() =>
        {
            *inner = Some(Type {
                name: name.clone(),
                description: None,
                deprecated: false,
                data: TypeData::StringAlias,
            });
        }
        FieldType::List { inner } | FieldType::Set { inner } => {
            resolve_schema_ref_in_field_type(Arc::make_mut(inner), string_alias_names);
        }
        FieldType::Map { value_ty } => {
            resolve_schema_ref_in_field_type(Arc::make_mut(value_ty), string_alias_names);
        }
        _ => {}
    }
}

/// Replaces the embedded `allOf` parts of every struct by their fields, for targets that
/// cannot flatten a nested object when (de)serializing.
pub(crate) fn inline_flattened_fields(types: &mut Types) -> anyhow::Result<()> {
    let snapshot = types.clone();
    for (name, ty) in types.iter_mut() {
        let flat = |fields: &mut Vec<Field>| -> anyhow::Result<()> {
            if fields.iter().any(|f| f.flatten) {
                *fields = flattened(&snapshot, name, fields, &mut BTreeSet::new())?;
            }
            Ok(())
        };
        match &mut ty.data {
            TypeData::Struct { fields } => flat(fields)?,
            TypeData::StructEnum { fields, repr, .. } => {
                flat(fields)?;
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                for variant in variants {
                    if let EnumVariantType::Struct { fields } = &mut variant.content {
                        flat(fields)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `fields` with each embedded part replaced by its own fields; direct fields win.
fn flattened<'a>(
    types: &'a Types,
    owner: &str,
    fields: &'a [Field],
    seen: &mut BTreeSet<&'a str>,
) -> anyhow::Result<Vec<Field>> {
    let mut out: Vec<Field> = Vec::new();
    for field in fields {
        if !field.flatten {
            match out.iter_mut().find(|f| f.name == field.name) {
                Some(inherited) => *inherited = field.clone(),
                None => out.push(field.clone()),
            }
            continue;
        }
        let part = field.r#type.referenced_schema().unwrap_or_default();
        let Some(TypeData::Struct { fields: inner }) = types.get(part).map(|t| &t.data) else {
            bail!(
                "schema `{owner}`: its `allOf` part `{part}` is not an object, which this target cannot embed"
            );
        };
        if !seen.insert(part) {
            continue;
        }
        for inherited in flattened(types, owner, inner, seen)? {
            if !out.iter().any(|f| f.name == inherited.name) {
                out.push(inherited);
            }
        }
    }
    Ok(out)
}

/// Replace every reference to a [`TypeData::Alias`] type by the alias target, for
/// targets whose templates cannot declare named aliases.
pub(crate) fn inline_aliases(types: &mut Types, resources: &mut Resources) -> anyhow::Result<()> {
    let aliases = resolve_alias_targets(types)?;
    if aliases.is_empty() {
        return Ok(());
    }
    for ty in types.values_mut() {
        ty.data
            .for_each_field_type(|field_type| field_type.inline_aliases(&aliases));
        if let TypeData::StructEnum {
            repr:
                StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants },
            ..
        } = &ty.data
        {
            for variant in variants {
                if let EnumVariantType::Ref {
                    schema_ref: Some(name),
                    ..
                } = &variant.content
                {
                    ensure!(
                        !aliases.contains_key(name),
                        "alias schema `{name}` cannot be a union variant"
                    );
                }
            }
        }
    }
    for resource in resources.values_mut() {
        resource.inline_aliases(&aliases)?;
    }
    Ok(())
}

fn resolve_alias_targets(types: &Types) -> anyhow::Result<BTreeMap<String, FieldType>> {
    fn resolve<'a>(
        name: &'a str,
        raw: &BTreeMap<&'a str, &'a FieldType>,
        resolved: &mut BTreeMap<String, FieldType>,
        stack: &mut Vec<&'a str>,
    ) -> anyhow::Result<()> {
        if resolved.contains_key(name) {
            return Ok(());
        }
        ensure!(!stack.contains(&name), "cyclic alias schema `{name}`");
        stack.push(name);
        let source = raw[name];
        if let Some(inner) = source.referenced_schema()
            && raw.contains_key(inner)
        {
            resolve(inner, raw, resolved, stack)?;
        }
        let mut target = source.clone();
        target.inline_aliases(resolved);
        stack.pop();
        resolved.insert(name.to_owned(), target);
        Ok(())
    }

    let raw: BTreeMap<&str, &FieldType> = types
        .iter()
        .filter_map(|(name, ty)| match &ty.data {
            TypeData::Alias { target } => Some((name.as_str(), &**target)),
            _ => None,
        })
        .collect();
    let mut resolved = BTreeMap::new();
    for name in raw.keys() {
        resolve(name, &raw, &mut resolved, &mut Vec::new())?;
    }
    Ok(resolved)
}

/// Promote every inline `FieldType::StringEnum` into a named top-level
/// `TypeData::StringEnum` and rewrite the field to `FieldType::SchemaRef`.
///
/// Name selection:
/// - If an existing top-level string enum has exactly the same values, reuse it.
/// - Else, use the schema `title` if present.
/// - Else, derive from context: `{parent}_{field}` (or `..._item` / `..._value`
///   when promoted through a List/Set/Map), converted to UpperCamelCase.
///
/// After this pass, `FieldType::StringEnum` should not appear anywhere; the render
/// methods type a leftover one as a plain string.
pub(crate) fn promote_inline_enums(
    types: &mut Types,
    resources: &mut Resources,
) -> anyhow::Result<()> {
    // Snapshot existing top-level string enums keyed by their value set, so we
    // can reuse them instead of generating duplicates (e.g. the subscription
    // `statuses` filter reuses the existing `SubscriptionStatusEnum`).
    let existing = ExistingTypes {
        by_values: types
            .iter()
            .filter_map(|(name, ty)| match &ty.data {
                TypeData::StringEnum { values } => Some((values.clone(), name.clone())),
                _ => None,
            })
            .collect(),
        type_names: types.keys().map(|n| n.to_upper_camel_case()).collect(),
    };

    let mut new_types: BTreeMap<String, Type> = BTreeMap::new();

    for (type_name, ty) in types.iter_mut() {
        match &mut ty.data {
            TypeData::Struct { fields } => {
                for field in fields {
                    let base = format!("{}_{}", type_name, field.name);
                    promote_field_type(&mut field.r#type, &base, &existing, &mut new_types)?;
                }
            }
            TypeData::StructEnum { fields, repr, .. } => {
                for field in fields {
                    let base = format!("{}_{}", type_name, field.name);
                    promote_field_type(&mut field.r#type, &base, &existing, &mut new_types)?;
                }
                let variants = match repr {
                    StructEnumRepr::AdjacentlyTagged { variants, .. }
                    | StructEnumRepr::InternallyTagged { variants } => variants,
                };
                for variant in variants {
                    if let EnumVariantType::Struct { fields } = &mut variant.content {
                        for field in fields {
                            let base = format!("{}_{}_{}", type_name, variant.name, field.name);
                            promote_field_type(
                                &mut field.r#type,
                                &base,
                                &existing,
                                &mut new_types,
                            )?;
                        }
                    }
                }
            }
            TypeData::Alias { target } => {
                let base = format!("{type_name}_value");
                promote_field_type(target, &base, &existing, &mut new_types)?;
            }
            TypeData::StringEnum { .. } | TypeData::IntegerEnum { .. } | TypeData::StringAlias => {}
        }
    }

    for resource in resources.values_mut() {
        promote_inline_enums_in_resource(resource, &existing, &mut new_types)?;
    }

    for (name, ty) in new_types {
        match types.entry(name) {
            btree_map::Entry::Vacant(e) => {
                e.insert(ty);
            }
            btree_map::Entry::Occupied(e) => {
                if *e.get() != ty {
                    bail!(
                        "promoted inline enum `{}` collides with an existing type of different shape",
                        e.key()
                    );
                }
            }
        }
    }

    Ok(())
}

fn promote_inline_enums_in_resource(
    resource: &mut Resource,
    existing: &ExistingTypes,
    new_types: &mut BTreeMap<String, Type>,
) -> anyhow::Result<()> {
    for sub in resource.subresources.values_mut() {
        promote_inline_enums_in_resource(sub, existing, new_types)?;
    }
    for op in &mut resource.operations {
        let op_id = op.id.clone();
        for field in &mut op.multipart_fields {
            let base = format!("{}_{}", op_id, field.field.name);
            promote_field_type(&mut field.field.r#type, &base, existing, new_types)?;
        }
        for param in &mut op.query_params {
            let base = format!("{}_{}", op_id, param.name);
            promote_field_type(&mut param.r#type, &base, existing, new_types)?;
        }
    }
    Ok(())
}

struct ExistingTypes {
    /// String enums by their values, reused instead of promoting a copy.
    by_values: BTreeMap<Vec<String>, String>,
    type_names: BTreeSet<String>,
}

fn promote_field_type(
    ft: &mut FieldType,
    base_name: &str,
    existing: &ExistingTypes,
    new_types: &mut BTreeMap<String, Type>,
) -> anyhow::Result<()> {
    match ft {
        FieldType::StringEnum { values, title } => {
            let values = std::mem::take(values);
            if let Some(existing_name) = existing.by_values.get(&values) {
                *ft = FieldType::SchemaRef {
                    name: existing_name.clone(),
                    inner: None,
                };
                return Ok(());
            }
            let base = match title.take() {
                Some(t) => t.to_upper_camel_case(),
                None => base_name.to_upper_camel_case(),
            };
            let data = TypeData::StringEnum { values };
            let mut name = base.clone();
            for n in 2.. {
                match new_types.get(&name) {
                    Some(promoted) if promoted.data == data => break,
                    None if !existing.type_names.contains(&name) => break,
                    _ => name = format!("{base}{n}"),
                }
            }
            new_types.entry(name.clone()).or_insert_with(|| Type {
                name: name.clone(),
                description: None,
                deprecated: false,
                data,
            });
            *ft = FieldType::SchemaRef { name, inner: None };
        }
        FieldType::List { inner } | FieldType::Set { inner } => {
            promote_field_type(
                Arc::make_mut(inner),
                &format!("{base_name}_item"),
                existing,
                new_types,
            )?;
        }
        FieldType::Map { value_ty } => {
            promote_field_type(
                Arc::make_mut(value_ty),
                &format!("{base_name}_value"),
                existing,
                new_types,
            )?;
        }
        FieldType::Bool
        | FieldType::Int16
        | FieldType::UInt16
        | FieldType::Int32
        | FieldType::Int64
        | FieldType::UInt64
        | FieldType::Float
        | FieldType::Double
        | FieldType::String
        | FieldType::Decimal
        | FieldType::DateTime
        | FieldType::Uri
        | FieldType::JsonObject
        | FieldType::SchemaRef { .. }
        | FieldType::Date => {}
    }
    Ok(())
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Type {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    deprecated: bool,
    #[serde(flatten)]
    pub data: TypeData,
}

fn has_open_additional_properties(obj: &ObjectValidation) -> bool {
    obj.additional_properties
        .as_deref()
        .is_some_and(|schema| *schema != Schema::Bool(false))
}

impl Type {
    pub(crate) fn from_schema(name: String, s: SchemaObject) -> anyhow::Result<Self> {
        let metadata = s.metadata.clone().unwrap_or_default();
        let ty = |data| Self {
            name: name.clone(),
            description: metadata.description.clone(),
            deprecated: metadata.deprecated,
            data,
        };
        let alias = |s: SchemaObject| -> anyhow::Result<Self> {
            Ok(ty(match FieldType::from_schema_object(s)? {
                FieldType::StringEnum { values, .. } => TypeData::StringEnum { values },
                target => TypeData::Alias {
                    target: Box::new(target),
                },
            }))
        };

        if s.reference.is_some() {
            return alias(s);
        }
        if let Some(subschemas) = &s.subschemas {
            if let Some(all_of) = &subschemas.all_of {
                let object = s.object.clone().map(|o| *o).unwrap_or_default();
                return Ok(ty(TypeData::from_all_of(all_of, object)?));
            }
            if let Some(one_of) = &subschemas.one_of
                && let Some(discriminator) = s.extensions.get("discriminator")
            {
                return Ok(ty(TypeData::from_discriminated_oneof(
                    one_of,
                    discriminator,
                    s.object.as_deref(),
                )?));
            }
            let is_struct_enum = subschemas.one_of.is_some()
                && s.object.as_ref().is_some_and(|o| !o.properties.is_empty());
            if (subschemas.one_of.is_some() || subschemas.any_of.is_some()) && !is_struct_enum {
                return alias(s);
            }
        }

        let effective_type = match &s.instance_type {
            Some(SingleOrVec::Vec(types)) => match extract_non_null_type(types) {
                Ok(t) => t,
                Err(_) => return alias(s),
            },
            Some(SingleOrVec::Single(t)) => Some(**t),
            None if s.enum_values.is_some() => Some(InstanceType::String),
            None if s.subschemas.is_some() => Some(InstanceType::Object),
            None => match implied_type(&s) {
                Some(implied) => Some(implied),
                None => return alias(s),
            },
        };

        let data = match effective_type {
            Some(InstanceType::Object) => {
                let obj = s.object.clone().unwrap_or_default();
                if obj.properties.is_empty() && has_open_additional_properties(&obj) {
                    return alias(s);
                }
                TypeData::from_object_schema(*obj, s.subschemas)?
            }
            Some(InstanceType::Integer) if s.enum_values.is_some() => {
                let values = s.enum_values.unwrap_or_default();
                let names = ["x-enum-varnames", "x-enumNames"]
                    .into_iter()
                    .find_map(|key| s.extensions.get(key))
                    .map(|names| {
                        names
                            .as_array()
                            .context("integer enum varnames should be a list")?
                            .iter()
                            .map(|n| n.as_str().map(ToOwned::to_owned))
                            .collect::<Option<Vec<_>>>()
                            .context("integer enum varnames should be strings")
                    })
                    .transpose()?;
                TypeData::from_integer_enum(values, names)?
            }
            Some(InstanceType::String) => match s.enum_values {
                Some(values) => TypeData::from_string_enum(values)?,
                None => TypeData::StringAlias,
            },
            Some(_) => return alias(s),
            None => bail!("a schema with only `null` values is not supported"),
        };

        Ok(ty(data))
    }

    pub(crate) fn referenced_components(&self) -> BTreeSet<&str> {
        match &self.data {
            TypeData::Struct { fields } => fields_referenced_schemas(fields),
            TypeData::StringEnum { .. } => BTreeSet::new(),
            TypeData::IntegerEnum { .. } => BTreeSet::new(),
            TypeData::StringAlias => BTreeSet::new(),
            TypeData::Alias { target } => target.referenced_schema().into_iter().collect(),
            TypeData::StructEnum { repr, fields, .. } => {
                let mut res = repr.referenced_components();
                res.append(&mut fields_referenced_schemas(fields));
                res
            }
        }
    }

    /// The wire names of the fields a struct gets from its `allOf` parts.
    pub(crate) fn inherited_fields<'a>(&'a self, types: &'a Types) -> BTreeSet<&'a str> {
        let mut fields = BTreeSet::new();
        let mut stack = vec![self];
        let mut seen = BTreeSet::new();
        while let Some(ty) = stack.pop() {
            let TypeData::Struct { fields: own } = &ty.data else {
                continue;
            };
            for base in own.iter().filter(|f| f.flatten) {
                let Some(name) = base.r#type.referenced_schema() else {
                    continue;
                };
                if let Some(
                    base @ Type {
                        data: TypeData::Struct { fields: inherited },
                        ..
                    },
                ) = types.get(name)
                {
                    fields.extend(
                        inherited
                            .iter()
                            .filter(|f| !f.flatten)
                            .map(|f| f.name.as_str()),
                    );
                    if seen.insert(name) {
                        stack.push(base);
                    }
                }
            }
        }
        fields
    }

    /// Schemas this type holds by value rather than behind a list or map.
    pub(crate) fn direct_refs(&self) -> BTreeSet<&str> {
        fn direct(ty: &FieldType) -> Option<&str> {
            match ty {
                FieldType::SchemaRef { name, .. } => Some(name),
                _ => None,
            }
        }
        fn fields(fields: &[Field]) -> BTreeSet<&str> {
            fields.iter().filter_map(|f| direct(&f.r#type)).collect()
        }
        match &self.data {
            TypeData::Struct { fields: f } => fields(f),
            TypeData::Alias { target } => direct(target).into_iter().collect(),
            TypeData::StructEnum {
                repr, fields: f, ..
            } => {
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                let mut refs = fields(f);
                for variant in variants {
                    match &variant.content {
                        EnumVariantType::Struct { fields: f } => refs.extend(fields(f)),
                        EnumVariantType::Ref { schema_ref, .. } => {
                            refs.extend(schema_ref.as_deref())
                        }
                    }
                }
                refs
            }
            TypeData::StringEnum { .. } | TypeData::IntegerEnum { .. } | TypeData::StringAlias => {
                BTreeSet::new()
            }
        }
    }
}

fn fields_referenced_schemas(fields: &[Field]) -> BTreeSet<&str> {
    fields
        .iter()
        .filter_map(|f| f.r#type.referenced_schema())
        .collect()
}

/// Extract the non-null type from an OpenAPI 3.1 type array like ["string", "null"].
/// Returns None if the array only contains null or is empty.
fn extract_non_null_type(types: &[InstanceType]) -> anyhow::Result<Option<InstanceType>> {
    let non_null_types: Vec<_> = types.iter().filter(|t| **t != InstanceType::Null).collect();

    match non_null_types.len() {
        0 => Ok(None),
        1 => Ok(Some(*non_null_types[0])),
        _ => bail!("unsupported: multiple non-null types in type array: {types:?}"),
    }
}

fn is_null_schema(schema: &Schema) -> bool {
    matches!(schema, Schema::Object(obj) if obj.instance_type == Some(InstanceType::Null.into()))
}

/// `X` in the `oneOf`/`anyOf: [X, {type: null}]` nullable pattern, in either order.
fn extract_nullable_variant(variants: &[Schema]) -> Option<&Schema> {
    match variants {
        [a, b] if is_null_schema(a) && !is_null_schema(b) => Some(b),
        [a, b] if is_null_schema(b) && !is_null_schema(a) => Some(a),
        _ => None,
    }
}

/// The type implied by validation keywords when `type` is absent.
fn implied_type(obj: &SchemaObject) -> Option<InstanceType> {
    if obj.reference.is_some() || obj.const_value.is_some() {
        None
    } else if obj.array.is_some() {
        Some(InstanceType::Array)
    } else if obj.object.is_some() {
        Some(InstanceType::Object)
    } else if obj.enum_values.is_some() || obj.string.is_some() || obj.format.is_some() {
        Some(InstanceType::String)
    } else {
        None
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TypeData {
    Struct {
        fields: Vec<Field>,
    },
    StringEnum {
        values: Vec<String>,
    },
    IntegerEnum {
        variants: Vec<(String, i64)>,
    },
    /// A type alias to String (for ID types like CustomerId, InvoiceId, etc.)
    StringAlias,
    Alias {
        #[serde(serialize_with = "serialize_field_type")]
        target: Box<FieldType>,
    },
    StructEnum {
        /// Name of the field that identifies the variant.
        discriminator_field: String,

        /// JSON representation of the enum variants.
        #[serde(flatten)]
        repr: StructEnumRepr,

        /// Variant-independent fields.
        fields: Vec<Field>,
    },
}

impl TypeData {
    fn for_each_field_type(&mut self, mut visit: impl FnMut(&mut FieldType)) {
        match self {
            Self::Alias { target } => visit(target),
            Self::Struct { fields } => fields.iter_mut().for_each(|f| visit(&mut f.r#type)),
            Self::StructEnum { fields, repr, .. } => {
                fields.iter_mut().for_each(|f| visit(&mut f.r#type));
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                for variant in variants {
                    if let EnumVariantType::Struct { fields } = &mut variant.content {
                        fields.iter_mut().for_each(|f| visit(&mut f.r#type));
                    }
                }
            }
            Self::StringEnum { .. } | Self::IntegerEnum { .. } | Self::StringAlias => {}
        }
    }

    pub(super) fn from_object_schema(
        obj: ObjectValidation,
        subschemas: Option<Box<SubschemaValidation>>,
    ) -> anyhow::Result<Self> {
        if has_open_additional_properties(&obj) || !obj.pattern_properties.is_empty() {
            tracing::warn!("properties beyond the declared ones are dropped when decoding");
        }

        let fields: Vec<_> = obj
            .properties
            .into_iter()
            .map(|(name, schema)| {
                Field::from_schema(name.clone(), schema, obj.required.contains(&name))
                    .with_context(|| format!("unsupported field `{name}`"))
            })
            .collect::<anyhow::Result<_>>()?;

        // `not` and `if`/`then`/`else` only constrain values, they do not change the type.
        if let Some(sub) = subschemas {
            ensure!(
                sub.all_of.is_none(),
                "`allOf` is only supported over object schemas"
            );
            if let Some(one_of) = sub.one_of {
                match Self::inline_struct_enum(&one_of, &fields) {
                    Ok(data) => return Ok(data),
                    Err(e) => tracing::warn!("`oneOf` next to properties is ignored: {e:#}"),
                }
            }
            if sub.any_of.is_some() {
                tracing::warn!("`anyOf` next to properties is ignored");
            }
        }

        Ok(Self::Struct { fields })
    }

    /// A struct embedding the referenced `allOf` parts, which the spec normalization leaves
    /// after merging the inline ones into `object`.
    fn from_all_of(all_of: &[Schema], object: ObjectValidation) -> anyhow::Result<Self> {
        let mut fields = Vec::new();
        for part in all_of {
            let Schema::Object(SchemaObject {
                reference: Some(reference),
                ..
            }) = part
            else {
                bail!("`allOf` is only supported over object schemas");
            };
            let name = get_schema_name(Some(reference)).context("invalid $ref in allOf")?;
            fields.push(Field {
                name: format!("__flatten_{}", name.to_lowercase()),
                r#type: FieldType::SchemaRef { name, inner: None },
                default: None,
                description: None,
                required: true,
                nullable: false,
                deprecated: false,
                example: None,
                read_only: false,
                write_only: false,
                flatten: true,
            });
        }
        let Self::Struct { fields: own } = Self::from_object_schema(object, None)? else {
            unreachable!("objects without subschemas are structs")
        };
        fields.extend(own);
        Ok(Self::Struct { fields })
    }

    /// Parse a oneOf schema with a discriminator - creates a struct enum
    fn from_discriminated_oneof(
        one_of: &[Schema],
        discriminator: &serde_json::Value,
        shared: Option<&ObjectValidation>,
    ) -> anyhow::Result<Self> {
        let discriminator_obj = discriminator
            .as_object()
            .context("discriminator should be an object")?;

        let property_name = discriminator_obj
            .get("propertyName")
            .and_then(|v| v.as_str())
            .context("discriminator.propertyName is required")?;

        // Mapping targets are `#/components/schemas/X` references or bare `X` names.
        let mapping: Vec<(&String, &str)> = discriminator_obj
            .get("mapping")
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
            .filter_map(|(key, target)| Some((key, target.as_str()?.rsplit('/').next()?)))
            .collect();

        let mut variants = Vec::new();

        for schema in one_of {
            match schema {
                Schema::Object(obj) => {
                    if let Some(ref_str) = &obj.reference {
                        let schema_name =
                            get_schema_name(Some(ref_str)).context("invalid $ref in oneOf")?;
                        let mut values: Vec<String> = mapping
                            .iter()
                            .filter(|(_, target)| *target == schema_name)
                            .map(|(key, _)| (*key).clone())
                            .collect();
                        if values.is_empty() {
                            values.push(schema_name.clone());
                        }
                        for value in values {
                            variants.push(SimpleVariant {
                                name: value,
                                content: EnumVariantType::Ref {
                                    schema_ref: Some(schema_name.clone()),
                                    inner: None,
                                },
                            });
                        }
                    } else if let Some(ref obj_validation) = obj.object {
                        // Inline schema - try to extract the discriminator value from properties
                        let discriminator_value = obj_validation
                            .properties
                            .get(property_name)
                            .and_then(|s| {
                                if let Schema::Object(disc_obj) = s {
                                    disc_obj
                                        .const_value
                                        .as_ref()
                                        .or_else(|| disc_obj.enum_values.as_ref()?.first())
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())
                                } else {
                                    None
                                }
                            })
                            .context("inline schema in oneOf must have discriminator enum value")?;

                        // Parse fields from the inline schema (excluding the discriminator)
                        let fields: Vec<_> = obj_validation
                            .properties
                            .iter()
                            .filter(|(name, _)| *name != property_name)
                            .map(|(name, prop_schema)| {
                                Field::from_schema(
                                    name.clone(),
                                    prop_schema.clone(),
                                    obj_validation.required.contains(name),
                                )
                                .with_context(|| {
                                    format!("unsupported field `{name}` in inline oneOf schema")
                                })
                            })
                            .collect::<anyhow::Result<_>>()?;

                        variants.push(SimpleVariant {
                            name: discriminator_value,
                            content: EnumVariantType::Struct { fields },
                        });
                    } else {
                        bail!("oneOf variant must have either $ref or object properties");
                    }
                }
                Schema::Bool(_) => bail!("unsupported bool schema in oneOf"),
            }
        }

        let fields = shared
            .into_iter()
            .flat_map(|object| {
                object
                    .properties
                    .iter()
                    .filter(|(name, _)| *name != property_name)
                    .map(|(name, schema)| {
                        Field::from_schema(
                            name.clone(),
                            schema.clone(),
                            object.required.contains(name),
                        )
                        .with_context(|| format!("unsupported shared field `{name}`"))
                    })
            })
            .collect::<anyhow::Result<_>>()?;

        Ok(Self::StructEnum {
            discriminator_field: property_name.to_string(),
            repr: StructEnumRepr::InternallyTagged { variants },
            fields,
        })
    }

    fn from_string_enum(values: Vec<serde_json::Value>) -> anyhow::Result<TypeData> {
        Ok(Self::StringEnum {
            values: values
                .into_iter()
                .enumerate()
                .filter(|(_, v)| !v.is_null())
                .map(|(i, v)| match v {
                    serde_json::Value::String(s) => Ok(s),
                    _ => bail!("enum value {} is not a string", i + 1),
                })
                .collect::<anyhow::Result<_>>()?,
        })
    }

    /// Variant names come from `x-enum-varnames`/`x-enumNames`, or else from the values.
    fn from_integer_enum(
        values: Vec<serde_json::Value>,
        names: Option<Vec<String>>,
    ) -> anyhow::Result<TypeData> {
        if let Some(names) = &names {
            ensure!(
                names.len() == values.len(),
                "{} enum varnames for {} values",
                names.len(),
                values.len()
            );
        }
        let variants = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let value = v
                    .as_i64()
                    .with_context(|| format!("enum value {v} is not an integer"))?;
                let name = match &names {
                    Some(names) => names[i].clone(),
                    None if value < 0 => format!("Minus{}", value.unsigned_abs()),
                    None => format!("Value{value}"),
                };
                Ok((name, value))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self::IntegerEnum { variants })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "repr", rename_all = "snake_case")]
pub(crate) enum StructEnumRepr {
    // add more variants here to support other enum representations
    AdjacentlyTagged {
        /// Name of the field that contains the variant-specific fields.
        content_field: String,

        /// Enum variants.
        ///
        /// Every variant has a discriminator value that's stored in the discriminator field to
        /// identify the variant.
        variants: Vec<SimpleVariant>,
    },
    /// Internally tagged enum - the discriminator is a field inside each variant
    /// Used for OpenAPI discriminated oneOf patterns like Fee with fee_type discriminator
    InternallyTagged {
        /// Enum variants.
        variants: Vec<SimpleVariant>,
    },
}

impl StructEnumRepr {
    fn referenced_components(&self) -> BTreeSet<&str> {
        let variants = match self {
            StructEnumRepr::AdjacentlyTagged { variants, .. } => variants,
            StructEnumRepr::InternallyTagged { variants } => variants,
        };
        variants
            .iter()
            .flat_map(|v| match &v.content {
                EnumVariantType::Struct { fields } => fields_referenced_schemas(fields),
                EnumVariantType::Ref { schema_ref, .. } => {
                    schema_ref.as_deref().into_iter().collect()
                }
            })
            .collect()
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub(crate) name: String,
    #[serde(serialize_with = "serialize_field_type")]
    pub r#type: FieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    required: bool,
    nullable: bool,
    deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    example: Option<serde_json::Value>,
    /// Only sent by the server (`readOnly`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    read_only: bool,
    /// Only sent by the client (`writeOnly`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    write_only: bool,
    /// An embedded `allOf` part, whose fields are the struct's own on the wire.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    flatten: bool,
}

impl Field {
    pub(crate) fn from_schema(name: String, s: Schema, required: bool) -> anyhow::Result<Self> {
        let _span = tracing::warn_span!("field", name = %name).entered();
        let obj = match s {
            Schema::Bool(_) => bail!("unsupported bool schema"),
            Schema::Object(o) => o,
        };
        let example = obj.extensions.get("example").cloned();
        let metadata = obj.metadata.clone().unwrap_or_default();

        // Check for OpenAPI 3.0 style nullable extension
        let mut nullable = obj
            .extensions
            .get("nullable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Handle OpenAPI 3.1 oneOf nullable pattern: oneOf: [{type: null}, {actual type}]
        let (field_type, is_oneof_nullable) = FieldType::from_schema_object_with_nullable(obj)?;
        nullable = nullable || is_oneof_nullable;

        Ok(Self {
            name,
            r#type: field_type,
            default: metadata.default,
            description: metadata.description,
            required,
            nullable,
            deprecated: metadata.deprecated,
            example,
            read_only: metadata.read_only,
            write_only: metadata.write_only,
            flatten: false,
        })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum EnumVariantType {
    Struct {
        fields: Vec<Field>,
    },
    Ref {
        #[serde(skip_serializing_if = "Option::is_none")]
        schema_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<Type>,
    },
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub(crate) struct SimpleVariant {
    /// Discriminator value that identifies this variant.
    pub name: String,
    #[serde(flatten)]
    pub content: EnumVariantType,
}

/// Supported field type.
///
/// Equivalent to openapi's `type` + `format` + `$ref`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "id")]
pub(crate) enum FieldType {
    Bool,
    Int16,
    UInt16,
    Int32,
    Int64,
    UInt64,
    Float,
    Double,
    String,
    Decimal,
    DateTime,
    /// A calendar date (`format: date`), typed as a string until SDKs map it to a date type.
    Date,
    Uri,
    /// A JSON object with arbitrary field values.
    JsonObject,
    /// A regular old list.
    List {
        inner: Arc<FieldType>,
    },
    /// List with unique items.
    Set {
        inner: Arc<FieldType>,
    },
    /// A map with a given value type.
    ///
    /// The key type is always `String` in JSON schemas.
    Map {
        value_ty: Arc<FieldType>,
    },
    /// The name of another schema that defines this type.
    SchemaRef {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<Type>,
    },

    /// An inline string enum that will be promoted to a named top-level type
    /// by `promote_inline_enums`. Should never reach the render stage.
    StringEnum {
        values: Vec<String>,
        /// Title from the OpenAPI schema, used as the promoted type name when set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

impl FieldType {
    pub(crate) fn from_openapi(format: openapi::ParameterSchemaOrContent) -> anyhow::Result<Self> {
        let openapi::ParameterSchemaOrContent::Schema(s) = format else {
            bail!("found unexpected 'content' data format");
        };
        Self::from_schema(s.json_schema)
    }

    fn from_schema(s: Schema) -> anyhow::Result<Self> {
        let Schema::Object(obj) = s else {
            bail!("found unexpected `true` schema");
        };

        Self::from_schema_object(obj)
    }

    pub(crate) fn from_schema_object(obj: SchemaObject) -> anyhow::Result<Self> {
        let (field_type, _nullable) = Self::from_schema_object_with_nullable(obj)?;
        Ok(field_type)
    }

    /// Parse a schema object, returning the field type and whether it's nullable.
    /// Handles OpenAPI 3.1 patterns like type arrays and unions with null.
    fn from_schema_object_with_nullable(obj: SchemaObject) -> anyhow::Result<(Self, bool)> {
        if let Some(subschemas) = &obj.subschemas {
            if let Some(all_of) = &subschemas.all_of {
                ensure!(
                    all_of.len() == 1,
                    "`allOf` is only supported over object schemas"
                );
                let Schema::Object(inner) = all_of[0].clone() else {
                    bail!("boolean schemas are not supported");
                };
                return Self::from_schema_object_with_nullable(inner);
            }
            if let Some(variants) = subschemas.one_of.as_ref().or(subschemas.any_of.as_ref()) {
                if let Some(inner) = extract_nullable_variant(variants) {
                    let Schema::Object(inner) = inner.clone() else {
                        bail!("boolean schemas are not supported");
                    };
                    let (field_type, _) = Self::from_schema_object_with_nullable(inner)?;
                    return Ok((field_type, true));
                }
                tracing::warn!(
                    "`oneOf`/`anyOf` without a discriminator is typed as an untyped JSON value"
                );
                return Ok((Self::JsonObject, variants.iter().any(is_null_schema)));
            }
        }

        let (effective_type, nullable) = match &obj.instance_type {
            Some(SingleOrVec::Vec(types)) => {
                let has_null = types.contains(&InstanceType::Null);
                match extract_non_null_type(types) {
                    Ok(non_null) => (non_null, has_null),
                    Err(_) => {
                        tracing::warn!(
                            "a value of several types is typed as an untyped JSON value"
                        );
                        return Ok((Self::JsonObject, has_null));
                    }
                }
            }
            Some(SingleOrVec::Single(t)) => (Some(**t), false),
            None => (implied_type(&obj), false),
        };

        let result = match effective_type {
            Some(InstanceType::Boolean) => Self::Bool,
            Some(InstanceType::Integer) => match obj.format.as_deref() {
                Some("int16" | "int8") => Self::Int16,
                Some("uint16") => Self::UInt16,
                Some("int32") => Self::Int32,
                Some("uint8" | "uint32" | "uint" | "uint64") => Self::UInt64,
                // Formats are annotations: an unknown one (`int`, `unix-time`) is still an integer.
                _ => Self::Int64,
            },
            Some(InstanceType::Number) => match obj.format.as_deref() {
                Some("float") => Self::Float,
                _ => Self::Double,
            },
            Some(InstanceType::String) => {
                if let Some(values) = obj.enum_values {
                    let mut values: Vec<String> = values
                        .into_iter()
                        .filter(|v| !v.is_null())
                        .map(|v| match v {
                            serde_json::Value::String(s) => Ok(s),
                            _ => bail!("string enums with non-string values are not supported"),
                        })
                        .collect::<anyhow::Result<_>>()?;
                    // A single value is a constant, most often a discriminator.
                    if values.len() <= 1 {
                        return Ok((Self::String, nullable));
                    }
                    values.dedup();
                    let title = obj.metadata.as_ref().and_then(|m| m.title.clone());
                    return Ok((Self::StringEnum { values, title }, nullable));
                }
                match obj.format.as_deref() {
                    Some("decimal") => Self::Decimal,
                    Some("date-time") => Self::DateTime,
                    Some("date") => Self::Date,
                    Some("uri") => Self::Uri,
                    _ => Self::String,
                }
            }
            Some(InstanceType::Array) => {
                let array = obj.array.unwrap_or_default();
                let inner = match array.items {
                    None => Self::JsonObject,
                    Some(SingleOrVec::Single(ty)) => Self::from_schema(*ty)?,
                    Some(SingleOrVec::Vec(_)) => {
                        bail!("tuple arrays (`items` as a list) are not supported")
                    }
                };
                let inner = Arc::new(inner);
                if array.unique_items == Some(true) {
                    Self::Set { inner }
                } else {
                    Self::List { inner }
                }
            }
            Some(InstanceType::Object) => {
                let obj = obj.object.unwrap_or_default();
                ensure!(
                    obj.properties.is_empty(),
                    "inline objects with properties must be named schemas"
                );
                match obj.additional_properties.map(|s| *s) {
                    None | Some(Schema::Bool(_)) => Self::JsonObject,
                    Some(Schema::Object(schema_object)) => {
                        let value_ty = Arc::new(Self::from_schema_object(schema_object)?);
                        Self::Map { value_ty }
                    }
                }
            }
            Some(InstanceType::Null) => bail!("`null`-only values are not supported"),
            None => match (get_schema_name(obj.reference.as_deref()), obj.const_value) {
                (Some(name), _) => Self::SchemaRef { name, inner: None },
                (None, Some(serde_json::Value::String(_))) => Self::String,
                (None, Some(serde_json::Value::Bool(_))) => Self::Bool,
                (None, Some(serde_json::Value::Number(n))) if n.is_i64() => Self::Int64,
                (None, Some(serde_json::Value::Number(_))) => Self::Double,
                // `{}` accepts any JSON value.
                (None, _) => Self::JsonObject,
            },
        };

        Ok((result, nullable))
    }

    fn to_csharp_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "bool".into(),
            Self::Int16 => "short".into(),
            Self::Int32 => "int".into(),
            Self::Int64 => "long".into(),
            Self::UInt16 => "ushort".into(),
            Self::UInt64 => "ulong".into(),
            Self::Float => "float".into(),
            Self::Double => "double".into(),
            Self::String | Self::Uri => "string".into(),
            Self::Decimal => "decimal".into(),
            Self::DateTime => "DateTimeOffset".into(),
            Self::JsonObject => "JsonNode".into(),
            Self::Map { value_ty } => {
                format!("Dictionary<string, {}>", value_ty.to_csharp_typename()).into()
            }
            Self::List { inner } | Self::Set { inner } => {
                format!("List<{}>", inner.to_csharp_typename()).into()
            }
            Self::SchemaRef {
                inner: Some(ty), ..
            } if matches!(ty.data, TypeData::StringAlias) => "string".into(),
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Date => "string".into(),
            Self::StringEnum { .. } => "string".into(),
        }
    }

    fn to_go_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "bool".into(),
            Self::Int16 => "int16".into(),
            Self::Int32 => "int32".into(),
            Self::Int64 => "int64".into(),
            Self::UInt16 => "uint16".into(),
            Self::UInt64 => "uint64".into(),
            Self::Float => "float32".into(),
            Self::Double => "float64".into(),
            Self::Uri | Self::String | Self::Decimal => "string".into(),
            Self::DateTime => "time.Time".into(),
            Self::JsonObject => "map[string]any".into(),
            Self::Map { value_ty } => format!("map[string]{}", value_ty.to_go_typename()).into(),
            Self::List { inner } | Self::Set { inner } => {
                format!("[]{}", inner.to_go_typename()).into()
            }
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Date => "string".into(),
            Self::StringEnum { .. } => "string".into(),
        }
    }

    fn to_kotlin_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "Boolean".into(),
            Self::Int16 => "Short".into(),
            Self::Int32 => "Int".into(),
            Self::UInt16 => "UShort".into(),
            Self::Int64 => "Long".into(),
            Self::UInt64 => "ULong".into(),
            Self::Float => "Float".into(),
            Self::Double => "Double".into(),
            Self::Uri | Self::String => "String".into(),
            Self::Decimal => "java.math.BigDecimal".into(),
            Self::DateTime => "Instant".into(),
            Self::Map { value_ty } => {
                format!("Map<String,{}>", value_ty.to_kotlin_typename()).into()
            }
            Self::JsonObject => "Map<String,Any>".into(),
            Self::List { inner } => format!("List<{}>", inner.to_kotlin_typename()).into(),
            Self::Set { inner } => format!("Set<{}>", inner.to_kotlin_typename()).into(),
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Date => "String".into(),
            Self::StringEnum { .. } => "String".into(),
        }
    }

    fn to_js_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "boolean".into(),
            Self::Int16
            | Self::UInt16
            | Self::Int32
            | Self::Int64
            | Self::UInt64
            | Self::Float
            | Self::Double => "number".into(),
            // `format: decimal` values travel over the wire as JSON strings
            // (`"12.50"`), and JS `number` cannot represent them losslessly, so
            // the TypeScript SDK surfaces them as strings.
            Self::Decimal => "string".into(),
            Self::String | Self::Uri => "string".into(),
            Self::DateTime => "Date".into(),
            Self::JsonObject => "any".into(),
            Self::List { inner } | Self::Set { inner } => {
                format!("{}[]", inner.to_js_typename()).into()
            }
            Self::Map { value_ty } => {
                format!("{{ [key: string]: {} }}", value_ty.to_js_typename()).into()
            }
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Date => "string".into(),
            Self::StringEnum { .. } => "string".into(),
        }
    }

    fn to_rust_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "bool".into(),
            Self::Int16 => "i16".into(),
            Self::UInt16 => "u16".into(),
            Self::Int32 => "i32".into(),
            Self::Int64 => "i64".into(),
            Self::UInt64 => "u64".into(),
            Self::Float => "f32".into(),
            Self::Double => "f64".into(),
            // FIXME: Do we want a separate type for Uri?
            Self::Uri | Self::String => "String".into(),
            Self::Decimal => "rust_decimal::Decimal".into(),
            // FIXME: Depends on those chrono imports being in scope, not that great..
            Self::DateTime => "DateTime<Utc>".into(),
            Self::JsonObject => "serde_json::Value".into(),
            // FIXME: Treat set differently? (BTreeSet)
            Self::List { inner } | Self::Set { inner } => {
                format!("Vec<{}>", inner.to_rust_typename()).into()
            }
            Self::Map { value_ty } => format!(
                "std::collections::HashMap<String, {}>",
                value_ty.to_rust_typename(),
            )
            .into(),
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Date => "String".into(),
            Self::StringEnum { .. } => "String".into(),
        }
    }

    /// Whether the SDK value is the decoded JSON value itself in every language: scalars,
    /// untyped JSON, and lists and maps of those.
    pub(crate) fn is_plain_json(&self) -> bool {
        match self {
            Self::Bool
            | Self::Int16
            | Self::UInt16
            | Self::Int32
            | Self::Int64
            | Self::UInt64
            | Self::Float
            | Self::Double
            | Self::String
            | Self::Date
            | Self::Uri
            | Self::JsonObject => true,
            Self::List { inner } | Self::Set { inner } => inner.is_plain_json(),
            Self::Map { value_ty } => value_ty.is_plain_json(),
            Self::Decimal | Self::DateTime | Self::SchemaRef { .. } | Self::StringEnum { .. } => {
                false
            }
        }
    }

    pub(crate) fn inline_aliases(&mut self, aliases: &BTreeMap<String, FieldType>) {
        match self {
            Self::SchemaRef { name, .. } => {
                if let Some(target) = aliases.get(name) {
                    *self = target.clone();
                }
            }
            Self::List { inner } | Self::Set { inner } => {
                Arc::make_mut(inner).inline_aliases(aliases);
            }
            Self::Map { value_ty } => Arc::make_mut(value_ty).inline_aliases(aliases),
            _ => {}
        }
    }

    pub(crate) fn referenced_schema(&self) -> Option<&str> {
        match self {
            Self::SchemaRef { name, .. } => Some(name),
            Self::List { inner: ty } | Self::Set { inner: ty } | Self::Map { value_ty: ty } => {
                ty.referenced_schema()
            }
            _ => None,
        }
    }

    fn to_python_typename(&self) -> Cow<'_, str> {
        match self {
            Self::Bool => "bool".into(),
            Self::Int16 | Self::UInt16 | Self::Int32 | Self::Int64 | Self::UInt64 => "int".into(),
            Self::Float | Self::Double => "float".into(),
            Self::String => "str".into(),
            Self::Decimal => "Decimal".into(),
            Self::DateTime => "datetime".into(),
            Self::SchemaRef { name, .. } => name.to_upper_camel_case().into(),
            Self::Uri => "str".into(),
            Self::JsonObject => "t.Dict[str, t.Any]".into(),
            Self::Set { inner } | Self::List { inner } => {
                format!("t.List[{}]", inner.to_python_typename()).into()
            }
            Self::Map { value_ty } => {
                format!("t.Dict[str, {}]", value_ty.to_python_typename()).into()
            }
            Self::Date => "str".into(),
            Self::StringEnum { .. } => "str".into(),
        }
    }

    fn to_java_typename(&self) -> Cow<'_, str> {
        match self {
            // _ => "String".into(),
            FieldType::Bool => "Boolean".into(),
            FieldType::Int16 => "Short".into(),
            FieldType::UInt16 | FieldType::UInt64 | FieldType::Int64 => "Long".into(),
            FieldType::Int32 => "Integer".into(),
            FieldType::Float => "Float".into(),
            FieldType::Double => "Double".into(),
            FieldType::String => "String".into(),
            FieldType::Decimal => "BigDecimal".into(),
            FieldType::DateTime => "OffsetDateTime".into(),
            FieldType::Uri => "URI".into(),
            FieldType::JsonObject => "Object".into(),
            FieldType::List { inner } => format!("List<{}>", inner.to_java_typename()).into(),
            FieldType::Set { inner: field_type } => {
                format!("Set<{}>", field_type.to_java_typename()).into()
            }
            FieldType::Map { value_ty } => {
                format!("Map<String,{}>", value_ty.to_java_typename()).into()
            }
            FieldType::SchemaRef { name, inner } => {
                // Java doesn't have type aliases, so resolve string alias refs to String
                if let Some(ty) = inner
                    && matches!(ty.data, TypeData::StringAlias)
                {
                    return "String".into();
                }
                name.to_upper_camel_case().into()
            }
            FieldType::Date => "String".into(),
            FieldType::StringEnum { .. } => "String".into(),
        }
    }

    /// Check if this field type needs an import statement in Java.
    /// Returns false for primitives, built-in types, and string aliases.
    fn needs_java_import(&self) -> bool {
        match self {
            FieldType::Bool
            | FieldType::Int16
            | FieldType::UInt16
            | FieldType::Int32
            | FieldType::Int64
            | FieldType::UInt64
            | FieldType::Float
            | FieldType::Double
            | FieldType::String
            | FieldType::Decimal
            | FieldType::DateTime
            | FieldType::Uri
            | FieldType::JsonObject
            | FieldType::Date => false,
            FieldType::StringEnum { .. } => false,
            FieldType::List { inner } | FieldType::Set { inner } => inner.needs_java_import(),
            FieldType::Map { value_ty } => value_ty.needs_java_import(),
            FieldType::SchemaRef { inner, .. } => {
                // String aliases don't need import - they resolve to String
                if let Some(ty) = inner {
                    !matches!(ty.data, TypeData::StringAlias)
                } else {
                    true
                }
            }
        }
    }

    fn to_ruby_typename(&self) -> Cow<'_, str> {
        match self {
            FieldType::SchemaRef { name, .. } => name.clone().into(),
            _ => panic!("types? in ruby?!?!, not on my watch!"),
        }
    }

    /// returns `PHPDoc` annotations
    fn to_phpdoc_typename(&self) -> Cow<'_, str> {
        match self {
            FieldType::Bool
            | FieldType::Int16
            | FieldType::UInt16
            | FieldType::Int32
            | FieldType::Int64
            | FieldType::UInt64
            | FieldType::Float
            | FieldType::Double
            | FieldType::String
            | FieldType::Decimal
            | FieldType::DateTime
            | FieldType::Uri
            | FieldType::JsonObject
            | FieldType::Date
            | FieldType::SchemaRef { .. } => self.to_php_typename(),
            FieldType::StringEnum { .. } => "string".into(),
            FieldType::Set { inner } | FieldType::List { inner } => {
                format!("list<{}>", inner.to_phpdoc_typename()).into()
            }
            FieldType::Map { value_ty } => {
                format!("array<string, {}>", value_ty.to_phpdoc_typename()).into()
            }
        }
    }

    fn to_php_typename(&self) -> Cow<'_, str> {
        match self {
            FieldType::Bool => "bool".into(),
            FieldType::UInt16
            | FieldType::Int16
            | FieldType::UInt64
            | FieldType::Int32
            | FieldType::Int64 => "int".into(),
            FieldType::Float | FieldType::Double | FieldType::Decimal => "float".into(),
            FieldType::Uri | FieldType::Date | FieldType::String => "string".into(),
            FieldType::StringEnum { .. } => "string".into(),
            FieldType::DateTime => r#"\DateTimeImmutable"#.into(),

            FieldType::JsonObject
            | FieldType::List { .. }
            | FieldType::Set { .. }
            | FieldType::Map { .. } => "array".into(),
            FieldType::SchemaRef { name, .. } => name.clone().into(),
        }
    }
}

impl minijinja::value::Object for FieldType {
    fn repr(self: &Arc<Self>) -> minijinja::value::ObjectRepr {
        minijinja::value::ObjectRepr::Plain
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[minijinja::Value],
    ) -> Result<minijinja::Value, minijinja::Error> {
        match method {
            "to_python" => {
                ensure_no_args(args, "to_python")?;
                Ok(self.to_python_typename().into())
            }
            "to_csharp" => {
                ensure_no_args(args, "to_csharp")?;
                Ok(self.to_csharp_typename().into())
            }
            "to_go" => {
                ensure_no_args(args, "to_go")?;
                Ok(self.to_go_typename().into())
            }
            "to_js" => {
                ensure_no_args(args, "to_js")?;
                Ok(self.to_js_typename().into())
            }
            "to_kotlin" => {
                ensure_no_args(args, "to_kotlin")?;
                Ok(self.to_kotlin_typename().into())
            }
            "to_rust" => {
                ensure_no_args(args, "to_rust")?;
                Ok(self.to_rust_typename().into())
            }
            "to_java" => {
                ensure_no_args(args, "to_java")?;
                Ok(self.to_java_typename().into())
            }
            "needs_java_import" => {
                ensure_no_args(args, "needs_java_import")?;
                Ok(self.needs_java_import().into())
            }
            "to_ruby" => {
                ensure_no_args(args, "to_ruby")?;
                Ok(self.to_ruby_typename().into())
            }
            "to_php" => {
                ensure_no_args(args, "to_php")?;
                Ok(self.to_php_typename().into())
            }
            "to_phpdoc" => {
                ensure_no_args(args, "to_phpdoc")?;
                Ok(self.to_phpdoc_typename().into())
            }

            "is_datetime" => {
                ensure_no_args(args, "is_datetime")?;
                Ok(matches!(**self, Self::DateTime).into())
            }
            "is_schema_ref" => {
                ensure_no_args(args, "is_schema_ref")?;
                Ok(matches!(**self, Self::SchemaRef { .. }).into())
            }
            "is_list" => {
                ensure_no_args(args, "is_list")?;
                Ok(matches!(**self, Self::List { .. }).into())
            }
            "is_set" => {
                ensure_no_args(args, "is_set")?;
                Ok(matches!(**self, Self::Set { .. }).into())
            }
            "is_map" => {
                ensure_no_args(args, "is_map")?;
                Ok(matches!(**self, Self::Map { .. }).into())
            }
            // Dates are typed as strings, so templates treat them alike unless they check `is_date`.
            "is_string" => {
                ensure_no_args(args, "is_string")?;
                Ok(matches!(**self, Self::String | Self::Date).into())
            }
            "is_uri" => {
                ensure_no_args(args, "is_uri")?;
                Ok(matches!(**self, Self::Uri).into())
            }
            "is_bool" => {
                ensure_no_args(args, "is_bool")?;
                Ok(matches!(**self, Self::Bool).into())
            }
            "is_int_or_uint" => {
                ensure_no_args(args, "is_int_or_uint")?;
                use FieldType as F;
                let is_int_or_uint = match &**self {
                    F::Int16 | F::UInt16 | F::Int32 | F::Int64 | F::UInt64 => true,
                    F::Bool
                    | F::Float
                    | F::Double
                    | F::String
                    | F::Decimal
                    | F::DateTime
                    | F::Uri
                    | F::JsonObject
                    | F::List { .. }
                    | F::Set { .. }
                    | F::Map { .. }
                    | F::SchemaRef { .. }
                    | F::Date => false,
                    F::StringEnum { .. } => false,
                };
                Ok(is_int_or_uint.into())
            }
            "is_json_object" => {
                ensure_no_args(args, "is_json_object")?;
                Ok(matches!(**self, Self::JsonObject).into())
            }
            "is_date" => {
                ensure_no_args(args, "is_date")?;
                Ok(matches!(**self, Self::Date).into())
            }

            // Returns the inner type of a list or set
            "inner_type" => {
                ensure_no_args(args, "inner_type")?;

                let ty = match &**self {
                    FieldType::List { inner } | FieldType::Set { inner } => {
                        Some(minijinja::Value::from_dyn_object(inner.clone()))
                    }
                    _ => None,
                };
                Ok(ty.into())
            }
            "inner_schema_ref_ty" => {
                ensure_no_args(args, "inner_schema_ref_ty")?;
                let ty = match &**self {
                    FieldType::SchemaRef { inner, .. } => {
                        inner.as_ref().map(minijinja::Value::from_serialize)
                    }
                    _ => None,
                };
                Ok(ty.into())
            }
            // Returns the value type of a map
            "value_type" => {
                ensure_no_args(args, "value_type")?;

                let ty = match &**self {
                    FieldType::Map { value_ty } => {
                        Some(minijinja::Value::from_dyn_object(value_ty.clone()))
                    }
                    _ => None,
                };
                Ok(ty.into())
            }
            _ => Err(minijinja::Error::from(minijinja::ErrorKind::UnknownMethod)),
        }
    }
}

fn ensure_no_args(args: &[minijinja::Value], method_name: &str) -> Result<(), minijinja::Error> {
    if !args.is_empty() {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::TooManyArguments,
            format!("{method_name} does not take any arguments"),
        ));
    }
    Ok(())
}

/// Serialize a `FieldType`, as an object for minijinja, or
pub(super) fn serialize_field_type<S>(
    field_ty: &FieldType,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if minijinja::value::serializing_for_value() {
        minijinja::Value::from_object(field_ty.clone()).serialize(serializer)
    } else {
        field_ty.serialize(serializer)
    }
}

pub(super) fn serialize_optional_field_type<S>(
    field_ty: &Option<FieldType>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match field_ty {
        Some(field_ty) => serialize_field_type(field_ty, serializer),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(value: serde_json::Value) -> SchemaObject {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn rust_preserves_integer_width_and_acronym_names() {
        assert_eq!(FieldType::Int64.to_rust_typename(), "i64");
        assert_eq!(FieldType::UInt64.to_rust_typename(), "u64");
        let ty = FieldType::SchemaRef {
            name: "OPADecisionLog".into(),
            inner: None,
        };
        assert_eq!(ty.to_rust_typename(), "OpaDecisionLog");
    }

    #[test]
    fn aliases_retain_nested_dependencies() {
        let ty = Type::from_schema(
            "Results".into(),
            schema(json!({
                "type": "array", "items": {"$ref": "#/components/schemas/Widget"}
            })),
        )
        .unwrap();
        assert!(matches!(ty.data, TypeData::Alias { .. }));
        assert_eq!(ty.referenced_components(), BTreeSet::from(["Widget"]));
    }

    #[test]
    fn named_empty_structs_and_explicit_freeform_objects_are_distinct() {
        let named = Type::from_schema("Empty".into(), schema(json!({"type":"object"}))).unwrap();
        assert!(matches!(named.data, TypeData::Struct { .. }));
        let map = Type::from_schema(
            "Metadata".into(),
            schema(json!({"type":"object", "additionalProperties":true})),
        )
        .unwrap();
        assert!(matches!(map.data, TypeData::Alias { .. }));
    }

    #[test]
    fn tagged_unions_collect_every_field_dependency() {
        let ty = Type::from_schema(
            "Event".into(),
            schema(json!({
                "oneOf": [{"type":"object", "required":["kind", "left", "right"],
                    "properties": {
                        "kind":{"type":"string", "enum":["pair"]},
                        "left":{"$ref":"#/components/schemas/Left"},
                        "right":{"type":"array", "items":{"$ref":"#/components/schemas/Right"}}
                    }}],
                "discriminator":{"propertyName":"kind"}
            })),
        )
        .unwrap();
        assert_eq!(
            ty.referenced_components(),
            BTreeSet::from(["Left", "Right"])
        );
    }

    fn types_from(schemas: serde_json::Value) -> Types {
        schemas
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, s)| {
                let ty = Type::from_schema(name.clone(), schema(s.clone())).unwrap();
                (name.clone(), ty)
            })
            .collect()
    }

    fn field_type<'a>(types: &'a Types, ty: &str, field: &str) -> &'a FieldType {
        let TypeData::Struct { fields } = &types[ty].data else {
            panic!("{ty} is not a struct");
        };
        &fields.iter().find(|f| f.name == field).unwrap().r#type
    }

    #[test]
    fn inlining_replaces_alias_references_and_chains() {
        let mut types = types_from(json!({
            "Widgets": {"type": "array", "items": {"$ref": "#/components/schemas/Widget"}},
            "Batch": {"$ref": "#/components/schemas/Widgets"},
            "Widget": {"type": "object", "properties": {"id": {"type": "string"}}},
            "Holder": {"type": "object", "properties": {
                "batch": {"$ref": "#/components/schemas/Batch"},
                "labels": {"type": "array", "items": {"$ref": "#/components/schemas/Widgets"}},
                "widget": {"$ref": "#/components/schemas/Widget"}
            }}
        }));
        inline_aliases(&mut types, &mut Resources::new()).unwrap();

        let widget = FieldType::SchemaRef {
            name: "Widget".into(),
            inner: None,
        };
        let FieldType::List { inner } = field_type(&types, "Holder", "batch") else {
            panic!("alias chain was not inlined");
        };
        assert_eq!(**inner, widget);
        let FieldType::List { inner } = field_type(&types, "Holder", "labels") else {
            panic!("nested alias was not inlined");
        };
        assert!(matches!(&**inner, FieldType::List { .. }));
        assert_eq!(field_type(&types, "Holder", "widget"), &widget);
        assert_eq!(
            types["Batch"].referenced_components(),
            BTreeSet::from(["Widget"])
        );
    }

    #[test]
    fn cyclic_aliases_are_rejected() {
        let mut types = types_from(json!({
            "A": {"$ref": "#/components/schemas/B"},
            "B": {"$ref": "#/components/schemas/A"}
        }));
        let error = inline_aliases(&mut types, &mut Resources::new()).unwrap_err();
        assert!(error.to_string().contains("cyclic alias"));
    }

    #[test]
    fn scalar_and_untyped_components_are_aliases() {
        for (name, value) in [
            ("Count", json!({"type": "integer", "format": "int64"})),
            ("Flag", json!({"type": "boolean"})),
            ("Ratio", json!({"type": "number"})),
            ("Anything", json!({})),
            ("Ids", json!({"type": "array", "items": {"type": "string"}})),
        ] {
            let ty = Type::from_schema(name.into(), schema(value)).unwrap();
            assert!(matches!(ty.data, TypeData::Alias { .. }), "{name}");
        }
    }

    #[test]
    fn closed_objects_are_structs() {
        let empty = Type::from_schema(
            "Empty".into(),
            schema(json!({"type": "object", "additionalProperties": false})),
        )
        .unwrap();
        assert!(matches!(empty.data, TypeData::Struct { ref fields } if fields.is_empty()));
        let closed = Type::from_schema(
            "Closed".into(),
            schema(json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"id": {"type": "string"}}
            })),
        )
        .unwrap();
        assert!(matches!(closed.data, TypeData::Struct { ref fields } if fields.len() == 1));
    }

    #[test]
    fn data_is_an_ordinary_schema_name() {
        let ty = FieldType::SchemaRef {
            name: "Data".into(),
            inner: None,
        };
        assert_eq!(ty.referenced_schema(), Some("Data"));
        assert_eq!(ty.to_rust_typename(), "Data");
        assert_eq!(ty.to_js_typename(), "Data");
    }

    fn field(value: serde_json::Value) -> Field {
        Field::from_schema("f".into(), Schema::Object(schema(value)), false).unwrap()
    }

    #[test]
    fn unions_without_a_discriminator_are_untyped_json() {
        let f = field(json!({"oneOf": [{"$ref": "#/components/schemas/A"}, {"type": "string"}]}));
        assert_eq!((f.r#type, f.nullable), (FieldType::JsonObject, false));
        let f =
            field(json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]}));
        assert_eq!((f.r#type, f.nullable), (FieldType::JsonObject, true));
        let f = field(json!({"type": ["string", "integer"]}));
        assert_eq!(f.r#type, FieldType::JsonObject);
    }

    #[test]
    fn nullable_and_wrapped_references_keep_their_type() {
        let a = FieldType::SchemaRef {
            name: "A".into(),
            inner: None,
        };
        let f = field(json!({"anyOf": [{"$ref": "#/components/schemas/A"}, {"type": "null"}]}));
        assert_eq!((&f.r#type, f.nullable), (&a, true));
        let f = field(json!({"allOf": [{"$ref": "#/components/schemas/A"}], "description": "d"}));
        assert_eq!((&f.r#type, f.description.as_deref()), (&a, Some("d")));
    }

    #[test]
    fn constants_and_single_value_enums_are_plain_values() {
        assert_eq!(
            field(json!({"type": "string", "const": "circle"})).r#type,
            FieldType::String
        );
        assert_eq!(
            field(json!({"type": "string", "enum": ["only"]})).r#type,
            FieldType::String
        );
        assert_eq!(field(json!({"const": 2})).r#type, FieldType::Int64);
        assert_eq!(field(json!({"const": true})).r#type, FieldType::Bool);
        let f = field(json!({"type": ["string", "null"], "enum": ["a", "b", null]}));
        assert!(
            matches!(f.r#type, FieldType::StringEnum { ref values, .. } if values == &["a", "b"])
        );
        assert!(f.nullable);
    }

    #[test]
    fn dates_and_access_modes_reach_templates() {
        assert_eq!(
            field(json!({"type": "string", "format": "date"})).r#type,
            FieldType::Date
        );
        assert_eq!(
            field(json!({"type": "integer", "format": "unix-time"})).r#type,
            FieldType::Int64
        );
        assert_eq!(
            field(json!({"type": "number", "format": "decimal"})).r#type,
            FieldType::Double
        );
        let f = field(json!({"type": "string", "readOnly": true}));
        assert!(f.read_only && !f.write_only);
        let f = field(json!({"type": "string", "writeOnly": true}));
        assert!(f.write_only && !f.read_only);
    }

    #[test]
    fn discriminator_mappings_accept_bare_names_and_aliases() {
        let ty = Type::from_schema(
            "Shape".into(),
            schema(json!({
                "oneOf": [{"$ref": "#/components/schemas/Circle"}, {"$ref": "#/components/schemas/Square"}],
                "discriminator": {"propertyName": "kind", "mapping": {
                    "circle": "Circle", "round": "#/components/schemas/Circle"
                }}
            })),
        )
        .unwrap();
        let TypeData::StructEnum {
            repr: StructEnumRepr::InternallyTagged { variants },
            ..
        } = ty.data
        else {
            panic!("not a union");
        };
        let names: Vec<_> = variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["circle", "round", "Square"]);
    }

    #[test]
    fn integer_enums_without_varnames_are_named_after_their_values() {
        let ty = Type::from_schema(
            "Level".into(),
            schema(json!({"type": "integer", "enum": [1, -1]})),
        )
        .unwrap();
        assert_eq!(
            ty.data,
            TypeData::IntegerEnum {
                variants: vec![("Value1".into(), 1), ("Minus1".into(), -1)]
            }
        );
    }

    #[test]
    fn top_level_objects_without_a_type_and_nullable_enums_are_declared() {
        let ty = Type::from_schema(
            "Page".into(),
            schema(json!({"properties": {"a": {"type": "string"}}})),
        )
        .unwrap();
        assert!(matches!(ty.data, TypeData::Struct { ref fields } if fields.len() == 1));
        let ty = Type::from_schema(
            "Mode".into(),
            schema(json!({"anyOf": [{"type": "string", "enum": ["a", "b"]}, {"type": "null"}]})),
        )
        .unwrap();
        assert!(matches!(ty.data, TypeData::StringEnum { .. }));
        let error = Type::from_schema(
            "Mixed".into(),
            schema(json!({"allOf": [{"type": "string"}, {"type": "integer"}]})),
        )
        .unwrap_err();
        assert!(error.to_string().contains("`allOf`"), "{error}");
    }

    #[test]
    fn clashing_names_are_reported() {
        let types = types_from(json!({
            "issue": {"type": "object", "properties": {"type": {"type": "string"}, "@type": {"type": "string"}}},
            "Issue": {"type": "string", "enum": ["a-b", "a_b"]}
        }));
        assert_eq!(
            clashing_type_names(&types),
            ["schemas `Issue` and `issue` both become the type `Issue`"]
        );
        let errors = clashing_identifiers(&types);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors[0].starts_with("schema `Issue`: `a-b` and `a_b`"),
            "{errors:?}"
        );
        assert!(
            errors[1].starts_with("schema `issue`: `@type` and `type`"),
            "{errors:?}"
        );
    }

    #[test]
    fn unions_over_non_object_variants_become_untyped_json() {
        let mut types = types_from(json!({
            "Listing": {"type": "array", "items": {"type": "string"}},
            "File": {"type": "object", "properties": {"type": {"type": "string"}}},
            "Content": {
                "oneOf": [{"$ref": "#/components/schemas/Listing"}, {"$ref": "#/components/schemas/File"}],
                "discriminator": {"propertyName": "type"}
            }
        }));
        untag_unions_with_non_object_variants(&mut types);
        assert_eq!(
            types["Content"].data,
            TypeData::Alias {
                target: Box::new(FieldType::JsonObject)
            }
        );
    }

    #[test]
    fn promoted_enums_avoid_names_that_differ_only_in_case() {
        let mut types = types_from(json!({
            "thing_kind": {"type": "string"},
            "Thing": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["a", "b"]}}}
        }));
        promote_inline_enums(&mut types, &mut Resources::new()).unwrap();
        assert_eq!(
            field_type(&types, "Thing", "kind"),
            &FieldType::SchemaRef {
                name: "ThingKind2".into(),
                inner: None
            }
        );
    }

    #[test]
    fn all_of_embeds_references_next_to_merged_properties() {
        let ty = Type::from_schema(
            "Composed".into(),
            schema(json!({
                "allOf": [{"$ref": "#/components/schemas/Base"}],
                "type": "object",
                "required": ["extra"],
                "properties": {"extra": {"type": "string"}}
            })),
        )
        .unwrap();
        let TypeData::Struct { fields } = &ty.data else {
            panic!("not a struct");
        };
        let fields: Vec<_> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.flatten))
            .collect();
        assert_eq!(fields, [("__flatten_base", true), ("extra", false)]);
        assert_eq!(ty.referenced_components(), BTreeSet::from(["Base"]));
    }

    #[test]
    fn flattened_parts_are_inlined_and_own_fields_win() {
        let object = |props: serde_json::Value| {
            schema(json!({"type": "object", "required": ["id"], "properties": props}))
        };
        let mut types = Types::new();
        for (name, schema) in [
            (
                "Base",
                object(json!({"id": {"type": "string"}, "note": {"type": "string"}})),
            ),
            (
                "Composed",
                schema(json!({
                    "allOf": [{"$ref": "#/components/schemas/Base"}],
                    "type": "object",
                    "properties": {"note": {"type": "integer"}}
                })),
            ),
        ] {
            types.insert(name.into(), Type::from_schema(name.into(), schema).unwrap());
        }
        inline_flattened_fields(&mut types).unwrap();
        let TypeData::Struct { fields } = &types["Composed"].data else {
            panic!("not a struct");
        };
        let fields: Vec<_> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.r#type.clone()))
            .collect();
        assert_eq!(
            fields,
            [("id", FieldType::String), ("note", FieldType::Int64)]
        );
    }
}
