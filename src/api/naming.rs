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
    let mut errors = Vec::new();
    for resource in resources.values_mut() {
        name_resource(resource, names, &mut errors);
    }
    match errors.len() {
        0 => Ok(()),
        1 => bail!("{}", errors[0]),
        n => bail!("{n} method names clash:\n  - {}", errors.join("\n  - ")),
    }
}

fn name_resource(
    resource: &mut Resource,
    names: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    for sub in resource.subresources.values_mut() {
        name_resource(sub, names, errors);
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
                &op.id,
                &op.method,
                &op.path,
                is_collection(resource, op),
                is_single_object(op),
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
        if let Some(other) = seen.insert(key(name), op)
            && other.id != op.id
        {
            errors.push(format!(
                "operations `{}` ({} {}) and `{}` ({} {}) are both named `{}` in resource `{}`: \
                 set `x-perseid-name` on one of them or rename it in the `[methods]` table of \
                 perseid.toml",
                other.id,
                other.method.to_uppercase(),
                other.path,
                op.id,
                op.method.to_uppercase(),
                op.path,
                key(name),
                resource.name
            ));
        }
    }
    for (op, name) in resource.operations.iter_mut().zip(chosen) {
        op.name = name;
    }
}

fn base_name(op: &Operation, own: &Option<String>, derived: &Option<String>) -> String {
    own.clone()
        .or_else(|| derived.clone())
        .unwrap_or_else(|| snake(&op.name))
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

/// Whether a `GET` returns one named object rather than something that could be a page: a
/// schema that is not a list and is not named like a list wrapper.
fn is_single_object(op: &Operation) -> bool {
    const WRAPPERS: [&str; 10] = [
        "Response",
        "Result",
        "Results",
        "Collection",
        "Envelope",
        "Data",
        "Items",
        "Paged",
        "Page",
        "List",
    ];
    !op.returns_list()
        && op
            .response_schema()
            .is_some_and(|name| !WRAPPERS.iter().any(|w| name.ends_with(w)))
}

/// `GET /customers` is `list`, `POST /customers/{id}/sources` `create_source`. Where the path
/// only gives a CRUD verb or a noun posted to, an operation id starting with an action
/// (`archive_customer`) or `create` (`create_session`) names the operation instead.
/// A `GET` of a singular noun with no items below it (`collection`), or of one named object
/// (`single_object`), retrieves rather than lists.
pub(crate) fn derive(
    resource: &str,
    id: &str,
    method: &str,
    path: &str,
    collection: bool,
    single_object: bool,
) -> Option<String> {
    let (name, kind) = from_path(resource, method, path, collection, single_object)?;
    let phrase = id_phrase(resource, id);
    let verb = phrase.split('_').next().unwrap_or_default();
    let name = match kind {
        Kind::Action => name,
        _ if is_action(verb) => phrase,
        Kind::Noun if verb == "create" && phrase != verb => phrase,
        _ => name,
    };
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some(name)
}

/// What a path says an operation does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A CRUD verb on the resource or a noun below it: `list`, `retrieve_source`.
    Crud,
    /// A verb segment: `POST /invoices/{id}/finalize`.
    Action,
    /// A noun posted to: `POST /session`.
    Noun,
}

fn from_path(
    resource: &str,
    method: &str,
    path: &str,
    collection: bool,
    single_object: bool,
) -> Option<(String, Kind)> {
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
    let own = singular(&snake(resource)).replace('_', "");
    let singleton = !collection
        && (single_object
            || segments
                .iter()
                .rfind(|s| !is_param(s) && !is_prefix(s))
                .is_some_and(|s| !is_plural(&snake(s))));
    let rest: Vec<&str> = match segments
        .iter()
        .position(|s| !is_param(s) && singular(&snake(s)).replace('_', "") == own)
    {
        Some(i) => segments[i + 1..].to_vec(),
        None => segments
            .into_iter()
            // A one or two letter fragment (`/a/{id}`) names nothing worth a method name.
            .filter(|s| is_param(s) || (!is_prefix(s) && s.chars().count() > 2))
            .collect(),
    };
    let literals: Vec<String> = rest
        .iter()
        .filter(|s| !is_param(s))
        .map(|s| snake(s))
        .collect();
    if method == "get" && is_health(literals.last().map_or(&own, |l| l)) {
        let name = match literals.is_empty() || is_health(&own) {
            true => "check",
            false => "check_health",
        };
        return Some((name.to_owned(), Kind::Action));
    }
    if rest.is_empty() {
        let name = match method {
            "get" if singleton => "retrieve",
            "get" => "list",
            "post" => "create",
            "put" | "patch" => "update",
            "delete" => "delete",
            _ => return None,
        };
        return Some((name.to_owned(), Kind::Crud));
    }
    if literals.is_empty() {
        return Some((item_verb(method)?.to_owned(), Kind::Crud));
    }
    if rest.last().is_some_and(|s| is_param(s)) {
        let noun = literals.iter().map(|l| singular(l)).join("_");
        return Some((format!("{}_{noun}", item_verb(method)?), Kind::Crud));
    }
    let tail_start = rest.iter().rposition(|s| is_param(s)).map_or(0, |i| i + 1);
    let tail = rest[tail_start..].iter().map(|s| snake(s)).join("_");
    let context = rest[..tail_start]
        .iter()
        .filter(|s| !is_param(s))
        .map(|s| singular(&snake(s)))
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
    Some(match method {
        "get" if verb => (action(&tail), Kind::Action),
        "get" if collection || is_plural(&tail) => (format!("list_{}", noun(&tail)), Kind::Crud),
        "get" => (format!("retrieve_{}", noun(&tail)), Kind::Crud),
        "post" if !verb && is_plural(&tail) => {
            (format!("create_{}", noun(&singular(&tail))), Kind::Crud)
        }
        "post" if verb => (action(&tail), Kind::Action),
        "post" => (action(&tail), Kind::Noun),
        "put" | "patch" => (format!("update_{}", noun(&tail)), Kind::Crud),
        "delete" => (format!("delete_{}", noun(&tail)), Kind::Crud),
        _ => return None,
    })
}

/// The operation id in snake_case without the resource's own noun: `send_invoice_reminder` is
/// `send_reminder` in `invoices`.
fn id_phrase(resource: &str, id: &str) -> String {
    let own = singular(&snake(resource)).replace('_', "");
    let mut words: Vec<String> = snake(id.rsplit('/').next().unwrap_or(id))
        .split('_')
        .map(str::to_owned)
        .collect();
    let noun = (1..words.len())
        .flat_map(|start| (start + 1..=words.len()).map(move |end| (start, end)))
        .find(|&(start, end)| singular(&words[start..end].join("_")).replace('_', "") == own);
    if let Some((start, end)) = noun {
        words.drain(start..end);
    }
    words.join("_")
}

/// Verbs saying more than CRUD does, at the start of a path segment or an operation id.
const ACTIONS: &str = "accept activate approve archive attach authorize cancel capture check close \
    complete confirm count deactivate decline detach disable disconnect download duplicate enable \
    estimate export finalize ingest introspect login logout mark pause preview publish reactivate \
    redeem refresh refund regenerate reject render reopen resend restore resume revoke rotate \
    search send simulate submit suspend sync test unarchive unlink unpublish upload validate \
    verify void";

fn is_action(verb: &str) -> bool {
    ACTIONS.split_whitespace().any(|action| action == verb)
}

/// Whether a path segment names an action rather than a sub-resource.
fn is_verb(words: &str) -> bool {
    const CRUD: [&str; 11] = [
        "add", "create", "delete", "find", "get", "list", "lookup", "remove", "retrieve", "set",
        "update",
    ];
    let verb = words.split('_').next().unwrap_or(words);
    CRUD.contains(&verb) || is_action(verb)
}

fn is_health(words: &str) -> bool {
    ["health", "healthz", "healthcheck", "health_check"].contains(&words)
}

/// snake_case keeping a single letter with the word after it: `OAuth` is `oauth`, not `o_auth`,
/// while `IPAddress` is `ip_address` and the article in `getAThing` stays `get_a_thing`.
pub(crate) fn snake(name: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in name.split(|c: char| !c.is_alphanumeric()) {
        let mut letter: Option<String> = None;
        let pieces = word.to_snake_case();
        for (i, piece) in pieces.split('_').filter(|p| !p.is_empty()).enumerate() {
            let article = i > 0 && piece == "a";
            match letter.take() {
                Some(letter) => out.push(letter + piece),
                None if piece.len() == 1 && piece.chars().all(char::is_alphabetic) && !article => {
                    letter = Some(piece.to_owned());
                }
                None => out.push(piece.to_owned()),
            }
        }
        out.extend(letter);
    }
    out.join("_")
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
        let name = |method, path| derive("customers", "", method, path, false, false);
        assert_eq!(name("get", "/v1/customers").as_deref(), Some("list"));
        assert_eq!(
            derive("customers", "", "get", "/v1/customers", false, true).as_deref(),
            Some("retrieve")
        );
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
        let name = |method, path| derive("customers", "", method, path, false, false).unwrap();
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
            derive("add_ons", "", "get", "/addons/{id}", false, false).unwrap(),
            "retrieve"
        );
        assert_eq!(name("post", "/customers/{c}/archive"), "archive");
        assert_eq!(
            derive("pet", "", "get", "/pet/findByStatus", false, false).unwrap(),
            "find_by_status"
        );
        assert_eq!(
            derive("store", "", "get", "/store/order/{id}", false, false).unwrap(),
            "retrieve_order"
        );
    }

    #[test]
    fn resources_missing_from_the_path_name_its_nouns() {
        assert_eq!(
            derive("billing", "", "get", "/api/v1/invoices", false, false).unwrap(),
            "list_invoices"
        );
        assert_eq!(
            derive("billing", "", "post", "/invoices", false, false).unwrap(),
            "create_invoice"
        );
        assert_eq!(
            derive("billing", "", "get", "/invoices/{id}", false, false).unwrap(),
            "retrieve_invoice"
        );
        assert_eq!(
            derive("billing", "", "get", "/{id}", false, false),
            Some("retrieve".into())
        );
    }

    #[test]
    fn singular_paths_without_items_are_retrieved() {
        let name =
            |path, collection| derive("balance", "", "get", path, collection, false).unwrap();
        assert_eq!(name("/v1/balance", false), "retrieve");
        assert_eq!(name("/v1/balance", true), "list");
        assert_eq!(name("/v1/balance/history", false), "retrieve_history");
        assert_eq!(name("/v1/balance/history", true), "list_history");
        assert_eq!(
            derive("usage", "", "get", "/v1/usage/costs", false, false).unwrap(),
            "list_costs"
        );
    }

    #[test]
    fn fragments_of_foreign_paths_are_not_nouns() {
        assert_eq!(
            derive("collide", "", "get", "/a/{id}", false, false).unwrap(),
            "retrieve"
        );
        assert_eq!(
            derive("collide", "", "post", "/ab/{id}/cancel", false, false).unwrap(),
            "cancel"
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

    #[test]
    fn posts_to_a_singular_noun_keep_the_create_of_their_operation_id() {
        let name = |resource, id, path| derive(resource, id, "post", path, false, false).unwrap();
        assert_eq!(
            name("auth", "create_session", "/auth/session"),
            "create_session"
        );
        assert_eq!(
            name("account", "createSession", "/session"),
            "create_session"
        );
        assert_eq!(
            name(
                "customers",
                "create_portal_token",
                "/customers/{id}/portal-token"
            ),
            "create_portal_token"
        );
        assert_eq!(name("auth", "session", "/auth/session"), "session");
        assert_eq!(name("oauth", "token_endpoint", "/oauth/token"), "token");
        assert_eq!(name("sessions", "create_session", "/sessions"), "create");
    }

    #[test]
    fn action_verbs_of_operation_ids_name_crud_paths() {
        let name =
            |resource, id, method, path| derive(resource, id, method, path, false, false).unwrap();
        assert_eq!(
            name("customers", "archive_customer", "delete", "/customers/{id}"),
            "archive"
        );
        assert_eq!(name("files", "upload_file", "post", "/files"), "upload");
        assert_eq!(
            name("storage", "upload_file", "post", "/files"),
            "upload_file"
        );
        assert_eq!(
            name(
                "invoices",
                "sendInvoiceReminder",
                "post",
                "/invoices/{id}/reminders"
            ),
            "send_reminder"
        );
        assert_eq!(
            name(
                "checkout_sessions",
                "cancel_checkout_session",
                "delete",
                "/checkout-sessions/{id}"
            ),
            "cancel"
        );
        assert_eq!(
            name("add_ons", "archive_addon", "delete", "/addons/{id}"),
            "archive"
        );
        assert_eq!(
            name(
                "invoices",
                "download_invoice_xml",
                "get",
                "/invoices/{id}/xml"
            ),
            "download_xml"
        );
        assert_eq!(name("user", "loginUser", "get", "/user/login"), "login");
        assert_eq!(name("pets", "findPets", "get", "/pets"), "list");
        assert_eq!(
            name("customers", "get_customer", "get", "/customers/{id}"),
            "retrieve"
        );
        assert_eq!(
            name("customers", "fetch_customer", "get", "/customers/{id}"),
            "retrieve"
        );
        assert_eq!(
            name("customers", "patch_customer", "patch", "/customers/{id}"),
            "update"
        );
    }

    #[test]
    fn action_segments_of_paths_win_over_operation_ids() {
        let name = |resource, id, path| derive(resource, id, "post", path, false, false).unwrap();
        assert_eq!(
            name(
                "oauth_apps",
                "rotate_client_secret",
                "/oauth-apps/{id}/rotate"
            ),
            "rotate"
        );
        assert_eq!(name("oauth", "revoke_endpoint", "/oauth/revoke"), "revoke");
        assert_eq!(
            name("invoices", "finalize_invoice", "/invoices/{id}/finalize"),
            "finalize"
        );
    }

    #[test]
    fn health_checks_are_checked() {
        let name =
            |resource, path| derive(resource, "get_health", "get", path, false, false).unwrap();
        assert_eq!(name("health", "/health"), "check");
        assert_eq!(name("health", "/v1/healthz"), "check");
        assert_eq!(name("account", "/health"), "check_health");
        assert_eq!(
            derive("health", "", "post", "/health", false, false).unwrap(),
            "create"
        );
    }

    #[test]
    fn single_letters_stay_with_the_next_word() {
        assert_eq!(snake("OAuth"), "oauth");
        assert_eq!(snake("OAuth Apps"), "oauth_apps");
        assert_eq!(snake("getOAuth2Token"), "get_oauth2_token");
        assert_eq!(snake("iOS"), "ios");
        assert_eq!(snake("IPAddress"), "ip_address");
        assert_eq!(snake("APIKeys"), "api_keys");
        assert_eq!(snake("Plan A"), "plan_a");
        assert_eq!(snake("a-b"), "a_b");
        assert_eq!(snake("tax_ids"), "tax_ids");
        assert_eq!(snake("getAThing"), "get_a_thing");
        assert_eq!(
            derive("oauth", "", "get", "/oauth/authorizeOAuthApp", false, false).unwrap(),
            "authorize_oauth_app"
        );
    }
}
