use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, btree_map},
    sync::Arc,
};

use aide::openapi;
use anyhow::{Context as _, bail, ensure};
use indexmap::IndexMap;
use itertools::Itertools as _;
use schemars::schema::{
    InstanceType, ObjectValidation, Schema, SchemaObject, SingleOrVec, SubschemaValidation,
};
use serde::{Deserialize, Serialize};

use crate::spec::IncludeMode;

use heck::{ToSnakeCase as _, ToUpperCamelCase as _};

use super::{
    get_schema_name,
    resources::{self, Resource, Resources},
    unions::{self, Condition, UnionMode},
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
        IncludeMode::OnlyPublic | IncludeMode::PublicAndInternal => {
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
                        .chain(ty.union_refs())
                        .filter(|&c| c != schema_name && !types.contains_key(c))
                        .map(ToOwned::to_owned),
                );
                types.insert(schema_name.to_owned(), ty);
            }
            // A schema no SDK can model is still a valid JSON value.
            Err(e) => {
                tracing::warn!("{e:#}, so the schema is typed as an untyped JSON value");
                types.insert(
                    schema_name.to_owned(),
                    Type {
                        name: schema_name.to_owned(),
                        description: None,
                        deprecated: false,
                        discriminator_defaults: BTreeMap::new(),
                        data: TypeData::Alias {
                            target: Box::new(FieldType::JsonObject),
                        },
                    },
                );
            }
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

/// Settles the JSON type of the variants of every union referencing a schema, typing the unions
/// whose variants cannot be told apart as untyped JSON. Operations only take untyped JSON.
pub(crate) fn resolve_unions(types: &mut Types, resources: &mut Resources) {
    fn json_type(types: &Types, name: &str, depth: usize) -> Option<&'static str> {
        match &types.get(name)?.data {
            TypeData::Struct { .. } | TypeData::StructEnum { .. } => Some("object"),
            TypeData::StringEnum { .. } | TypeData::StringAlias => Some("string"),
            TypeData::IntegerEnum { .. } => Some("integer"),
            TypeData::Alias { target } if depth < 16 => match &**target {
                FieldType::SchemaRef { name, .. } => json_type(types, name, depth + 1),
                FieldType::JsonObject | FieldType::Union { .. } | FieldType::StringEnum { .. } => {
                    None
                }
                target => UnionVariant::json_type_of(target),
            },
            TypeData::Alias { .. } => None,
        }
    }
    fn settle(ty: &mut FieldType, known: &Known, owner: &str) {
        match ty {
            FieldType::Union {
                variants,
                mode,
                requested,
            } => {
                let mut settled = true;
                for variant in variants.iter_mut() {
                    settle(&mut variant.r#type, known, owner);
                    if let FieldType::SchemaRef { name, .. } = &variant.r#type {
                        match known.kinds.get(name).copied().flatten() {
                            Some(kind) => variant.json_type = kind.to_owned(),
                            None => settled = false,
                        }
                    }
                }
                let _span = tracing::warn_span!("schema", name = %owner).entered();
                if *requested == Some(UnionMode::Json) {
                    *ty = FieldType::JsonObject;
                } else if settled && distinct_json_types(variants) {
                } else if let Some(decided) = settled
                    .then(|| decide_objects(variants, &known.shapes))
                    .flatten()
                {
                    *mode = decided;
                } else {
                    tracing::warn!(
                        "`oneOf`/`anyOf` without a discriminator is typed as an untyped JSON value"
                    );
                    *ty = FieldType::JsonObject;
                }
            }
            FieldType::List { inner } | FieldType::Set { inner } => {
                settle(Arc::make_mut(inner), known, owner)
            }
            FieldType::Map { value_ty } => settle(Arc::make_mut(value_ty), known, owner),
            _ => {}
        }
    }
    struct Known {
        kinds: BTreeMap<String, Option<&'static str>>,
        shapes: BTreeMap<String, Option<Vec<unions::Property>>>,
    }
    let known = Known {
        kinds: types
            .keys()
            .map(|name| (name.clone(), json_type(types, name, 0)))
            .collect(),
        shapes: types
            .keys()
            .map(|name| (name.clone(), unions::shape(types, name)))
            .collect(),
    };
    for (name, ty) in types.iter_mut() {
        ty.data.for_each_field_type(|t| settle(t, &known, name));
    }
    let mut stack: Vec<&mut Resource> = resources.values_mut().collect();
    while let Some(resource) = stack.pop() {
        for op in &mut resource.operations {
            op.untype_unions();
        }
        stack.extend(resource.subresources.values_mut());
    }
}

/// Sets the `id` of the struct variants of every union, once access modes settled which fields
/// are required.
pub(crate) fn set_union_ids(types: &mut Types) {
    fn set(ty: &mut FieldType, ids: &BTreeMap<String, Option<String>>) {
        match ty {
            FieldType::Union { variants, .. } => {
                for variant in variants.iter_mut() {
                    set(&mut variant.r#type, ids);
                    if let FieldType::SchemaRef { name, .. } = &variant.r#type {
                        variant.id = ids.get(name).cloned().flatten();
                    }
                }
            }
            FieldType::List { inner } | FieldType::Set { inner } => set(Arc::make_mut(inner), ids),
            FieldType::Map { value_ty } => set(Arc::make_mut(value_ty), ids),
            _ => {}
        }
    }
    let ids: BTreeMap<String, Option<String>> = types
        .iter()
        .map(|(name, ty)| {
            let TypeData::Struct { fields } = &ty.data else {
                return (name.clone(), None);
            };
            let id = fields
                .iter()
                .find(|f| {
                    f.name == "id" && !f.flatten && !f.nullable && f.r#type == FieldType::String
                })
                .map(|f| if f.required { "required" } else { "optional" }.to_owned());
            (name.clone(), id)
        })
        .collect();
    for ty in types.values_mut() {
        ty.data.for_each_field_type(|t| set(t, &ids));
    }
}

/// The mode of a union whose object variants, all structs, have rules telling them apart,
/// else best match; `None` when its other variants share a JSON type or an object variant
/// is not a struct.
fn decide_objects(
    variants: &mut [UnionVariant],
    shapes: &BTreeMap<String, Option<Vec<unions::Property>>>,
) -> Option<UnionMode> {
    let (mut objects, others): (Vec<_>, Vec<_>) =
        variants.iter_mut().partition(|v| v.json_type == "object");
    let others: Vec<UnionVariant> = others.into_iter().map(|v| v.clone()).collect();
    if objects.len() < 2 || !distinct_json_types(&others) {
        return None;
    }
    let object_shapes = objects
        .iter()
        .map(|v| match &v.r#type {
            FieldType::SchemaRef { name, .. } => shapes.get(name).cloned().flatten(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    match unions::infer(&object_shapes) {
        Some(rules) => {
            for (variant, (rank, when)) in objects.iter_mut().zip(rules) {
                variant.rank = rank;
                variant.when = when;
            }
            Some(UnionMode::Rules)
        }
        None => {
            for (variant, shape) in objects.iter_mut().zip(&object_shapes) {
                (variant.required, variant.properties) = unions::best_match(shape);
            }
            Some(UnionMode::BestMatch)
        }
    }
}

/// Sets the `discriminator_defaults` of the structs that are variants of internally tagged unions.
pub(crate) fn set_discriminator_defaults(types: &mut Types) {
    let mut values: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for ty in types.values() {
        let TypeData::StructEnum {
            discriminator_field,
            repr: StructEnumRepr::InternallyTagged { variants },
            ..
        } = &ty.data
        else {
            continue;
        };
        for variant in variants {
            if let EnumVariantType::Ref {
                schema_ref: Some(target),
                ..
            } = &variant.content
            {
                values
                    .entry((target.clone(), discriminator_field.clone()))
                    .or_default()
                    .insert(variant.name.clone());
            }
        }
    }
    for ((target, field), values) in values {
        let Some(ty) = types.get_mut(&target) else {
            continue;
        };
        let TypeData::Struct { fields } = &ty.data else {
            continue;
        };
        let own = fields.iter().any(|f| f.name == field && !f.flatten);
        if let (true, [value]) = (own, &values.into_iter().collect::<Vec<_>>()[..]) {
            ty.discriminator_defaults.insert(field, value.clone());
        }
    }
}

/// Types string enums whose values would share an identifier, such as `bps` and `Bps`, as
/// strings: no SDK could name both members.
pub(crate) fn untype_clashing_enums(types: &mut Types) {
    let mut untyped = false;
    for (name, ty) in types.iter_mut() {
        let TypeData::StringEnum { values } = &ty.data else {
            continue;
        };
        let owner = format!("schema `{name}`");
        let clash = ["pascal", "shouty", "snake"]
            .into_iter()
            .find_map(|case| crate::template::ident::idents(values, case, None, &owner).err());
        if let Some(error) = clash {
            let _span = tracing::warn_span!("schema", name = %name).entered();
            let detail = error.detail().unwrap_or_default();
            let detail = detail.split(": ").nth(1).unwrap_or(detail);
            tracing::warn!(
                "{}, so the enum is typed as a string",
                detail.split(',').next().unwrap_or(detail)
            );
            ty.data = TypeData::StringAlias;
            untyped = true;
        }
    }
    if untyped {
        resolve_schema_refs(types);
    }
}

/// Makes `readOnly` fields optional in the schemas sent in requests, and `writeOnly` ones in
/// the schemas received in responses, so that callers need not invent the server's values.
pub(crate) fn relax_access_modes<'a>(
    types: &mut Types,
    requests: impl IntoIterator<Item = &'a str>,
    responses: impl IntoIterator<Item = &'a str>,
) {
    let reachable = |types: &Types, roots: Vec<String>| {
        let mut seen = BTreeSet::new();
        let mut stack = roots;
        while let Some(name) = stack.pop() {
            if let Some(ty) = types.get(&name)
                && seen.insert(name)
            {
                let refs = ty
                    .referenced_components()
                    .into_iter()
                    .chain(ty.union_refs());
                stack.extend(refs.map(str::to_owned));
            }
        }
        seen
    };
    let requests = reachable(types, requests.into_iter().map(str::to_owned).collect());
    let responses = reachable(types, responses.into_iter().map(str::to_owned).collect());
    for (name, ty) in types.iter_mut() {
        let (sent, received) = (requests.contains(name), responses.contains(name));
        ty.data.for_each_field(|field| {
            if (field.read_only && sent) || (field.write_only && received) {
                field.required = false;
            }
        });
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
                discriminator_defaults: BTreeMap::new(),
                data: TypeData::StringAlias,
            });
        }
        FieldType::List { inner } | FieldType::Set { inner } => {
            resolve_schema_ref_in_field_type(Arc::make_mut(inner), string_alias_names);
        }
        FieldType::Map { value_ty } => {
            resolve_schema_ref_in_field_type(Arc::make_mut(value_ty), string_alias_names);
        }
        FieldType::Union { variants, .. } => {
            for variant in variants {
                resolve_schema_ref_in_field_type(&mut variant.r#type, string_alias_names);
            }
        }
        _ => {}
    }
}

/// See [`super::Api::settle_object_unions`].
pub(crate) fn settle_object_unions(types: &mut Types, best_match: bool) -> (usize, usize) {
    let mut counts = (0, 0);
    for ty in types.values_mut() {
        ty.data
            .for_each_field_type(|t| t.settle_object_unions(best_match, &mut counts));
    }
    counts
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
        for inner in source
            .referenced_schema()
            .into_iter()
            .chain(source.union_refs())
        {
            if raw.contains_key(inner) {
                resolve(inner, raw, resolved, stack)?;
            }
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
    reserved: &BTreeSet<String>,
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
        reserved: reserved.clone(),
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
    /// Names the SDK already uses, suffixed with `Model` as schemas named so are.
    reserved: BTreeSet<String>,
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
            let mut base = match title.take() {
                Some(t) => t.to_upper_camel_case(),
                None => base_name.to_upper_camel_case(),
            };
            if existing.reserved.contains(&base) {
                base.push_str("Model");
            }
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
                discriminator_defaults: BTreeMap::new(),
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
        | FieldType::Union { .. }
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
    /// Wire value of a field telling this struct apart in the unions it is a variant of, such as
    /// `{"type": "circle"}`, when every such union agrees: constructors can default it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) discriminator_defaults: BTreeMap<String, String>,
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
            description: super::html::doc(metadata.description.clone()),
            deprecated: metadata.deprecated,
            discriminator_defaults: BTreeMap::new(),
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

    /// Schemas referenced by the variants of its unions, which [`Self::referenced_components`]
    /// leaves out for templates rendering unions as untyped JSON.
    pub(crate) fn union_refs(&self) -> BTreeSet<&str> {
        self.data
            .field_types()
            .into_iter()
            .flat_map(FieldType::union_refs)
            .collect()
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
    fn field_types(&self) -> Vec<&FieldType> {
        match self {
            Self::Alias { target } => vec![&**target],
            Self::Struct { fields } => fields.iter().map(|f| &f.r#type).collect(),
            Self::StructEnum { fields, repr, .. } => {
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                let own = fields.iter().map(|f| &f.r#type);
                let nested = variants.iter().flat_map(|v| match &v.content {
                    EnumVariantType::Struct { fields } => {
                        fields.iter().map(|f| &f.r#type).collect()
                    }
                    EnumVariantType::Ref { .. } => Vec::new(),
                });
                own.chain(nested).collect()
            }
            Self::StringEnum { .. } | Self::IntegerEnum { .. } | Self::StringAlias => Vec::new(),
        }
    }

    fn for_each_field(&mut self, mut visit: impl FnMut(&mut Field)) {
        match self {
            Self::Struct { fields } => fields.iter_mut().for_each(visit),
            Self::StructEnum { fields, repr, .. } => {
                fields.iter_mut().for_each(&mut visit);
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                for variant in variants {
                    if let EnumVariantType::Struct { fields } = &mut variant.content {
                        fields.iter_mut().for_each(&mut visit);
                    }
                }
            }
            Self::Alias { .. }
            | Self::StringEnum { .. }
            | Self::IntegerEnum { .. }
            | Self::StringAlias => {}
        }
    }

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
                constant: None,
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
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .unique()
                .collect(),
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
    pub(crate) required: bool,
    pub(crate) nullable: bool,
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
    pub(crate) flatten: bool,
    /// The only value of the field (`const` or a single-value `enum`), which can tell apart
    /// the variants of a union.
    #[serde(skip)]
    pub(crate) constant: Option<serde_json::Value>,
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
        let constant = obj
            .const_value
            .clone()
            .or_else(|| match obj.enum_values.as_deref() {
                Some([value]) => Some(value.clone()),
                _ => None,
            })
            .filter(|v| v.is_string() || v.is_boolean() || v.is_i64() || v.is_u64());

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
            description: super::html::doc(metadata.description),
            required,
            nullable,
            deprecated: metadata.deprecated,
            example,
            read_only: metadata.read_only,
            write_only: metadata.write_only,
            flatten: false,
            constant: constant.filter(|_| !nullable),
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

/// A member of a [`FieldType::Union`].
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(crate) struct UnionVariant {
    /// Unique in its union: the snake_case schema name of a reference, else `string`,
    /// `integer`, `number`, `boolean`, `date_time`, `decimal`, `list`, `object` or `empty`.
    pub name: String,
    /// JSON type deciding the variant when decoding: `string`, `integer`, `number`, `boolean`,
    /// `array` or `object`. No two variants of a union share one (`integer` and `number` count
    /// as one).
    pub json_type: String,
    /// Only the empty string, which Stripe accepts to unset a value (`enum: [""]`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
    #[serde(serialize_with = "serialize_field_type")]
    pub r#type: FieldType,
    /// Conditions an object must meet to be this variant, in a union of several objects
    /// decided by rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
    /// Order in which the `when` conditions of the object variants are checked.
    #[serde(default, skip_serializing)]
    pub rank: usize,
    /// Required properties, in a union of several objects decided by best match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Every declared property, in a union of several objects decided by best match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<String>,
    /// `required` or `optional` when the variant is a struct with a string `id`, which
    /// expandable unions (`string | Customer`) return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl UnionVariant {
    fn from_schema(schema: &Schema) -> Option<Self> {
        let Schema::Object(obj) = schema else {
            return None;
        };
        let empty = [serde_json::Value::String(String::new())];
        if obj.enum_values.as_deref() == Some(&empty) {
            return Some(Self {
                name: "empty".into(),
                json_type: "string".into(),
                empty: true,
                r#type: FieldType::String,
                when: Vec::new(),
                rank: 0,
                required: Vec::new(),
                properties: Vec::new(),
                id: None,
            });
        }
        let explicit_object = obj.instance_type == Some(InstanceType::Object.into());
        let r#type = match FieldType::from_schema_object(obj.clone()).ok()? {
            FieldType::StringEnum { .. } => FieldType::String,
            FieldType::JsonObject if !explicit_object => return None,
            ty => ty,
        };
        let (name, json_type) = match &r#type {
            FieldType::SchemaRef { name, .. } => (name.to_snake_case(), String::new()),
            ty => (
                Self::name_of(ty)?.to_owned(),
                Self::json_type_of(ty)?.to_owned(),
            ),
        };
        Some(Self {
            name,
            json_type,
            empty: false,
            r#type,
            when: Vec::new(),
            rank: 0,
            required: Vec::new(),
            properties: Vec::new(),
            id: None,
        })
    }

    fn name_of(ty: &FieldType) -> Option<&'static str> {
        Some(match ty {
            FieldType::Bool => "boolean",
            FieldType::Int16
            | FieldType::UInt16
            | FieldType::Int32
            | FieldType::Int64
            | FieldType::UInt64 => "integer",
            FieldType::Float | FieldType::Double => "number",
            FieldType::String | FieldType::Uri | FieldType::Date => "string",
            FieldType::DateTime => "date_time",
            FieldType::Decimal => "decimal",
            FieldType::List { .. } | FieldType::Set { .. } => "list",
            FieldType::Map { .. } | FieldType::JsonObject => "object",
            FieldType::SchemaRef { .. }
            | FieldType::Union { .. }
            | FieldType::StringEnum { .. } => {
                return None;
            }
        })
    }

    /// The JSON type of values of `ty`, unknown for references.
    fn json_type_of(ty: &FieldType) -> Option<&'static str> {
        Some(match ty {
            FieldType::Bool => "boolean",
            FieldType::Int16
            | FieldType::UInt16
            | FieldType::Int32
            | FieldType::Int64
            | FieldType::UInt64 => "integer",
            FieldType::Float | FieldType::Double => "number",
            FieldType::String
            | FieldType::Uri
            | FieldType::Date
            | FieldType::DateTime
            | FieldType::Decimal
            | FieldType::StringEnum { .. } => "string",
            FieldType::List { .. } | FieldType::Set { .. } => "array",
            FieldType::Map { .. } | FieldType::JsonObject => "object",
            FieldType::SchemaRef { .. } | FieldType::Union { .. } => return None,
        })
    }
}

/// Whether variants are told apart by their JSON types, those of references being unknown yet.
fn distinct_json_types(variants: &[UnionVariant]) -> bool {
    let group = |t: &str| {
        if t == "integer" {
            "number".to_owned()
        } else {
            t.to_owned()
        }
    };
    let known: Vec<String> = variants
        .iter()
        .filter(|v| !v.json_type.is_empty())
        .map(|v| group(&v.json_type))
        .collect();
    known.iter().all_unique() && variants.iter().map(|v| &v.name).all_unique()
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

    /// A value of one of several types told apart by their JSON type, such as Stripe's
    /// expandable `string | Customer`. Templates render it as untyped JSON (it answers
    /// `is_json_object`) unless they check `is_union` first.
    Union {
        variants: Vec<UnionVariant>,
        /// How the variant of an object is picked when several variants are objects.
        #[serde(default, skip_serializing_if = "UnionMode::is_json")]
        mode: UnionMode,
        /// The mode `x-perseid-union` asks for.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<UnionMode>,
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
                let nullable = variants.iter().any(is_null_schema);
                let requested = obj
                    .extensions
                    .get("x-perseid-union")
                    .and_then(UnionMode::from_extension);
                let members: Option<Vec<UnionVariant>> = variants
                    .iter()
                    .filter(|v| !is_null_schema(v))
                    .map(UnionVariant::from_schema)
                    .collect();
                if let Some(variants) = members
                    && variants.len() > 1
                    && distinct_json_types(&variants)
                {
                    return Ok((
                        Self::Union {
                            variants,
                            mode: UnionMode::Json,
                            requested,
                        },
                        nullable,
                    ));
                }
                tracing::warn!(
                    "`oneOf`/`anyOf` without a discriminator is typed as an untyped JSON value"
                );
                return Ok((Self::JsonObject, nullable));
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
            Self::JsonObject | Self::Union { .. } => "JsonNode".into(),
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
            Self::JsonObject | Self::Union { .. } => "map[string]any".into(),
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
            Self::JsonObject | Self::Union { .. } => "Map<String,Any>".into(),
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
            Self::JsonObject | Self::Union { .. } => "any".into(),
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
            Self::JsonObject | Self::Union { .. } => "serde_json::Value".into(),
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
            | Self::JsonObject
            | Self::Union { .. } => true,
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
            Self::Union { variants, .. } => {
                for variant in variants.iter_mut() {
                    variant.r#type.inline_aliases(aliases);
                }
                let untyped = variants.iter().any(|v| {
                    matches!(v.r#type, Self::JsonObject | Self::Union { .. })
                        && v.json_type != "object"
                });
                if untyped {
                    *self = Self::JsonObject;
                }
            }
            _ => {}
        }
    }

    /// Schemas the variants of the unions in this type reference.
    pub(crate) fn union_refs(&self) -> BTreeSet<&str> {
        match self {
            Self::Union { variants, .. } => variants
                .iter()
                .flat_map(|v| {
                    let mut refs = v.r#type.union_refs();
                    refs.extend(v.r#type.referenced_schema());
                    refs
                })
                .collect(),
            Self::List { inner } | Self::Set { inner } => inner.union_refs(),
            Self::Map { value_ty } => value_ty.union_refs(),
            _ => BTreeSet::new(),
        }
    }

    fn settle_object_unions(&mut self, best_match: bool, counts: &mut (usize, usize)) {
        match self {
            Self::Union {
                variants,
                mode,
                requested,
            } => {
                for variant in variants.iter_mut() {
                    variant.r#type.settle_object_unions(best_match, counts);
                }
                let wanted = best_match || *requested == Some(UnionMode::BestMatch);
                if *mode == UnionMode::BestMatch {
                    if wanted {
                        counts.0 += 1;
                    } else {
                        counts.1 += 1;
                        *self = Self::JsonObject;
                    }
                }
            }
            Self::List { inner } | Self::Set { inner } => {
                Arc::make_mut(inner).settle_object_unions(best_match, counts)
            }
            Self::Map { value_ty } => {
                Arc::make_mut(value_ty).settle_object_unions(best_match, counts)
            }
            _ => {}
        }
    }

    /// This type with its unions typed as untyped JSON.
    pub(crate) fn untype_unions(&mut self) {
        match self {
            Self::Union { .. } => *self = Self::JsonObject,
            Self::List { inner } | Self::Set { inner } => Arc::make_mut(inner).untype_unions(),
            Self::Map { value_ty } => Arc::make_mut(value_ty).untype_unions(),
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
            Self::JsonObject | Self::Union { .. } => "t.Dict[str, t.Any]".into(),
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
            FieldType::JsonObject | FieldType::Union { .. } => "Object".into(),
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
            | FieldType::Union { .. }
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
            | FieldType::Union { .. }
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
            | FieldType::Union { .. }
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
                    | F::Union { .. }
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
                Ok(matches!(**self, Self::JsonObject | Self::Union { .. }).into())
            }
            "is_union" => {
                ensure_no_args(args, "is_union")?;
                Ok(matches!(**self, Self::Union { .. }).into())
            }
            "union_mode" => {
                ensure_no_args(args, "union_mode")?;
                Ok(match &**self {
                    Self::Union { mode, .. } => minijinja::Value::from_serialize(mode),
                    _ => minijinja::Value::from(()),
                })
            }
            "object_variants" => {
                ensure_no_args(args, "object_variants")?;
                Ok(match &**self {
                    Self::Union { variants, .. } => {
                        let mut objects: Vec<_> = variants
                            .iter()
                            .filter(|v| v.json_type == "object")
                            .collect();
                        objects.sort_by_key(|v| v.rank);
                        minijinja::Value::from_serialize(objects)
                    }
                    _ => minijinja::Value::from(Vec::<minijinja::Value>::new()),
                })
            }
            "union_variants" => {
                ensure_no_args(args, "union_variants")?;
                Ok(match &**self {
                    Self::Union { variants, .. } => minijinja::Value::from_serialize(variants),
                    _ => minijinja::Value::from(Vec::<minijinja::Value>::new()),
                })
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

    fn variants(ty: &FieldType) -> Vec<(&str, &str)> {
        let FieldType::Union { variants, .. } = ty else {
            panic!("{ty:?} is not a union");
        };
        variants
            .iter()
            .map(|v| (v.name.as_str(), v.json_type.as_str()))
            .collect()
    }

    #[test]
    fn unions_told_apart_by_json_type_are_typed() {
        let f = field(json!({"oneOf": [{"$ref": "#/components/schemas/A"}, {"type": "string"}]}));
        assert_eq!(variants(&f.r#type), [("a", ""), ("string", "string")]);
        let f =
            field(json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]}));
        assert_eq!(
            variants(&f.r#type),
            [("string", "string"), ("integer", "integer")]
        );
        assert!(f.nullable);
        let emptyable = json!({"anyOf": [{"type": "object", "additionalProperties": {"type": "string"}}, {"type": "string", "enum": [""]}]});
        let FieldType::Union { variants, .. } = field(emptyable).r#type else {
            panic!("not a union");
        };
        assert_eq!(
            (variants[0].name.as_str(), variants[0].json_type.as_str()),
            ("object", "object")
        );
        assert!(variants[1].empty && variants[1].json_type == "string");
    }

    #[test]
    fn ambiguous_unions_are_untyped_json() {
        let f = field(
            json!({"anyOf": [{"type": "string"}, {"type": "string", "format": "date-time"}]}),
        );
        assert_eq!(f.r#type, FieldType::JsonObject);
        let f = field(json!({"anyOf": [{"type": "integer"}, {"type": "number"}]}));
        assert_eq!(f.r#type, FieldType::JsonObject);
        let f = field(json!({"anyOf": [{"type": "string"}, {}]}));
        assert_eq!(f.r#type, FieldType::JsonObject);
        let f = field(json!({"type": ["string", "integer"]}));
        assert_eq!(f.r#type, FieldType::JsonObject);
    }

    #[test]
    fn union_references_settle_their_json_type_or_untype_the_union() {
        let customer = |a: &str, b: &str| {
            json!({"anyOf": [
                {"$ref": format!("#/components/schemas/{a}")},
                {"$ref": format!("#/components/schemas/{b}")},
                {"type": "string"}
            ]})
        };
        let mut types = types_from(json!({
            "Charge": {"type": "object", "properties": {
                "customer": customer("Customer", "Tier"),
                "both": customer("Customer", "Deleted"),
                "code": customer("Code", "Customer")
            }},
            "Customer": {"type": "object", "properties": {"id": {"type": "string"}}},
            "Deleted": {"type": "object", "properties": {"id": {"type": "string"}}},
            "Tier": {"type": "integer", "enum": [1, 2]},
            "Code": {"type": "string", "enum": ["a", "b"]}
        }));
        let mut resources = Resources::new();
        resolve_unions(&mut types, &mut resources);
        assert_eq!(
            variants(field_type(&types, "Charge", "customer")),
            [
                ("customer", "object"),
                ("tier", "integer"),
                ("string", "string")
            ]
        );
        assert!(matches!(
            field_type(&types, "Charge", "both"),
            FieldType::Union {
                mode: UnionMode::BestMatch,
                ..
            }
        ));
        assert_eq!(field_type(&types, "Charge", "code"), &FieldType::JsonObject);
        assert_eq!(
            types["Charge"].union_refs(),
            BTreeSet::from(["Customer", "Deleted", "Tier"])
        );
        assert!(types["Charge"].referenced_components().is_empty());
        assert_eq!(settle_object_unions(&mut types, false), (0, 1));
        assert_eq!(field_type(&types, "Charge", "both"), &FieldType::JsonObject);
    }

    fn object_unions() -> Types {
        let mut types = types_from(json!({
            "Charge": {"type": "object", "properties": {
                "customer": {"anyOf": [
                    {"type": "string"},
                    {"$ref": "#/components/schemas/Customer"},
                    {"$ref": "#/components/schemas/DeletedCustomer"}
                ]},
                "source": {"oneOf": [
                    {"$ref": "#/components/schemas/UrlSource"},
                    {"$ref": "#/components/schemas/FileSource"}
                ]},
                "doc": {"oneOf": [
                    {"$ref": "#/components/schemas/Draft"},
                    {"$ref": "#/components/schemas/Article"}
                ]},
                "pinned": {"x-perseid-union": "best-match", "oneOf": [
                    {"$ref": "#/components/schemas/Draft"},
                    {"$ref": "#/components/schemas/Article"}
                ]},
                "raw": {"x-perseid-union": "json", "oneOf": [
                    {"$ref": "#/components/schemas/UrlSource"},
                    {"$ref": "#/components/schemas/FileSource"}
                ]}
            }},
            "Customer": {"type": "object", "required": ["id", "object"], "properties": {
                "id": {"type": "string"},
                "object": {"type": "string", "enum": ["customer"]},
                "email": {"type": "string"}
            }},
            "DeletedCustomer": {"type": "object", "required": ["deleted", "id", "object"], "properties": {
                "deleted": {"type": "boolean", "enum": [true]},
                "id": {"type": "string"},
                "object": {"type": "string", "const": "customer"}
            }},
            "UrlSource": {"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}},
            "FileSource": {"type": "object", "required": ["file_id"], "properties": {"file_id": {"type": "string"}}},
            "Draft": {"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}}},
            "Article": {"type": "object", "required": ["title"], "properties": {
                "title": {"type": "string"}, "author": {"type": "string"}
            }}
        }));
        resolve_unions(&mut types, &mut Resources::new());
        set_union_ids(&mut types);
        types
    }

    type Rule<'a> = (
        &'a str,
        usize,
        Vec<(&'a str, Option<&'a serde_json::Value>)>,
    );

    fn rules(ty: &FieldType) -> Vec<Rule<'_>> {
        let FieldType::Union { variants, mode, .. } = ty else {
            panic!("{ty:?} is not a union");
        };
        assert_eq!(*mode, UnionMode::Rules);
        variants
            .iter()
            .filter(|v| v.json_type == "object")
            .map(|v| {
                let when = v
                    .when
                    .iter()
                    .map(|c| (c.property.as_str(), c.value.as_ref()));
                (v.name.as_str(), v.rank, when.collect())
            })
            .collect()
    }

    #[test]
    fn object_variants_are_told_apart_by_constants_and_required_properties() {
        let types = object_unions();
        let (customer, deleted) = (json!("customer"), json!(true));
        assert_eq!(
            rules(field_type(&types, "Charge", "customer")),
            [
                ("customer", 1, vec![("object", Some(&customer))]),
                ("deleted_customer", 0, vec![("deleted", Some(&deleted))]),
            ]
        );
        assert_eq!(
            rules(field_type(&types, "Charge", "source")),
            [
                ("url_source", 0, vec![("url", None)]),
                ("file_source", 1, vec![("file_id", None)]),
            ]
        );
        let FieldType::Union { variants, .. } = field_type(&types, "Charge", "customer") else {
            unreachable!()
        };
        assert_eq!(variants[1].id.as_deref(), Some("required"));
    }

    #[test]
    fn undecidable_object_unions_follow_the_setting_or_their_extension() {
        let best_match = |types: &Types, field| {
            matches!(
                field_type(types, "Charge", field),
                FieldType::Union {
                    mode: UnionMode::BestMatch,
                    ..
                }
            )
        };
        let mut types = object_unions();
        assert!(best_match(&types, "doc") && best_match(&types, "pinned"));
        assert_eq!(field_type(&types, "Charge", "raw"), &FieldType::JsonObject);
        let FieldType::Union { variants, .. } = field_type(&types, "Charge", "doc") else {
            unreachable!()
        };
        assert_eq!(variants[1].required, ["title"]);
        assert_eq!(variants[1].properties, ["author", "title"]);

        let mut json = types.clone();
        assert_eq!(settle_object_unions(&mut json, false), (1, 1));
        assert_eq!(field_type(&json, "Charge", "doc"), &FieldType::JsonObject);
        assert!(best_match(&json, "pinned"));
        assert_eq!(settle_object_unions(&mut types, true), (2, 0));
        assert!(best_match(&types, "doc"));
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
    fn enums_whose_values_clash_are_strings_and_duplicates_are_dropped() {
        let mut types = types_from(json!({
            "Unit": {"type": "string", "enum": ["bps", "Bps"]},
            "Zone": {"type": "string", "enum": ["Etc/GMT-0", "Etc/GMT0"]},
            "Operator": {"type": "string", "enum": ["lt", "gt", "lt"]},
            "Reading": {"type": "object", "properties": {"unit": {"$ref": "#/components/schemas/Unit"}}}
        }));
        untype_clashing_enums(&mut types);
        assert_eq!(types["Unit"].data, TypeData::StringAlias);
        assert_eq!(types["Zone"].data, TypeData::StringAlias);
        assert_eq!(
            types["Operator"].data,
            TypeData::StringEnum {
                values: vec!["lt".into(), "gt".into()]
            }
        );
        let FieldType::SchemaRef { inner, .. } = field_type(&types, "Reading", "unit") else {
            panic!("a reference")
        };
        assert_eq!(
            inner.as_ref().map(|t| &t.data),
            Some(&TypeData::StringAlias)
        );
    }

    #[test]
    fn read_only_fields_are_optional_in_requests_and_write_only_ones_in_responses() {
        let mut types = types_from(json!({
            "Account": {"type": "object", "required": ["id", "password", "owner"], "properties": {
                "id": {"type": "string", "readOnly": true},
                "password": {"type": "string", "writeOnly": true},
                "owner": {"$ref": "#/components/schemas/Owner"}
            }},
            "Owner": {"type": "object", "required": ["id"], "properties": {
                "id": {"type": "string", "readOnly": true}
            }},
            "Receipt": {"type": "object", "required": ["id"], "properties": {
                "id": {"type": "string", "readOnly": true}
            }}
        }));
        relax_access_modes(&mut types, ["Account"], ["Account", "Receipt"]);
        let required = |types: &Types, ty: &str, name: &str| {
            let TypeData::Struct { fields } = &types[ty].data else {
                panic!("a struct")
            };
            fields.iter().find(|f| f.name == name).unwrap().required
        };
        assert!(!required(&types, "Account", "id"));
        assert!(!required(&types, "Account", "password"));
        assert!(required(&types, "Account", "owner"));
        assert!(!required(&types, "Owner", "id"));
        assert!(required(&types, "Receipt", "id"));
    }

    #[test]
    fn promoted_enums_avoid_names_that_differ_only_in_case() {
        let mut types = types_from(json!({
            "thing_kind": {"type": "string"},
            "Thing": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["a", "b"]}}}
        }));
        promote_inline_enums(&mut types, &mut Resources::new(), &BTreeSet::new()).unwrap();
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
