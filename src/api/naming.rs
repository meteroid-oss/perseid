//! Method names of operations, after their HTTP method and path within their resource
//! (`customers.list`, `customers.retrieve`) rather than their operation id.

use std::collections::BTreeMap;

use anyhow::bail;
use heck::ToSnakeCase as _;
use itertools::Itertools as _;

use super::resources::{Operation, Resource, Resources};

/// Names every operation after its resource, unless `names` or `x-perseid-name` overrides it.
pub(crate) fn apply(
    resources: &mut Resources,
    names: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for resource in resources.values_mut() {
        name_resource(resource, names)?;
    }
    Ok(())
}

fn name_resource(resource: &mut Resource, names: &BTreeMap<String, String>) -> anyhow::Result<()> {
    for sub in resource.subresources.values_mut() {
        name_resource(sub, names)?;
    }
    let key = |name: &str| name.to_snake_case();
    let overrides: Vec<Option<String>> = resource
        .operations
        .iter()
        .map(|op| {
            names
                .get(&op.id)
                .cloned()
                .or_else(|| op.x_perseid_name.clone())
        })
        .collect();
    let mut derived: Vec<Option<String>> = resource
        .operations
        .iter()
        .map(|op| match op.stream {
            true => None,
            false => derive(
                &resource.name,
                &op.method,
                &op.path,
                is_collection(resource, op),
            ),
        })
        .collect();
    // `PUT` replaces what `PATCH` updates on the same path.
    for (op, derived) in resource.operations.iter().zip(derived.iter_mut()) {
        let patched = resource
            .operations
            .iter()
            .any(|other| other.method == "patch" && other.path == op.path);
        if op.method == "put"
            && patched
            && let Some(name) = derived
            && let Some(rest) = name.strip_prefix("update")
        {
            *name = format!("replace{rest}");
        }
    }
    // A derived name that two operations share, or that another operation keeps, is dropped for
    // the operation id until every name is unique.
    loop {
        let effective: Vec<String> = (0..derived.len())
            .map(|i| {
                key(&base_name(
                    &resource.operations[i],
                    &overrides[i],
                    &derived[i],
                ))
            })
            .collect();
        let counts = effective.iter().counts();
        let mut changed = false;
        for (i, name) in effective.iter().enumerate() {
            if counts[name] > 1 && overrides[i].is_none() && derived[i].take().is_some() {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let base: Vec<String> = (0..derived.len())
        .map(|i| base_name(&resource.operations[i], &overrides[i], &derived[i]))
        .collect();
    let ops = &resource.operations;
    let chosen: Vec<String> = (0..ops.len())
        .map(|i| match ops[i].stream {
            true => stream_name(ops, &base, i),
            false => base[i].clone(),
        })
        .collect();
    let mut seen = BTreeMap::new();
    for (op, name) in ops.iter().zip(&chosen) {
        if let Some(other) = seen.insert(key(name), &op.id)
            && *other != op.id
        {
            bail!(
                "operations `{other}` and `{}` are both named `{}` in resource `{}`: rename one \
                 in the `[methods]` table of perseid.toml",
                op.id,
                key(name),
                resource.name
            );
        }
    }
    for (op, name) in resource.operations.iter_mut().zip(chosen) {
        op.name = name;
    }
    Ok(())
}

fn base_name(op: &Operation, own: &Option<String>, derived: &Option<String>) -> String {
    own.clone()
        .or_else(|| derived.clone())
        .unwrap_or_else(|| op.name.clone())
}

/// The operation a `_stream` twin reads the event stream of.
fn base_of(ops: &[Operation], twin: usize) -> Option<usize> {
    ops.iter()
        .position(|op| !op.stream && op.id == ops[twin].id)
}

fn stream_name(ops: &[Operation], base: &[String], twin: usize) -> String {
    match base_of(ops, twin) {
        Some(b) => format!("{}_stream", base[b]),
        None => ops[twin].name.clone(),
    }
}

/// Whether a `GET` of the operation's path lists: it returns a list, or items live below it.
fn is_collection(resource: &Resource, op: &Operation) -> bool {
    let item = format!("{}/{{", op.path.trim_end_matches('/'));
    op.returns_list()
        || resource.operations.iter().any(|other| {
            other
                .path
                .strip_prefix(&item)
                .is_some_and(|rest| rest.ends_with('}') && !rest.contains('/'))
        })
}

/// `GET /customers` is `list`, `POST /customers/{id}/sources` `create_source` and
/// `POST /invoices/{id}/finalize` `finalize`, in the resources named after their first segment.
/// A `GET` of a singular noun with no items below it (`collection`) retrieves rather than lists.
pub(crate) fn derive(resource: &str, method: &str, path: &str, collection: bool) -> Option<String> {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| match s.starts_with('{') {
            true => s,
            false => s.split('.').next().unwrap_or(s),
        })
        .filter(|s| !s.is_empty())
        .collect();
    let is_param = |s: &str| s.starts_with('{');
    let own = singular(&resource.to_snake_case()).replace('_', "");
    let singleton = !collection
        && segments
            .iter()
            .rfind(|s| !is_param(s) && !is_prefix(s))
            .is_some_and(|s| !is_plural(&s.to_snake_case()));
    let rest: Vec<&str> = match segments
        .iter()
        .position(|s| !is_param(s) && singular(&s.to_snake_case()).replace('_', "") == own)
    {
        Some(i) => segments[i + 1..].to_vec(),
        None => segments
            .into_iter()
            .filter(|s| is_param(s) || !is_prefix(s))
            .collect(),
    };
    let literals: Vec<String> = rest
        .iter()
        .filter(|s| !is_param(s))
        .map(|s| s.to_snake_case())
        .collect();
    let name = if rest.is_empty() {
        match method {
            "get" if singleton => "retrieve",
            "get" => "list",
            "post" => "create",
            "put" | "patch" => "update",
            "delete" => "delete",
            _ => return None,
        }
        .to_owned()
    } else if literals.is_empty() {
        item_verb(method)?.to_owned()
    } else if rest.last().is_some_and(|s| is_param(s)) {
        let noun = literals.iter().map(|l| singular(l)).join("_");
        format!("{}_{noun}", item_verb(method)?)
    } else {
        let tail_start = rest.iter().rposition(|s| is_param(s)).map_or(0, |i| i + 1);
        let tail = rest[tail_start..]
            .iter()
            .map(|s| s.to_snake_case())
            .join("_");
        let context = rest[..tail_start]
            .iter()
            .filter(|s| !is_param(s))
            .map(|s| singular(&s.to_snake_case()))
            .join("_");
        let noun = |noun: &str| match context.is_empty() {
            true => noun.to_owned(),
            false => format!("{context}_{noun}"),
        };
        let action = |verb: &str| match context.is_empty() {
            true => verb.to_owned(),
            false => format!("{verb}_{context}"),
        };
        let verb = is_verb(&tail);
        match method {
            "get" if verb => action(&tail),
            "get" if collection || is_plural(&tail) => format!("list_{}", noun(&tail)),
            "get" => format!("retrieve_{}", noun(&tail)),
            "post" if !verb && is_plural(&tail) => format!("create_{}", noun(&singular(&tail))),
            "post" => action(&tail),
            "put" | "patch" => format!("update_{}", noun(&tail)),
            "delete" => format!("delete_{}", noun(&tail)),
            _ => return None,
        }
    };
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some(name)
}

/// Whether a path segment names an action rather than a sub-resource.
fn is_verb(words: &str) -> bool {
    const VERBS: [&str; 29] = [
        "add", "attach", "cancel", "check", "count", "create", "delete", "detach", "download",
        "estimate", "export", "find", "get", "list", "lookup", "mark", "preview", "remove",
        "render", "retrieve", "search", "set", "simulate", "sync", "test", "update", "upload",
        "validate", "verify",
    ];
    VERBS.contains(&words.split('_').next().unwrap_or(words))
}

fn item_verb(method: &str) -> Option<&'static str> {
    match method {
        "get" => Some("retrieve"),
        "put" | "patch" | "post" => Some("update"),
        "delete" => Some("delete"),
        _ => None,
    }
}

/// `api`, `v1` or `2010-04-01`, which scope paths rather than name resources.
fn is_prefix(segment: &str) -> bool {
    segment == "api"
        || segment.starts_with(|c: char| c.is_ascii_digit())
        || segment
            .strip_prefix(['v', 'V'])
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn is_plural(words: &str) -> bool {
    let word = words.rsplit('_').next().unwrap_or(words);
    word.len() >= 3
        && word.ends_with('s')
        && !["ss", "us", "is"].iter().any(|end| word.ends_with(end))
}

/// The singular of the last word of a snake_case name, as far as a suffix tells.
fn singular(words: &str) -> String {
    if !is_plural(words) {
        return words.to_owned();
    }
    if let Some(stem) = words.strip_suffix("ies") {
        return format!("{stem}y");
    }
    for suffix in ["sses", "xes", "ches", "shes", "zes", "uses"] {
        if let Some(stem) = words.strip_suffix(suffix) {
            return format!("{stem}{}", &suffix[..suffix.len() - 2]);
        }
    }
    words[..words.len() - 1].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_paths_get_resource_method_names() {
        let name = |method, path| derive("customers", method, path, false);
        assert_eq!(name("get", "/v1/customers").as_deref(), Some("list"));
        assert_eq!(name("post", "/v1/customers").as_deref(), Some("create"));
        assert_eq!(
            name("get", "/v1/customers/{id}").as_deref(),
            Some("retrieve")
        );
        assert_eq!(
            name("post", "/v1/customers/{id}").as_deref(),
            Some("update")
        );
        assert_eq!(
            name("patch", "/v1/customers/{id}").as_deref(),
            Some("update")
        );
        assert_eq!(
            name("delete", "/v1/customers/{id}").as_deref(),
            Some("delete")
        );
        assert_eq!(name("head", "/v1/customers/{id}"), None);
    }

    #[test]
    fn sub_paths_name_their_action_and_noun() {
        let name = |method, path| derive("customers", method, path, false).unwrap();
        assert_eq!(name("get", "/v1/customers/search"), "search");
        assert_eq!(name("get", "/v1/customers/{c}/sources"), "list_sources");
        assert_eq!(name("post", "/v1/customers/{c}/sources"), "create_source");
        assert_eq!(
            name("get", "/v1/customers/{c}/sources/{s}"),
            "retrieve_source"
        );
        assert_eq!(
            name("delete", "/v1/customers/{c}/tax_ids/{t}"),
            "delete_tax_id"
        );
        assert_eq!(
            name("get", "/v1/customers/{c}/cash_balance"),
            "retrieve_cash_balance"
        );
        assert_eq!(
            name("delete", "/v1/customers/{c}/discount"),
            "delete_discount"
        );
        assert_eq!(
            name("post", "/v1/customers/{c}/sources/{s}/verify"),
            "verify_source"
        );
        assert_eq!(
            name("get", "/v1/customers/{c}/sources/{s}/lines"),
            "list_source_lines"
        );
        assert_eq!(name("get", "/v1/customers/upcoming"), "retrieve_upcoming");
        assert_eq!(name("get", "/v1/customers/{c}/download"), "download");
        assert_eq!(name("post", "/v1/customers/{c}/add_lines"), "add_lines");
        assert_eq!(
            derive("add_ons", "get", "/addons/{id}", false).unwrap(),
            "retrieve"
        );
        assert_eq!(name("post", "/customers/{c}/archive"), "archive");
        assert_eq!(
            derive("pet", "get", "/pet/findByStatus", false).unwrap(),
            "find_by_status"
        );
        assert_eq!(
            derive("store", "get", "/store/order/{id}", false).unwrap(),
            "retrieve_order"
        );
    }

    #[test]
    fn resources_missing_from_the_path_name_its_nouns() {
        assert_eq!(
            derive("billing", "get", "/api/v1/invoices", false).unwrap(),
            "list_invoices"
        );
        assert_eq!(
            derive("billing", "post", "/invoices", false).unwrap(),
            "create_invoice"
        );
        assert_eq!(
            derive("billing", "get", "/invoices/{id}", false).unwrap(),
            "retrieve_invoice"
        );
        assert_eq!(
            derive("billing", "get", "/{id}", false),
            Some("retrieve".into())
        );
    }

    #[test]
    fn singular_paths_without_items_are_retrieved() {
        let name = |path, collection| derive("balance", "get", path, collection).unwrap();
        assert_eq!(name("/v1/balance", false), "retrieve");
        assert_eq!(name("/v1/balance", true), "list");
        assert_eq!(name("/v1/balance/history", false), "retrieve_history");
        assert_eq!(name("/v1/balance/history", true), "list_history");
        assert_eq!(
            derive("usage", "get", "/v1/usage/costs", false).unwrap(),
            "list_costs"
        );
    }

    #[test]
    fn plurals_are_singularized_by_suffix() {
        assert_eq!(singular("entries"), "entry");
        assert_eq!(singular("addresses"), "address");
        assert_eq!(singular("tax_ids"), "tax_id");
        assert_eq!(singular("status"), "status");
        assert_eq!(singular("statuses"), "status");
    }
}
