use anyhow::{Context as _, bail, ensure};
use serde::{Deserialize, Serialize};

use super::types::{FieldType, Type, TypeData, Types};
use crate::config;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Style {
    Cursor,
    Page,
    Offset,
}

/// A validated pagination scheme. Paths are JSON property names from the response root.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Pagination {
    pub(crate) style: Style,
    /// Query parameter selecting the page.
    pub(crate) param: String,
    pub(crate) items: Vec<String>,
    /// Component schema of one item.
    pub(crate) item_schema: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_cursor: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) has_more: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_pages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<Vec<String>>,
    pub(crate) first_page: i64,
}

/// What an operation exposes to pagination.
pub(crate) struct Candidate<'a> {
    pub(crate) query_params: Vec<(&'a str, &'a FieldType)>,
    pub(crate) response: Option<&'a str>,
}

impl Pagination {
    pub(crate) fn resolve(
        spec: &config::Pagination,
        op: &Candidate<'_>,
        types: &Types,
    ) -> anyhow::Result<Self> {
        let (style, param) = match (&spec.cursor, &spec.page, &spec.offset) {
            (Some(p), None, None) => (Style::Cursor, p),
            (None, Some(p), None) => (Style::Page, p),
            (None, None, Some(p)) => (Style::Offset, p),
            _ => bail!("exactly one of `cursor`, `page` or `offset` must name the page parameter"),
        };
        let (_, param_type) = op
            .query_params
            .iter()
            .find(|(name, _)| *name == param)
            .with_context(|| format!("no `{param}` query parameter"))?;
        match style {
            Style::Cursor => ensure!(
                is_string(param_type, types),
                "cursor parameter `{param}` must be a string"
            ),
            _ => ensure!(
                is_integer(param_type),
                "`{param}` must be an integer query parameter"
            ),
        }
        let allowed = |field: &Option<String>, name: &str, styles: &[Style]| {
            ensure!(
                field.is_none() || styles.contains(&style),
                "`{name}` does not apply to {style:?} pagination"
            );
            Ok(())
        };
        allowed(&spec.next_cursor, "next_cursor", &[Style::Cursor])?;
        allowed(&spec.item_cursor, "item_cursor", &[Style::Cursor])?;
        allowed(&spec.total_pages, "total_pages", &[Style::Page])?;
        allowed(&spec.total, "total", &[Style::Offset])?;
        ensure!(
            spec.first_page.is_none() || style == Style::Page,
            "`first_page` only applies to page pagination"
        );
        ensure!(
            style != Style::Cursor || spec.next_cursor.is_some() != spec.item_cursor.is_some(),
            "cursor pagination needs exactly one of `next_cursor` or `item_cursor`"
        );

        let response = op.response.context("no JSON response body")?;
        let items = split(spec.items.as_deref().unwrap_or("data"));
        let item_schema = match field(types, response, &items)? {
            FieldType::List { inner } => match &**inner {
                FieldType::SchemaRef { name, .. } => name.clone(),
                _ => bail!("items must be an array of a component schema"),
            },
            _ => bail!("`{}` is not an array", items.join(".")),
        };
        let path = |value: &Option<String>, check: fn(&FieldType, &Types) -> bool, what: &str| {
            value
                .as_deref()
                .map(|value| {
                    let path = split(value);
                    ensure!(
                        check(field(types, response, &path)?, types),
                        "`{value}` must be {what}"
                    );
                    Ok(path)
                })
                .transpose()
        };
        let next_cursor = path(&spec.next_cursor, is_string, "a string")?;
        let has_more = path(&spec.has_more, |t, _| *t == FieldType::Bool, "a boolean")?;
        let total_pages = path(&spec.total_pages, |t, _| is_integer(t), "an integer")?;
        let total = path(&spec.total, |t, _| is_integer(t), "an integer")?;
        if let Some(name) = &spec.item_cursor {
            ensure!(
                is_string(
                    field(types, &item_schema, std::slice::from_ref(name))?,
                    types
                ),
                "item cursor `{name}` must be a string"
            );
        }
        Ok(Self {
            style,
            param: param.clone(),
            items,
            item_schema,
            next_cursor,
            item_cursor: spec.item_cursor.clone(),
            has_more,
            total_pages,
            total,
            first_page: spec.first_page.unwrap_or(1),
        })
    }
}

fn split(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
}

fn field<'a>(types: &'a Types, schema: &'a str, path: &[String]) -> anyhow::Result<&'a FieldType> {
    let mut schema = schema;
    let mut found: Option<&'a FieldType> = None;
    for segment in path {
        if let Some(FieldType::SchemaRef { name, .. }) = found {
            schema = name.as_str();
        } else if found.is_some() {
            bail!("`{}` crosses a non-object value", path.join("."));
        }
        let fields = match types.get(schema).map(|t: &Type| &t.data) {
            Some(TypeData::Struct { fields }) => fields,
            _ => bail!("`{schema}` is not an object schema"),
        };
        found = Some(
            &fields
                .iter()
                .find(|f| f.name == *segment)
                .with_context(|| format!("`{schema}` has no `{segment}` property"))?
                .r#type,
        );
    }
    found.context("empty path")
}

fn is_integer(t: &FieldType) -> bool {
    matches!(
        t,
        FieldType::Int16
            | FieldType::UInt16
            | FieldType::Int32
            | FieldType::Int64
            | FieldType::UInt64
    )
}

fn is_string(t: &FieldType, types: &Types) -> bool {
    match t {
        FieldType::String => true,
        FieldType::SchemaRef { name, .. } => {
            matches!(
                types.get(name).map(|t| &t.data),
                Some(TypeData::StringAlias)
            )
        }
        _ => false,
    }
}
