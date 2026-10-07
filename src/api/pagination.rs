use std::collections::BTreeMap;

use anyhow::{Context as _, bail, ensure};
use heck::ToSnakeCase as _;
use serde::{Deserialize, Serialize};

use super::types::{Field, FieldType, Type, TypeData, Types};
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
    /// For each path above (`item_cursor` within an item), whether each of its properties may
    /// be absent or null, for SDKs that read them through typed fields.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) optional: BTreeMap<String, Vec<bool>>,
}

/// What an operation exposes to pagination.
pub(crate) struct Candidate<'a> {
    pub(crate) query_params: Vec<(&'a str, &'a FieldType)>,
    pub(crate) response: Option<&'a str>,
}

impl Pagination {
    /// Unless `strict`, `has_more`, `total_pages` and `total` are left out when the response
    /// has no such property, so that one perseid.toml rule fits responses giving either.
    pub(crate) fn resolve(
        spec: &config::Pagination,
        op: &Candidate<'_>,
        types: &Types,
        strict: bool,
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
        let items = match &spec.items {
            Some(items) => split(items),
            None => vec![default_items(types, response)?],
        };
        let item_schema = match field(types, response, &items)? {
            FieldType::List { inner } => match &**inner {
                FieldType::SchemaRef { name, .. } => name.clone(),
                _ => bail!("items must be an array of a component schema"),
            },
            _ => bail!("`{}` is not an array", items.join(".")),
        };
        let path = |value: &Option<String>,
                    check: fn(&FieldType, &Types) -> bool,
                    what: &str,
                    if_present: bool| {
            let Some(value) = value.as_deref() else {
                return Ok(None);
            };
            let path = split(value);
            if if_present && fields(types, response, &path).is_err() {
                return Ok(None);
            }
            ensure!(
                check(field(types, response, &path)?, types),
                "`{value}` must be {what}"
            );
            Ok(Some(path))
        };
        let next_cursor = path(&spec.next_cursor, is_string, "a string", false)?;
        let is_bool = |t: &FieldType, _: &Types| *t == FieldType::Bool;
        let has_more = path(&spec.has_more, is_bool, "a boolean", !strict)?;
        let total_pages = path(
            &spec.total_pages,
            |t, _| is_integer(t),
            "an integer",
            !strict,
        )?;
        let total = path(&spec.total, |t, _| is_integer(t), "an integer", !strict)?;
        if let Some(name) = &spec.item_cursor {
            ensure!(
                is_string(
                    field(types, &item_schema, std::slice::from_ref(name))?,
                    types
                ),
                "item cursor `{name}` must be a string"
            );
        }
        let item_cursor = spec.item_cursor.clone().map(|name| vec![name]);
        let mut optional = BTreeMap::new();
        for (key, schema, path) in [
            ("items", response, Some(&items)),
            ("next_cursor", response, next_cursor.as_ref()),
            ("has_more", response, has_more.as_ref()),
            ("total_pages", response, total_pages.as_ref()),
            ("total", response, total.as_ref()),
            ("item_cursor", item_schema.as_str(), item_cursor.as_ref()),
        ] {
            if let Some(path) = path {
                let along = fields(types, schema, path)?;
                let flags = along.iter().map(|f| !f.required || f.nullable).collect();
                optional.insert(key.to_owned(), flags);
            }
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
            optional,
        })
    }
}

/// The members a page adds to the response body, in snake case.
const PAGE_MEMBERS: &[&str] = &[
    "items",
    "body",
    "has_next_page",
    "next_page",
    "get_next_page",
    "pages",
    "iter_pages",
];

impl Pagination {
    /// Top-level properties of `response` that a page member shadows: only `body` reaches them.
    pub(crate) fn shadowed(&self, types: &Types, response: &str) -> Vec<String> {
        let Some(TypeData::Struct { fields, .. }) = types.get(response).map(|t| &t.data) else {
            return Vec::new();
        };
        fields
            .iter()
            .filter(|f| PAGE_MEMBERS.contains(&f.name.to_snake_case().as_str()))
            .filter(|f| self.items != [f.name.clone()])
            .map(|f| f.name.clone())
            .collect()
    }
}

/// `data`, else the response's only array of component schemas.
fn default_items(types: &Types, response: &str) -> anyhow::Result<String> {
    let fields = match types.get(response).map(|t| &t.data) {
        Some(TypeData::Struct { fields, .. }) => fields,
        _ => bail!("`{response}` is not an object schema"),
    };
    if fields.iter().any(|f| f.name == "data") {
        return Ok("data".to_owned());
    }
    let arrays: Vec<&str> = fields
        .iter()
        .filter(|f| match &f.r#type {
            FieldType::List { inner } => matches!(**inner, FieldType::SchemaRef { .. }),
            _ => false,
        })
        .map(|f| f.name.as_str())
        .collect();
    match arrays[..] {
        [one] => Ok(one.to_owned()),
        [] => bail!("`{response}` has no array of items"),
        _ => bail!("`items` must name one of `{}`", arrays.join("`, `")),
    }
}

fn split(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
}

fn field<'a>(types: &'a Types, schema: &'a str, path: &[String]) -> anyhow::Result<&'a FieldType> {
    let along = fields(types, schema, path)?;
    Ok(&along.last().context("empty path")?.r#type)
}

/// The properties `path` goes through, from `schema`.
fn fields<'a>(
    types: &'a Types,
    schema: &'a str,
    path: &[String],
) -> anyhow::Result<Vec<&'a Field>> {
    let mut schema = schema;
    let mut along: Vec<&'a Field> = Vec::new();
    for segment in path {
        match along.last().map(|f| &f.r#type) {
            Some(FieldType::SchemaRef { name, .. }) => schema = name.as_str(),
            Some(_) => bail!("`{}` crosses a non-object value", path.join(".")),
            None => {}
        }
        let fields = match types.get(schema).map(|t: &Type| &t.data) {
            Some(TypeData::Struct { fields, .. }) => fields,
            _ => bail!("`{schema}` is not an object schema"),
        };
        along.push(
            fields
                .iter()
                .find(|f| f.name == *segment)
                .with_context(|| format!("`{schema}` has no `{segment}` property"))?,
        );
    }
    Ok(along)
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
