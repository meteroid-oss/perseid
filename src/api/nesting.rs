//! Child resources from the paths within each tag: a collection or a segment paths go on below,
//! holding two operations or more (`/workspaces/{id}/peers`, `/workspaces/{id}/peers/{peer_id}`),
//! is a resource of its own, `client.workspaces.peers`; other segments name methods of the parent.

use std::collections::BTreeMap;

use anyhow::bail;
use itertools::Itertools as _;

use super::{
    naming,
    resources::{Child, Operation, Resource, Resources},
};

/// Most resources from the client to a method: the tag's, and two levels below it.
pub(crate) const MAX_DEPTH: usize = 3;

/// Members the generated resource classes declare, or whose class name a child named so would
/// take (`schools.api` would be `SchoolsApi` in C#, as `schools` is): no child resource has them.
pub(crate) const RESERVED: [&str; 9] = [
    "api",
    "async",
    "client",
    "constructor",
    "new",
    "request_ctx",
    "sync",
    "with_options",
    "with_raw_response",
];

/// `workspaces.peers` as the resource path `["workspaces", "peers"]`, or why it isn't one.
pub(crate) fn resource_path(dotted: &str) -> Result<Vec<String>, String> {
    let path: Vec<String> = dotted.split('.').map(str::to_owned).collect();
    if path.len() > MAX_DEPTH {
        return Err(format!(
            "nests {} resources, perseid nests {MAX_DEPTH} at most",
            path.len()
        ));
    }
    let snake = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_lowercase())
            && s.split('_').all(|w| {
                !w.is_empty()
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    };
    if let Some(segment) = path.iter().find(|s| !snake(s)) {
        return Err(format!(
            "`{segment}` is not a snake_case name, as `workspaces.peers` is"
        ));
    }
    if let Some(segment) = path[1..].iter().find(|s| RESERVED.contains(&s.as_str())) {
        return Err(format!(
            "`{segment}` is a member of the generated resources, which no child resource can be named"
        ));
    }
    Ok(path)
}

/// Moves the operations of the tag resources `tags` into their nested resources, or into the
/// resource `overrides` (`[resources]`, by operation id) or their `x-perseid-resource` names.
pub(crate) fn apply(
    tags: Resources,
    overrides: &BTreeMap<String, String>,
) -> anyhow::Result<Resources> {
    let mut placed: BTreeMap<Vec<String>, Vec<Operation>> = BTreeMap::new();
    let mut by_tag: Vec<(String, Vec<Operation>)> = Vec::new();
    for (tag, resource) in tags {
        let mut auto = Vec::new();
        for op in resource.operations {
            let path = match overrides.get(&op.id) {
                Some(dotted) => match resource_path(dotted) {
                    Ok(path) => Some(path),
                    Err(why) => bail!(
                        "`{dotted}`, the resource of `{}` in perseid.toml, {why}",
                        op.id
                    ),
                },
                None => op.x_perseid_resource.clone(),
            };
            match path {
                Some(path) => placed.entry(path).or_default().push(op),
                None => auto.push(op),
            }
        }
        by_tag.push((tag, auto));
    }
    let mut taken: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let paths = (by_tag.iter().map(|(tag, _)| vec![tag.clone()])).chain(placed.keys().cloned());
    for path in paths {
        for n in 1..=path.len() {
            taken
                .entry(path[..n].join("_"))
                .or_insert_with(|| path[..n].to_vec());
        }
    }
    for (tag, ops) in by_tag {
        let items = ops.into_iter().map(|op| Item::new(&tag, op)).collect();
        split(vec![tag], items, &mut taken, &mut placed);
    }
    build(placed)
}

/// An operation, and the indices of the segments of its path below the resource it is in.
struct Item {
    op: Operation,
    segments: Vec<String>,
    rest: Vec<usize>,
    /// Whether a segment names the tag, as `customers` in `/v1/customers/{id}`.
    rooted: bool,
}

impl Item {
    fn new(tag: &str, op: Operation) -> Self {
        let segments: Vec<String> = naming::segments(&op.path)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let root = naming::root_of(tag, &refs);
        let rest = match root {
            Some(i) => (i + 1..segments.len()).collect(),
            None => naming::unrooted(&refs),
        };
        Self {
            op,
            segments,
            rest,
            rooted: root.is_some(),
        }
    }

    /// The first literal segment below the resource, unless it names an action (`cancel`, not
    /// `test_clocks`): its index, and the singular snake_case noun it is grouped by.
    fn candidate(&self) -> Option<(usize, String)> {
        let i = *self
            .rest
            .iter()
            .find(|i| !naming::is_param(&self.segments[**i]))?;
        let words = naming::snake(&self.segments[i]);
        let action = naming::is_verb(&words) && !naming::is_plural(&words);
        let valid = !action && !naming::is_prefix(&self.segments[i]);
        valid.then(|| (i, naming::singular(&words)))
    }

    /// Whether the path goes on below its segment `i`.
    fn continues(&self, i: usize) -> bool {
        self.rest.iter().any(|j| *j > i)
    }

    fn below(&mut self, i: usize) {
        self.rest.retain(|j| *j > i);
    }
}

/// Distinct operations: a `_stream` twin shares the id of the operation it streams.
fn count<I: std::borrow::Borrow<Item>>(items: &[I]) -> usize {
    items
        .iter()
        .map(|item| &item.borrow().op.id)
        .unique()
        .count()
}

/// Places `items` in the resource at `path`, or in children of it for the segments holding two
/// operations or more: collections (`peers`) and the segments paths go on below (`issuing` of
/// `/issuing/cards`), while a single object (`discount`) names methods.
fn split(
    path: Vec<String>,
    mut items: Vec<Item>,
    taken: &mut BTreeMap<String, Vec<String>>,
    placed: &mut BTreeMap<Vec<String>, Vec<Operation>>,
) {
    // A tag its paths do not name, as `connect` for `/connected-accounts`, is rooted at the
    // segments they all share. Otherwise its unrooted paths (`/user/installations` in `apps`)
    // name methods: their first segments scope rather than nest.
    if path.len() == 1 && items.iter().all(|item| !item.rooted) {
        while count(&items) > 1 {
            let candidates: Vec<_> = items.iter().map(Item::candidate).collect();
            let Some(Some((_, noun))) = candidates.first() else {
                break;
            };
            if !candidates
                .iter()
                .all(|c| c.as_ref().is_some_and(|(_, n)| n == noun))
            {
                break;
            }
            for (item, (i, _)) in items.iter_mut().zip(candidates.into_iter().flatten()) {
                item.below(i);
                item.rooted = true;
            }
        }
    }
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let nests = item.rooted && path.len() < MAX_DEPTH;
        let Some((_, noun)) = item.candidate().filter(|_| nests) else {
            continue;
        };
        match groups.iter_mut().find(|(n, _)| *n == noun) {
            Some((_, members)) => members.push(index),
            None => groups.push((noun, vec![index])),
        }
    }
    let mut child_of: BTreeMap<usize, String> = BTreeMap::new();
    let names: Vec<String> = groups
        .iter()
        .map(|(_, members)| accessor(&members.iter().map(|i| &items[*i]).collect::<Vec<_>>()))
        .collect();
    let parent = path.last().map(|p| naming::singular(p));
    for ((_, members), name) in groups.iter().zip(&names) {
        let group: Vec<&Item> = members.iter().map(|i| &items[*i]).collect();
        let continues = group.iter().any(|item| {
            let (i, _) = item.candidate().expect("grouped");
            item.continues(i)
        });
        // `/teams/{id}/teams` lists teams: methods of `teams`, not `teams.teams`.
        if count(&group) < 2
            || !(continues || naming::is_plural(name))
            || parent == Some(naming::singular(name))
        {
            continue;
        }
        // `/checks/{id}/check-runs` is `checks.runs`, unless another child is named `runs`.
        let short = parent
            .as_ref()
            .and_then(|p| name.strip_prefix(&format!("{p}_")))
            .filter(|short| !names.iter().any(|n| n == short))
            .filter(|short| !child_of.values().any(|n| n == short));
        let chosen = short.into_iter().chain([name.as_str()]).find(|name| {
            let child: Vec<String> = path.iter().cloned().chain([(*name).to_owned()]).collect();
            !RESERVED.contains(name)
                && *taken.entry(child.join("_")).or_insert(child.clone()) == child
        });
        if let Some(name) = chosen {
            child_of.extend(members.iter().map(|i| (*i, name.to_owned())));
        }
    }
    let mut nested: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    let own = placed.entry(path.clone()).or_default();
    for (index, mut item) in items.into_iter().enumerate() {
        match child_of.remove(&index) {
            Some(name) => {
                let (i, _) = item.candidate().expect("grouped");
                item.below(i);
                item.op.root = Some(i);
                nested.entry(name).or_default().push(item);
            }
            None => own.push(item.op),
        }
    }
    for (name, items) in nested {
        let child = path.iter().cloned().chain([name]).collect();
        split(child, items, taken, placed);
    }
}

/// The accessor of a child resource: its segment as a tag, in the plural when one of its
/// operations spells it so.
fn accessor(group: &[&Item]) -> String {
    let spellings: Vec<String> = group
        .iter()
        .filter_map(|item| {
            let (i, _) = item.candidate()?;
            Some(super::resources::resource_name(&item.segments[i]))
        })
        .collect();
    spellings
        .iter()
        .find(|s| naming::is_plural(s))
        .or(spellings.first())
        .cloned()
        .unwrap_or_default()
}

/// The resources holding `placed`, those leading to them included, with their children.
fn build(placed: BTreeMap<Vec<String>, Vec<Operation>>) -> anyhow::Result<Resources> {
    let mut by_path: BTreeMap<Vec<String>, Resource> = BTreeMap::new();
    for (path, ops) in placed {
        if ops.is_empty() {
            continue;
        }
        for n in 1..path.len() {
            let prefix = path[..n].to_vec();
            by_path
                .entry(prefix.clone())
                .or_insert_with(|| Resource::new(prefix));
        }
        by_path
            .entry(path.clone())
            .or_insert_with(|| Resource::new(path))
            .operations
            .extend(ops);
    }
    let paths: Vec<Vec<String>> = by_path.keys().cloned().collect();
    for path in &paths {
        if let [parent @ .., leaf] = path.as_slice()
            && !parent.is_empty()
        {
            let name = path.join("_");
            let parent = by_path.get_mut(parent).expect("prefixes were inserted");
            parent.children.push(Child {
                name: name.clone(),
                accessor: leaf.clone(),
            });
            by_path.get_mut(path).expect("listed").parent = Some(parent_name(path));
        }
    }
    let mut resources = Resources::new();
    for (path, mut resource) in by_path {
        resource.disambiguate_operation_names();
        if let Some(other) = resources.insert(resource.name.clone(), resource) {
            bail!(
                "the resources `{}` and `{}` would both be named `{}`: set `x-perseid-resource` \
                 on their operations, or list them in the `[resources]` table of perseid.toml",
                other.path.join("."),
                path.join("."),
                other.name
            );
        }
    }
    Ok(resources)
}

fn parent_name(path: &[String]) -> String {
    path[..path.len() - 1].join("_")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use crate::spec::Filters;

    /// `(method, path, operation id, tag)` as a spec.
    fn spec(ops: &[(&str, &str, &str, &str)]) -> String {
        let mut paths = serde_json::Map::new();
        for (method, path, id, tag) in ops {
            let item = paths.entry(*path).or_insert_with(|| json!({}));
            item[*method] = json!({
                "operationId": id,
                "tags": [tag],
                "responses": { "204": { "description": "ok" } },
            });
        }
        json!({ "openapi": "3.1.0", "info": { "title": "A", "version": "1" }, "paths": paths })
            .to_string()
    }

    /// Method names by dotted resource path.
    fn tree(ops: &[(&str, &str, &str, &str)], filters: &Filters) -> BTreeMap<String, Vec<String>> {
        let api = crate::spec::api(&spec(ops), filters).unwrap();
        let api: Value = serde_json::to_value(&api).unwrap();
        let mut out = BTreeMap::new();
        for resource in api["resources"].as_array().unwrap() {
            let path: Vec<&str> = resource["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect();
            let mut names: Vec<String> = resource["operations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|op| op["name"].as_str().unwrap().to_owned())
                .collect();
            names.sort();
            out.insert(path.join("."), names);
        }
        out
    }

    fn expect(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(path, names)| {
                let names = names.iter().map(|n| (*n).to_owned()).collect();
                ((*path).to_owned(), names)
            })
            .collect()
    }

    #[test]
    fn segments_holding_two_operations_are_resources_and_single_ones_methods() {
        let ops = [
            ("get", "/workspaces", "listWorkspaces", "workspaces"),
            ("get", "/workspaces/{id}", "getWorkspace", "workspaces"),
            ("get", "/workspaces/{id}/usage", "getUsage", "workspaces"),
            ("get", "/workspaces/{id}/peers", "listPeers", "workspaces"),
            ("post", "/workspaces/{id}/peers", "addPeer", "workspaces"),
            ("get", "/workspaces/{id}/peers/{p}", "getPeer", "workspaces"),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("workspaces", &["list", "retrieve", "retrieve_usage"]),
                ("workspaces.peers", &["create", "list", "retrieve"]),
            ])
        );
    }

    #[test]
    fn an_endpoint_below_a_singular_segment_makes_it_a_resource() {
        let mut ops = vec![
            (
                "get",
                "/customers/{c}/cash_balance",
                "getBalance",
                "customers",
            ),
            (
                "post",
                "/customers/{c}/cash_balance",
                "updateBalance",
                "customers",
            ),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[("customers", &["cash_balance", "retrieve_cash_balance"])])
        );
        ops.push((
            "get",
            "/customers/{c}/cash_balance/transactions",
            "listTx",
            "customers",
        ));
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("customers", &[]),
                (
                    "customers.cash_balance",
                    &["create", "list_transactions", "retrieve"]
                ),
            ])
        );
    }

    #[test]
    fn lists_in_child_resources_are_paged_like_stripe_lists() {
        let params = json!([
            { "name": "c", "in": "path", "required": true, "schema": { "type": "string" } },
            { "name": "starting_after", "in": "query", "schema": { "type": "string" } },
            { "name": "ending_before", "in": "query", "schema": { "type": "string" } },
        ]);
        let ok = |schema: &str| {
            json!({ "200": { "description": "ok", "content": {
            "application/json": { "schema": { "$ref": format!("#/components/schemas/{schema}") } } } } })
        };
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "A", "version": "1" },
            "paths": {
                "/v1/customers/{c}/sources": {
                    "get": { "operationId": "ListSources", "tags": ["customers"], "parameters": params, "responses": ok("SourceList") },
                    "post": { "operationId": "CreateSource", "tags": ["customers"], "parameters": [params[0]], "responses": ok("Source") },
                },
            },
            "components": { "schemas": {
                "Source": { "type": "object", "required": ["id"], "properties": { "id": { "type": "string" } } },
                "SourceList": { "type": "object", "required": ["data", "has_more"], "properties": {
                    "data": { "type": "array", "items": { "$ref": "#/components/schemas/Source" } },
                    "has_more": { "type": "boolean" },
                } },
            } },
        });
        let filters = Filters {
            detect_pagination: true,
            ..Filters::default()
        };
        let api = crate::spec::api(&spec.to_string(), &filters).unwrap();
        let sources = &api.resources["customers_sources"];
        assert_eq!(sources.path, ["customers", "sources"]);
        let list = serde_json::to_value(&sources.operations[0]).unwrap();
        assert_eq!(list["name"], "list");
        assert_eq!(list["pagination"]["param"], "starting_after");
        assert_eq!(list["pagination"]["before"], "ending_before");
    }

    #[test]
    fn tag_and_header_descriptions_reach_the_model() {
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "A", "version": "1" },
            "tags": [{ "name": "Add-ons", "description": "Extras <b>sold</b> with plans." }],
            "paths": {
                "/add-ons/{a}/versions": {
                    "get": { "operationId": "ListVersions", "tags": ["Add-ons"], "parameters": [
                        { "name": "a", "in": "path", "required": true, "schema": { "type": "string" } },
                        { "name": "X-Tenant", "in": "header", "description": "The tenant.", "schema": { "type": "string" } },
                    ], "responses": { "204": { "description": "ok" } } },
                    "post": { "operationId": "CreateVersion", "tags": ["Add-ons"], "responses": { "204": { "description": "ok" } } },
                },
            },
        });
        let api = crate::spec::api(&spec.to_string(), &Filters::default()).unwrap();
        let root = &api.resources["add_ons"];
        assert_eq!(
            root.description.as_deref(),
            Some("Extras **sold** with plans.")
        );
        let child = &api.resources["add_ons_versions"];
        assert_eq!(child.description, None);
        let list = serde_json::to_value(&child.operations[0]).unwrap();
        assert_eq!(list["header_params"][0]["description"], "The tenant.");
    }

    #[test]
    fn nesting_stops_three_resources_deep() {
        let ops = [
            ("get", "/orgs/{o}/teams", "listTeams", "orgs"),
            ("post", "/orgs/{o}/teams", "createTeam", "orgs"),
            ("get", "/orgs/{o}/teams/{t}/members", "listMembers", "orgs"),
            ("post", "/orgs/{o}/teams/{t}/members", "addMember", "orgs"),
            (
                "get",
                "/orgs/{o}/teams/{t}/members/{m}/roles",
                "listRoles",
                "orgs",
            ),
            (
                "post",
                "/orgs/{o}/teams/{t}/members/{m}/roles",
                "addRole",
                "orgs",
            ),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("orgs", &[]),
                ("orgs.teams", &["create", "list"]),
                (
                    "orgs.teams.members",
                    &["create", "create_role", "list", "list_roles"]
                ),
            ])
        );
    }

    #[test]
    fn a_child_named_like_another_resource_stays_methods() {
        let ops = [
            ("get", "/orgs/{o}/teams", "listOrgTeams", "orgs"),
            ("post", "/orgs/{o}/teams", "createOrgTeam", "orgs"),
            ("get", "/org-teams", "listOrgsTeams", "orgs_teams"),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("orgs", &["create_team", "list_teams"]),
                ("orgs_teams", &["list_org_teams"])
            ])
        );
    }

    #[test]
    fn a_tag_its_paths_do_not_name_is_rooted_at_the_segment_they_share() {
        let ops = [
            ("get", "/connected-accounts", "listAccounts", "connect"),
            ("get", "/connected-accounts/{id}", "getAccount", "connect"),
            (
                "get",
                "/connected-accounts/{id}/payouts",
                "listPayouts",
                "connect",
            ),
            (
                "post",
                "/connected-accounts/{id}/payouts",
                "createPayout",
                "connect",
            ),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                (
                    "connect",
                    &["list_connected_accounts", "retrieve_connected_account"]
                ),
                ("connect.payouts", &["create", "list"]),
            ])
        );
    }

    #[test]
    fn children_drop_the_noun_of_their_parent_and_never_take_a_member_name() {
        let ops = [
            ("post", "/repos/{r}/check-runs", "createRun", "checks"),
            ("get", "/repos/{r}/check-runs/{id}", "getRun", "checks"),
            ("post", "/repos/{r}/check-suites", "createSuite", "checks"),
            ("get", "/repos/{r}/check-suites/{id}", "getSuite", "checks"),
            ("get", "/schools/{s}/client", "getClient", "schools"),
            (
                "get",
                "/schools/{s}/client/settings",
                "getClientSettings",
                "schools",
            ),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("checks", &[]),
                ("checks.runs", &["create", "retrieve"]),
                ("checks.suites", &["create", "retrieve"]),
                ("schools", &["list_client_settings", "retrieve_client"]),
            ])
        );
    }

    #[test]
    fn collections_set_and_delete_all_what_their_items_update_and_delete() {
        let ops = [
            ("get", "/groups/{g}/repositories", "listRepos", "groups"),
            ("put", "/groups/{g}/repositories", "setRepos", "groups"),
            ("put", "/groups/{g}/repositories/{r}", "addRepo", "groups"),
            (
                "delete",
                "/groups/{g}/repositories/{r}",
                "removeRepo",
                "groups",
            ),
            (
                "delete",
                "/groups/{g}/repositories",
                "removeRepos",
                "groups",
            ),
        ];
        assert_eq!(
            tree(&ops, &Filters::default()),
            expect(&[
                ("groups", &[]),
                (
                    "groups.repositories",
                    &["delete", "delete_all", "list", "set", "update"]
                ),
            ])
        );
    }

    #[test]
    fn leaving_operations_out_moves_no_other() {
        let ops = [
            ("get", "/plans", "listPlans", "plans"),
            ("get", "/plans/{id}/versions", "listVersions", "plans"),
            ("put", "/plans/versions/{v}/minimum", "setMinimum", "plans"),
            (
                "delete",
                "/plans/versions/{v}/minimum",
                "deleteMinimum",
                "plans",
            ),
        ];
        let all = tree(&ops, &Filters::default());
        assert_eq!(
            all["plans.versions"],
            ["delete_minimum", "list", "update_minimum"]
        );
        let filters = Filters {
            excluded: ["setMinimum".to_owned(), "deleteMinimum".to_owned()].into(),
            ..Filters::default()
        };
        assert_eq!(
            tree(&ops, &filters),
            expect(&[("plans", &["list"]), ("plans.versions", &["list"])])
        );
    }

    #[test]
    fn overrides_place_operations_and_must_be_resource_paths() {
        let ops = [
            ("get", "/audit-logs", "listLogs", "admin"),
            ("get", "/users", "listUsers", "admin"),
        ];
        let filters = |path: &str| Filters {
            resources: [("listLogs".to_owned(), path.to_owned())].into(),
            ..Filters::default()
        };
        assert_eq!(
            tree(&ops, &filters("admin.audit.logs")),
            expect(&[
                ("admin", &["list_users"]),
                ("admin.audit", &[]),
                ("admin.audit.logs", &["list_audit_logs"]),
            ])
        );
        let error = crate::spec::api(&spec(&ops), &filters("admin.audit.logs.all"))
            .err()
            .unwrap();
        assert!(
            format!("{error:#}").contains("nests 4 resources"),
            "{error:#}"
        );
        assert_eq!(
            super::resource_path("a.b"),
            Ok(vec!["a".to_owned(), "b".to_owned()])
        );
        assert!(super::resource_path("a.with_raw_response").is_err());
    }
}
