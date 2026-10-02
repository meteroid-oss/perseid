use anyhow::{Context as _, bail, ensure};
use schemars::schema::{ObjectValidation, Schema, SchemaObject};

use crate::api::{
    get_schema_name,
    types::{EnumVariantType, Field, SimpleVariant, StructEnumRepr, TypeData},
};

/// A wrapper around a Option<String>
///
/// Only allows value to be updated once. once updated, ant subsequent values must be the same
struct SameString(Option<String>);

impl SameString {
    fn update(&mut self, val: String) -> anyhow::Result<()> {
        match self.0.as_ref() {
            Some(current_val) => ensure!(
                *current_val == val,
                "`oneOf` variants disagree on the field name: `{current_val}` and `{val}`"
            ),
            None => self.0 = Some(val),
        }
        Ok(())
    }
    fn inner(self) -> Option<String> {
        self.0
    }
}

impl TypeData {
    pub(super) fn inline_struct_enum(
        one_of: &Vec<Schema>,
        fields: &[Field],
    ) -> anyhow::Result<Self> {
        let mut discriminator_field = SameString(None);
        let mut content_field = SameString(None);

        let mut variants = vec![];

        for s in one_of {
            let variant = get_obj_validation(s)?;

            let (variant_discriminator_name, discriminator) = get_discriminator(variant)?;
            discriminator_field.update(variant_discriminator_name)?;

            let len = variant.properties.len();
            ensure!(
                (1..=2).contains(&len),
                "an inline `oneOf` variant has {len} properties, expected 1 (the discriminator) or 2 (the discriminator and its content)"
            );
            if variant.properties.len() == 1 {
                variants.push(SimpleVariant {
                    name: discriminator,
                    content: EnumVariantType::Ref {
                        schema_ref: None,
                        inner: None,
                    },
                });
            } else {
                let (variant_content_field, content) = get_content(variant)?;
                content_field.update(variant_content_field)?;

                variants.push(SimpleVariant {
                    name: discriminator,
                    content,
                });
            }
        }

        let discriminator_field = discriminator_field
            .inner()
            .context("a `oneOf` without variants")?;
        let content_field = content_field
            .inner()
            .context("no `oneOf` variant carries content")?;
        // The shared properties may declare the discriminator and the content again: the
        // variants already carry them.
        let fields = fields
            .iter()
            .filter(|f| f.name != discriminator_field && f.name != content_field)
            .cloned()
            .collect();
        Ok(Self::StructEnum {
            discriminator_field,
            fields,
            repr: StructEnumRepr::AdjacentlyTagged {
                content_field,
                variants,
            },
        })
    }
}

fn get_content(variant: &ObjectValidation) -> anyhow::Result<(String, EnumVariantType)> {
    for (p_name, p) in &variant.properties {
        let schema_obj = get_schema_obj(p)?;
        if let Some(obj) = &schema_obj.object {
            let ty = TypeData::from_object_schema(*obj.clone(), None)?;
            let TypeData::Struct { fields, .. } = ty else {
                bail!("the content of an inline `oneOf` variant must be an object with properties");
            };

            return Ok((p_name.to_owned(), EnumVariantType::Struct { fields }));
        }

        if let Some(schema_ref) = &schema_obj.reference {
            return Ok((
                p_name.to_owned(),
                EnumVariantType::Ref {
                    schema_ref: Some(
                        get_schema_name(Some(schema_ref.as_str()))
                            .context("variant reference outside #/components/schemas")?,
                    ),
                    inner: None,
                },
            ));
        }
    }

    bail!(
        "no property of an inline `oneOf` variant holds an object or `$ref` to use as its content"
    )
}

fn get_discriminator(obj: &ObjectValidation) -> anyhow::Result<(String, String)> {
    let mut discriminator_field_name = None;
    let mut discriminator = None;

    for (p_name, p) in &obj.properties {
        let schema_obj = get_schema_obj(p)?;
        if let Some(enum_vals) = &schema_obj.enum_values
            && enum_vals.len() == 1
        {
            match &enum_vals[0].as_str() {
                Some(v) => {
                    discriminator_field_name = Some(p_name.clone());
                    discriminator = Some((*v).to_owned());
                }
                None => bail!(
                    "the single-value `enum` of property `{p_name}` in an inline `oneOf` variant is not a string, so it cannot name the variant"
                ),
            }
        }
    }

    let (Some(discriminator_field_name), Some(discriminator)) =
        (discriminator_field_name, discriminator)
    else {
        bail!(
            "an inline `oneOf` variant has no property with a single-value `enum` (or `const`) \
             naming the variant, so perseid cannot tell the variants apart; add one, such as \
             `type: {{type: string, enum: [circle]}}`, or `discriminator.mapping` with `$ref` \
             variants, or set `x-perseid-union: json` on the schema to keep it untyped"
        )
    };

    Ok((discriminator_field_name, discriminator))
}

fn get_schema_obj(s: &Schema) -> anyhow::Result<&SchemaObject> {
    match s {
        Schema::Bool(_) => bail!("a boolean schema inside `oneOf` is not supported"),
        Schema::Object(o) => Ok(o),
    }
}

fn get_obj_validation(s: &Schema) -> anyhow::Result<&ObjectValidation> {
    let Some(obj) = get_schema_obj(s)?.object.as_ref() else {
        bail!("an inline `oneOf` variant must be an object with properties");
    };
    Ok(obj)
}
