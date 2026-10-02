pub(crate) mod html;
pub(crate) mod naming;
pub(crate) mod pagination;
pub(crate) mod resources;
pub(crate) mod security;
pub(crate) mod struct_enum;
pub(crate) mod types;
pub(crate) mod unions;

use aide::openapi;
use anyhow::ensure;
use serde::{Deserialize, Serialize};

use crate::spec::Filters;

pub(crate) use self::{
    resources::{Resource, Resources},
    types::{FieldType, Types},
};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct Api {
    #[serde(with = "toplevel_resources_serde")]
    pub resources: Resources,
    pub types: Types,
    #[serde(default)]
    pub security_schemes: Vec<security::SecurityScheme>,
    /// Requirement of every operation that does not declare its own `security`.
    #[serde(default)]
    pub security: security::Requirement,
    /// Distinct schemas of the JSON bodies of error responses.
    #[serde(default)]
    pub error_schemas: Vec<String>,
    /// The error schema of (nearly) every operation declaring error responses, which SDKs can
    /// decode any API error with.
    #[serde(default)]
    pub default_error: Option<String>,
}

impl Api {
    pub(crate) fn new(
        paths: openapi::Paths,
        components: &mut openapi::Components,
        webhooks: &[String],
        raw_spec: &serde_json::Value,
        filters: &Filters,
    ) -> anyhow::Result<Self> {
        let include_mode = filters.include_mode;
        let (mut resources, mut errors) = resources::from_openapi(
            paths,
            &components.schemas,
            include_mode,
            &filters.excluded,
            &filters.specified,
        );
        if let Err(e) = naming::apply(&mut resources, &filters.names) {
            errors.push(format!("{e:#}"));
        }
        let (mut types, type_errors) = types::from_referenced_components(
            &resources,
            &mut components.schemas,
            webhooks,
            include_mode,
        );
        errors.extend(type_errors);
        types::untag_unions_with_non_object_variants(&mut types);
        types::resolve_unions(&mut types, &mut resources);
        let (requests, responses) = resources::request_and_response_roots(&resources);
        let responses = responses
            .into_iter()
            .chain(webhooks.iter().map(String::as_str));
        types::relax_access_modes(&mut types, requests, responses);
        types::set_union_ids(&mut types);

        // Promote inline enums (e.g. array-of-enum query params) to named
        // top-level types so generated SDKs get real enum types instead of
        // `Vec<String>`. Must run before we collect string alias names, since
        // promotion may add new `SchemaRef`s that need resolving.
        if let Err(e) = types::promote_inline_enums(&mut types, &mut resources, &filters.reserved) {
            errors.push(format!("{e:#}"));
        }
        errors.extend(types::clashing_type_names(&types));
        errors.extend(types::clashing_identifiers(&types));
        ensure!(
            errors.is_empty(),
            "the spec uses {} perseid does not support (skip an operation with \
             `exclude = [\"<operation id>\"]` in perseid.toml):\n  - {}",
            match errors.len() {
                1 => "a construct".to_owned(),
                n => format!("{n} constructs"),
            },
            errors.join("\n  - ")
        );

        resources::rename_resources_named_like_types(&mut resources, &types, &filters.reserved);
        resources::mark_structured_query_params(&mut resources, &types);
        resources::name_body_unions(&mut resources, &types);

        // Resolve string alias references in operation query params
        // This must happen after types are created so we know which types are string aliases
        let string_alias_names = types::collect_string_alias_names(&types);
        resources::resolve_schema_refs_in_resources(&mut resources, &string_alias_names);

        types::set_discriminator_defaults(&mut types);

        let security = security::Security::from_spec(raw_spec);
        for resource in resources.values_mut() {
            resource.resolve_extensions(&security, &filters.pagination, &types)?;
        }

        let mut api = Self {
            resources,
            types,
            security_schemes: security.schemes,
            security: security.default,
            error_schemas: Vec::new(),
            default_error: None,
        };
        api.collect_errors();
        Ok(api)
    }

    fn collect_errors(&mut self) {
        let mut stack: Vec<&Resource> = self.resources.values().collect();
        let mut uses = std::collections::BTreeMap::<&str, usize>::new();
        let mut successes = std::collections::BTreeMap::<&str, usize>::new();
        let mut declaring = 0;
        while let Some(resource) = stack.pop() {
            stack.extend(resource.subresources.values());
            for op in &resource.operations {
                if let Some(schema) = op.response_schema() {
                    *successes.entry(schema).or_default() += 1;
                }
                if op.errors.is_empty() {
                    continue;
                }
                declaring += 1;
                let schemas: std::collections::BTreeSet<&str> =
                    op.errors.values().map(String::as_str).collect();
                for schema in schemas {
                    *uses.entry(schema).or_default() += 1;
                }
            }
        }
        self.error_schemas = uses.keys().map(|s| (*s).to_owned()).collect();
        self.default_error = infer_default_error(&uses, &successes, declaring);
    }

    pub(crate) fn inline_aliases(&mut self) -> anyhow::Result<()> {
        types::inline_aliases(&mut self.types, &mut self.resources)?;
        self.collect_errors();
        Ok(())
    }

    /// Settles the unions of several objects for the SDK of context `sdk`: those without rules
    /// are untyped JSON unless best match is on. Returns how many are decided by best match
    /// and how many best match would type.
    pub(crate) fn settle_object_unions(&mut self, sdk: &serde_json::Value) -> (usize, usize) {
        let best_match = sdk["untagged_unions"].as_str() != Some("json");
        let mut counts = types::settle_object_unions(&mut self.types, best_match);
        let mut stack: Vec<&mut resources::Resource> = self.resources.values_mut().collect();
        while let Some(resource) = stack.pop() {
            for op in &mut resource.operations {
                op.settle_object_unions(best_match, &mut counts);
            }
            stack.extend(resource.subresources.values_mut());
        }
        counts
    }

    /// Declares the inline object variants of tagged unions as structs, for Go.
    pub(crate) fn hoist_inline_variants(&mut self) {
        types::hoist_inline_variants(&mut self.types);
    }

    pub(crate) fn inline_flattened_fields(&mut self) -> anyhow::Result<()> {
        types::inline_flattened_fields(&mut self.types)
    }

    /// Types string-alias bodies and parameters as plain strings, for Java.
    pub(crate) fn inline_string_alias_bodies(&mut self) -> anyhow::Result<()> {
        let aliases = types::collect_string_alias_names(&self.types)
            .into_iter()
            .map(|name| (name, FieldType::String))
            .collect();
        for resource in self.resources.values_mut() {
            resource.inline_aliases(&aliases)?;
        }
        Ok(())
    }
}

/// Fewest operations that must share an error schema for it to type every API error.
const MIN_DEFAULT_ERROR_USES: usize = 3;

/// The error schema shared by a clear majority of the operations declaring errors, and by at
/// least a few of them, so that one operation declaring `404: Item` never types the errors of
/// the others. A schema that is also a success body must beat those uses clearly.
fn infer_default_error(
    uses: &std::collections::BTreeMap<&str, usize>,
    successes: &std::collections::BTreeMap<&str, usize>,
    declaring: usize,
) -> Option<String> {
    uses.iter()
        .rev()
        .max_by_key(|(_, count)| **count)
        .filter(|(schema, count)| {
            let success = successes.get(**schema).copied().unwrap_or(0);
            **count >= MIN_DEFAULT_ERROR_USES
                && **count * 10 >= declaring * 9
                && (success == 0 || **count > success * 2)
        })
        .map(|(schema, _)| (*schema).to_owned())
}

pub(crate) fn get_schema_name(maybe_ref: Option<&str>) -> Option<String> {
    let r = maybe_ref?;
    let schema_name = r.strip_prefix("#/components/schemas/");
    if schema_name.is_none() {
        tracing::error!(
            component_ref = r,
            "missing #/components/schemas/ prefix on component ref"
        );
    };
    Some(schema_name?.to_owned())
}

pub(crate) mod toplevel_resources_serde {
    use std::fmt;

    use serde::{
        de::{Deserializer, SeqAccess, Visitor},
        ser::{SerializeSeq as _, Serializer},
    };

    use super::{Resource, Resources};

    pub(crate) fn serialize<S>(map: &Resources, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(map.len()))?;
        for item in map.values() {
            seq.serialize_element(item)?;
        }
        seq.end()
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Resources, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ToplevelResourcesVisitor;

        impl<'de> Visitor<'de> for ToplevelResourcesVisitor {
            type Value = Resources;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a list of resources")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut resources = Resources::new();
                while let Some(r) = seq.next_element::<Resource>()? {
                    resources.insert(r.name.clone(), r);
                }
                Ok(resources)
            }
        }

        deserializer.deserialize_seq(ToplevelResourcesVisitor)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::infer_default_error;

    #[test]
    fn a_lone_error_schema_is_not_the_default() {
        let uses = BTreeMap::from([("Item", 1)]);
        assert_eq!(infer_default_error(&uses, &BTreeMap::new(), 1), None);
        let uses = BTreeMap::from([("Error", 2)]);
        assert_eq!(infer_default_error(&uses, &BTreeMap::new(), 2), None);
    }

    #[test]
    fn a_shared_error_schema_is_the_default() {
        let uses = BTreeMap::from([("Error", 5)]);
        let none = BTreeMap::new();
        assert_eq!(
            infer_default_error(&uses, &none, 5).as_deref(),
            Some("Error")
        );
        // One of the operations declaring errors uses another schema.
        let uses = BTreeMap::from([("Error", 5), ("Other", 1)]);
        assert_eq!(infer_default_error(&uses, &none, 6), None);
    }

    #[test]
    fn a_success_body_is_not_the_default_unless_clearly_an_error() {
        let uses = BTreeMap::from([("Item", 4)]);
        let successes = BTreeMap::from([("Item", 3)]);
        assert_eq!(infer_default_error(&uses, &successes, 4), None);
        let successes = BTreeMap::from([("Item", 1)]);
        assert_eq!(
            infer_default_error(&uses, &successes, 4).as_deref(),
            Some("Item")
        );
    }
}
