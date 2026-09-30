pub(crate) mod pagination;
pub(crate) mod resources;
pub(crate) mod security;
pub(crate) mod struct_enum;
pub(crate) mod types;

use aide::openapi;
use serde::{Deserialize, Serialize};

use crate::spec::Filters;

pub(crate) use self::{
    resources::{Resource, Resources},
    types::Types,
};

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Api {
    #[serde(with = "toplevel_resources_serde")]
    pub resources: Resources,
    pub types: Types,
    #[serde(default)]
    pub security_schemes: Vec<security::SecurityScheme>,
    /// Requirement of every operation that does not declare its own `security`.
    #[serde(default)]
    pub security: security::Requirement,
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
        let mut resources = resources::from_openapi(
            paths,
            &components.schemas,
            include_mode,
            &filters.excluded,
            &filters.specified,
        )?;
        let mut types = types::from_referenced_components(
            &resources,
            &mut components.schemas,
            webhooks,
            include_mode,
        );

        // Promote inline enums (e.g. array-of-enum query params) to named
        // top-level types so generated SDKs get real enum types instead of
        // `Vec<String>`. Must run before we collect string alias names, since
        // promotion may add new `SchemaRef`s that need resolving.
        types::promote_inline_enums(&mut types, &mut resources)?;

        // Resolve string alias references in operation query params
        // This must happen after types are created so we know which types are string aliases
        let string_alias_names = types::collect_string_alias_names(&types);
        resources::resolve_schema_refs_in_resources(&mut resources, &string_alias_names);

        let security = security::Security::from_spec(raw_spec);
        for resource in resources.values_mut() {
            resource.resolve_extensions(&security, &filters.pagination, &types)?;
        }

        Ok(Self {
            resources,
            types,
            security_schemes: security.schemes,
            security: security.default,
        })
    }

    pub(crate) fn inline_aliases(&mut self) -> anyhow::Result<()> {
        types::inline_aliases(&mut self.types, &mut self.resources)
    }
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
