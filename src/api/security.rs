use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Alternatives of scheme sets: the first alternative whose credentials are all configured is sent.
pub(crate) type Requirement = Vec<Vec<String>>;

/// Scheme name used when the spec declares no `securitySchemes`, preserving bearer-token clients.
const IMPLICIT_BEARER: &str = "bearer";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SchemeKind {
    /// `Authorization: Bearer`: http bearer, oauth2 and openIdConnect.
    Bearer,
    Basic,
    ApiKey,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct SecurityScheme {
    pub(crate) name: String,
    pub(crate) kind: SchemeKind,
    /// `header`, `query` or `cookie`, for API keys.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<String>,
    /// Header, query parameter or cookie name, for API keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// OAuth2 client credentials token endpoint, when the spec declares one: absolute, or relative
    /// to the server URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) token_url: Option<String>,
    /// The space-separated scopes the operations require of an OAuth2 scheme, asked for with the
    /// token. Empty when no requirement names one.
    pub(crate) scope: String,
    /// The OAuth2 flows a person logs in with, for programs such as CLIs: `authorization_code`
    /// and `device_authorization` (OpenAPI 3.2), by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) flows: BTreeMap<String, Flow>,
}

/// An OAuth2 flow of a scheme, its URLs as written: absolute, or relative to the server URL.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(crate) struct Flow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) authorization_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) device_authorization_url: Option<String>,
    pub(crate) token_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_url: Option<String>,
    /// The scopes the flow declares, sorted.
    pub(crate) scopes: Vec<String>,
}

pub(crate) struct Security {
    pub(crate) schemes: Vec<SecurityScheme>,
    /// Requirement shared by most operations, applied by the client unless an operation overrides it.
    pub(crate) default: Requirement,
    effective: BTreeMap<String, Requirement>,
}

impl Security {
    /// Reads the requirements of the operations `generated` keeps.
    pub(crate) fn from_spec(
        spec: &Value,
        generated: impl Fn(&str) -> bool,
    ) -> anyhow::Result<Self> {
        let declared = spec["components"]["securitySchemes"].as_object();
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut schemes: Vec<_> = match declared {
            Some(declared) if !declared.is_empty() => declared
                .iter()
                .filter_map(|(name, scheme)| match parse_scheme(name, scheme) {
                    Ok(scheme) => Some(scheme),
                    Err(reason) => {
                        skipped.push((name.clone(), reason));
                        None
                    }
                })
                .collect(),
            _ => vec![SecurityScheme {
                name: IMPLICIT_BEARER.to_owned(),
                kind: SchemeKind::Bearer,
                location: None,
                param: None,
                description: None,
                token_url: None,
                scope: String::new(),
                flows: BTreeMap::new(),
            }],
        };
        let known = |name: &String| {
            schemes.iter().any(|s| &s.name == name) || skipped.iter().any(|(n, _)| n == name)
        };
        let usable = |alt: &Vec<String>| alt.iter().all(|n| schemes.iter().any(|s| &s.name == n));
        let mut dropped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut global = match spec.get("security") {
            Some(security) => requirement(security),
            None if declared.is_none_or(|d| d.is_empty()) => vec![vec![IMPLICIT_BEARER.to_owned()]],
            None => Vec::new(),
        };
        for name in drop_undeclared(&mut global, known, usable) {
            dropped
                .entry(name)
                .or_default()
                .push("the global `security`".to_owned());
        }

        let mut effective = BTreeMap::new();
        let mut counts: Vec<(Requirement, usize)> = Vec::new();
        let operations = spec["paths"]
            .as_object()
            .into_iter()
            .flat_map(|paths| paths.values())
            .filter_map(Value::as_object)
            .flat_map(|item| item.iter())
            .filter(|(method, _)| HTTP_METHODS.contains(&method.as_str()));
        for (_, op) in operations {
            let Some(id) = op["operationId"].as_str().filter(|id| generated(id)) else {
                continue;
            };
            let req = match op.get("security") {
                Some(security) => {
                    let mut req = requirement(security);
                    for name in drop_undeclared(&mut req, known, usable) {
                        dropped.entry(name).or_default().push(format!("`{id}`"));
                    }
                    req
                }
                None => global.clone(),
            };
            match counts.iter_mut().find(|(r, _)| *r == req) {
                Some((_, count)) => *count += 1,
                None => counts.push((req.clone(), 1)),
            }
            effective.insert(id.to_owned(), req);
        }
        for (name, users) in &dropped {
            tracing::warn!(
                "{} also accept{} the security scheme `{name}`, which `components.securitySchemes` \
                 does not declare, so perseid only sends their other alternatives",
                users.join(", "),
                if users.len() > 1 { "" } else { "s" }
            );
        }
        let mut undefined: Vec<&String> = std::iter::once(&global)
            .chain(effective.values())
            .flatten()
            .flatten()
            .filter(|name| !known(name))
            .collect();
        undefined.sort_unstable();
        undefined.dedup();
        if let Some(name) = undefined.first() {
            let affected: Vec<&str> = effective
                .iter()
                .filter(|(_, req)| req.iter().flatten().any(|n| n == *name))
                .map(|(id, _)| id.as_str())
                .collect();
            let used = match affected.as_slice() {
                [] => "the global `security`".to_owned(),
                ops => operation_list(ops),
            };
            anyhow::bail!(
                "{used} require{s} the security scheme `{name}`, which `components.securitySchemes` \
                 does not declare: declare it, or leave the operations out with `exclude`",
                s = if affected.len() > 1 { "" } else { "s" }
            );
        }
        for (name, reason) in &skipped {
            let affected: Vec<&str> = effective
                .iter()
                .filter(|(_, req)| req.iter().flatten().any(|n| n == name))
                .map(|(id, _)| id.as_str())
                .collect();
            let used = match affected.as_slice() {
                [] => "no operation uses it".to_owned(),
                ops => format!("it is ignored for {}", operation_list(ops)),
            };
            tracing::warn!(
                "security scheme `{name}` {reason}, so perseid sends no credentials for it ({used}); \
                 use `bearer`, `basic`, an `apiKey` or `oauth2` scheme, or pass the header \
                 yourself with the client's extra headers"
            );
        }
        for scheme in schemes.iter_mut().filter(|s| s.token_url.is_some()) {
            scheme.scope = required_scopes(spec, &scheme.name);
        }
        let default = if spec.get("security").is_some() {
            global
        } else {
            let most = counts.iter().map(|(_, c)| *c).max().unwrap_or(0);
            counts
                .into_iter()
                .find(|(_, c)| *c == most)
                .map_or(global, |(r, _)| r)
        };
        Ok(Self {
            schemes,
            default,
            effective,
        })
    }

    /// The requirement of an operation, when it differs from [`Self::default`].
    pub(crate) fn override_for(&self, operation_id: &str) -> Option<Requirement> {
        self.effective
            .get(operation_id)
            .filter(|req| **req != self.default)
            .cloned()
    }
}

/// The scopes the global and operation requirements ask of the scheme `name`, sorted and joined.
fn required_scopes(spec: &Value, name: &str) -> String {
    let operations = spec["paths"]
        .as_object()
        .into_iter()
        .flat_map(|paths| paths.values())
        .filter_map(Value::as_object)
        .flat_map(|item| item.iter())
        .filter(|(method, _)| HTTP_METHODS.contains(&method.as_str()))
        .map(|(_, op)| &op["security"]);
    let mut scopes: Vec<&str> = std::iter::once(&spec["security"])
        .chain(operations)
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|alternative| alternative[name].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    scopes.sort_unstable();
    scopes.dedup();
    scopes.join(" ")
}

const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Drops the alternatives naming an undeclared scheme when another one can be sent, returning the
/// undeclared schemes dropped.
fn drop_undeclared(
    req: &mut Requirement,
    known: impl Fn(&String) -> bool,
    usable: impl Fn(&Vec<String>) -> bool,
) -> Vec<String> {
    if !req.iter().any(usable) {
        return Vec::new();
    }
    let mut dropped = Vec::new();
    req.retain(|alt| {
        let before = dropped.len();
        dropped.extend(alt.iter().filter(|n| !known(n)).cloned());
        dropped.len() == before
    });
    if req.iter().all(Vec::is_empty) {
        req.clear();
    }
    dropped.sort_unstable();
    dropped.dedup();
    dropped
}

fn requirement(value: &Value) -> Requirement {
    let alternatives: Requirement = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|alt| alt.keys().cloned().collect())
        .collect();
    if alternatives.iter().all(Vec::is_empty) {
        return Vec::new();
    }
    alternatives
}

/// Up to five operation ids, then how many more.
fn operation_list(ops: &[&str]) -> String {
    let shown: Vec<String> = ops.iter().take(5).map(|id| format!("`{id}`")).collect();
    match ops.len().saturating_sub(5) {
        0 => format!(
            "operation{} {}",
            if ops.len() == 1 { "" } else { "s" },
            shown.join(", ")
        ),
        more => format!("operations {} and {more} more", shown.join(", ")),
    }
}

/// The scheme, or why perseid does not support it.
fn parse_scheme(name: &str, scheme: &Value) -> Result<SecurityScheme, String> {
    let text = |key: &str| scheme[key].as_str().map(str::to_owned);
    let (kind, location, param, token_url) = match scheme["type"].as_str() {
        Some("http") => match text("scheme")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "bearer" => (SchemeKind::Bearer, None, None, None),
            "basic" => (SchemeKind::Basic, None, None, None),
            "" => return Err("is `type: http` without a `scheme`".to_owned()),
            other => return Err(format!("uses the unsupported http auth scheme `{other}`")),
        },
        Some("apiKey") => (SchemeKind::ApiKey, text("in"), text("name"), None),
        Some("oauth2") => {
            // A relative URL is kept as written: the runtimes resolve it against the base URL.
            let token_url = scheme["flows"]["clientCredentials"]["tokenUrl"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(str::to_owned);
            (SchemeKind::Bearer, None, None, token_url)
        }
        Some("openIdConnect") => (SchemeKind::Bearer, None, None, None),
        Some(other) => return Err(format!("has the unsupported type `{other}`")),
        None => return Err("has no `type`".to_owned()),
    };
    Ok(SecurityScheme {
        name: name.to_owned(),
        kind,
        location,
        param,
        description: text("description"),
        token_url,
        scope: String::new(),
        flows: interactive_flows(scheme),
    })
}

/// The `authorizationCode` and `deviceAuthorization` flows of an `oauth2` scheme with a token URL.
fn interactive_flows(scheme: &Value) -> BTreeMap<String, Flow> {
    let text = |flow: &Value, key: &str| {
        (flow[key].as_str())
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
    };
    [
        ("authorizationCode", "authorization_code"),
        ("deviceAuthorization", "device_authorization"),
    ]
    .into_iter()
    .filter(|_| scheme["type"] == "oauth2")
    .filter_map(|(key, name)| {
        let flow = &scheme["flows"][key];
        let mut scopes: Vec<String> = (flow["scopes"].as_object())
            .map(|scopes| scopes.keys().cloned().collect())
            .unwrap_or_default();
        scopes.sort();
        let flow = Flow {
            authorization_url: text(flow, "authorizationUrl"),
            device_authorization_url: text(flow, "deviceAuthorizationUrl"),
            token_url: text(flow, "tokenUrl")?,
            refresh_url: text(flow, "refreshUrl"),
            scopes,
        };
        Some((name.to_owned(), flow))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn specs_without_schemes_keep_sending_the_bearer_token() {
        let security = Security::from_spec(
            &json!({"paths": {"/a": {"get": {"operationId": "a"}}}}),
            |_| true,
        )
        .unwrap();
        assert_eq!(security.schemes[0].kind, SchemeKind::Bearer);
        assert_eq!(security.default, vec![vec!["bearer".to_owned()]]);
        assert_eq!(security.override_for("a"), None);
    }

    #[test]
    fn operation_requirements_override_the_most_common_one() {
        let spec = json!({
            "components": {"securitySchemes": {
                "key": {"type": "apiKey", "in": "query", "name": "api_key"},
                "basic": {"type": "http", "scheme": "Basic"},
                "oauth": {"type": "oauth2", "flows": {"clientCredentials": {"tokenUrl": "https://t", "scopes": {}}}},
            }},
            "paths": {
                "/a": {"get": {"operationId": "a", "security": [{"key": []}]}},
                "/b": {"get": {"operationId": "b", "security": [{"key": []}]}, "parameters": []},
                "/c": {"post": {"operationId": "c", "security": []}},
                "/d": {"post": {"operationId": "d", "security": [{}, {"basic": [], "key": []}]}},
                "/e": {"get": {"operationId": "e"}},
            }
        });
        let security = Security::from_spec(&spec, |_| true).unwrap();
        assert_eq!(security.default, vec![vec!["key".to_owned()]]);
        assert_eq!(security.override_for("a"), None);
        assert_eq!(security.override_for("c"), Some(vec![]));
        assert_eq!(security.override_for("e"), Some(vec![]));
        assert_eq!(
            security.override_for("d"),
            Some(vec![vec![], vec!["basic".to_owned(), "key".to_owned()]])
        );
        let oauth = security.schemes.iter().find(|s| s.name == "oauth").unwrap();
        assert_eq!(oauth.token_url.as_deref(), Some("https://t"));
        assert_eq!(oauth.scope, "");
        let key = security.schemes.iter().find(|s| s.name == "key").unwrap();
        assert_eq!(
            (key.location.as_deref(), key.param.as_deref()),
            (Some("query"), Some("api_key"))
        );
    }

    #[test]
    fn oauth_schemes_ask_for_the_scopes_requirements_name() {
        let spec = json!({
            "security": [{"oauth": ["write", "read"]}],
            "components": {"securitySchemes": {
                "oauth": {"type": "oauth2", "flows": {"clientCredentials": {"tokenUrl": "/token", "scopes": {"admin": ""}}}},
                "plain": {"type": "oauth2", "flows": {"authorizationCode": {"authorizationUrl": "/a", "tokenUrl": "/t", "scopes": {}}}},
            }},
            "paths": {
                "/a": {"get": {"operationId": "a", "security": [{"oauth": ["read", "list"]}, {"plain": ["x"]}]}},
                "/b": {"get": {"operationId": "b"}},
            }
        });
        let security = Security::from_spec(&spec, |_| true).unwrap();
        let oauth = security.schemes.iter().find(|s| s.name == "oauth").unwrap();
        assert_eq!(oauth.token_url.as_deref(), Some("/token"));
        assert_eq!(oauth.scope, "list read write");
        let relative = parse_scheme(
            "r",
            &json!({"type": "oauth2", "flows": {"clientCredentials": {"tokenUrl": "oauth/token", "scopes": {}}}}),
        )
        .unwrap();
        assert_eq!(relative.token_url.as_deref(), Some("oauth/token"));
        let plain = security.schemes.iter().find(|s| s.name == "plain").unwrap();
        assert_eq!(plain.token_url, None);
        assert_eq!(plain.scope, "");
        assert!(oauth.flows.is_empty());
        let code = &plain.flows["authorization_code"];
        assert_eq!(
            (code.authorization_url.as_deref(), code.token_url.as_str()),
            (Some("/a"), "/t")
        );
    }

    #[test]
    fn interactive_flows_are_kept_for_programs_logging_people_in() {
        let scheme = parse_scheme(
            "o",
            &json!({"type": "oauth2", "flows": {
                "authorizationCode": {"authorizationUrl": "https://a/authorize", "tokenUrl": "https://a/token", "refreshUrl": "https://a/refresh", "scopes": {"write": "", "read": ""}},
                "deviceAuthorization": {"deviceAuthorizationUrl": "https://a/device", "tokenUrl": "https://a/token", "scopes": {}},
                "implicit": {"authorizationUrl": "https://a/authorize", "scopes": {}},
            }}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&scheme.flows).unwrap(),
            json!({
                "authorization_code": {"authorization_url": "https://a/authorize", "token_url": "https://a/token", "refresh_url": "https://a/refresh", "scopes": ["read", "write"]},
                "device_authorization": {"device_authorization_url": "https://a/device", "token_url": "https://a/token", "scopes": []},
            })
        );
    }

    #[test]
    fn global_security_is_the_default() {
        let spec = json!({
            "security": [{"b": []}],
            "components": {"securitySchemes": {"b": {"type": "http", "scheme": "bearer"}}},
            "paths": {"/a": {"get": {"operationId": "a", "security": []}}, "/b": {"get": {"operationId": "b", "security": []}}}
        });
        let security = Security::from_spec(&spec, |_| true).unwrap();
        assert_eq!(security.default, vec![vec!["b".to_owned()]]);
        assert_eq!(security.override_for("a"), Some(vec![]));
    }

    #[test]
    fn undeclared_schemes_are_errors() {
        let spec = json!({
            "components": {"securitySchemes": {
                "bearer": {"type": "http", "scheme": "bearer"},
                "digest": {"type": "http", "scheme": "digest"},
            }},
            "paths": {
                "/a": {"get": {"operationId": "a", "security": [{"beraer": []}]}},
                "/b": {"get": {"operationId": "b", "security": [{}, {"digest": []}]}},
                "/c": {"get": {"operationId": "c", "security": [{"bearer": []}]}},
            }
        });
        let err = Security::from_spec(&spec, |_| true).err().unwrap();
        assert_eq!(
            err.to_string(),
            "operation `a` requires the security scheme `beraer`, which \
             `components.securitySchemes` does not declare: declare it, or leave the operations \
             out with `exclude`"
        );
        let global = json!({"security": [{"key": []}], "paths": {}});
        let err = Security::from_spec(&global, |_| true).err().unwrap();
        assert!(
            err.to_string()
                .starts_with("the global `security` requires the security scheme `key`")
        );
        let implicit =
            json!({"paths": {"/a": {"get": {"operationId": "a", "security": [{"bearer": []}]}}}});
        assert!(Security::from_spec(&implicit, |_| true).is_ok());
    }

    #[test]
    fn operations_left_out_of_the_sdk_are_not_checked() {
        let spec = json!({
            "components": {"securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}}},
            "paths": {
                "/a": {"get": {"operationId": "a", "security": [{"api_key": []}]}},
                "/b": {"get": {"operationId": "b", "security": [{"bearer": []}]}},
            }
        });
        let security = Security::from_spec(&spec, |id| id == "b").unwrap();
        assert_eq!(security.default, vec![vec!["bearer".to_owned()]]);
        assert_eq!(security.override_for("a"), None);
    }

    #[test]
    fn undeclared_alternatives_are_dropped_when_another_can_be_sent() {
        let spec = json!({
            "security": [{"bearer": []}, {"api_key": []}],
            "components": {"securitySchemes": {
                "bearer": {"type": "http", "scheme": "bearer"},
                "digest": {"type": "http", "scheme": "digest"},
            }},
            "paths": {
                "/a": {"get": {"operationId": "a"}},
                "/b": {"get": {"operationId": "b", "security": [{}, {"api_key": []}]}},
                "/c": {"get": {"operationId": "c", "security": [{"digest": []}, {"api_key": []}]}},
            }
        });
        let err = Security::from_spec(&spec, |_| true).err().unwrap();
        assert!(
            err.to_string()
                .starts_with("operation `c` requires the security scheme `api_key`")
        );
        let security = Security::from_spec(&spec, |id| id != "c").unwrap();
        assert_eq!(security.default, vec![vec!["bearer".to_owned()]]);
        assert_eq!(security.override_for("b"), Some(vec![]));
    }

    #[test]
    fn unsupported_schemes_say_what_they_are() {
        let err = parse_scheme("d", &json!({"type": "http", "scheme": "Digest"})).unwrap_err();
        assert_eq!(err, "uses the unsupported http auth scheme `digest`");
        assert_eq!(operation_list(&["a"]), "operation `a`");
        assert_eq!(
            operation_list(&["a", "b", "c", "d", "e", "f", "g"]),
            "operations `a`, `b`, `c`, `d`, `e` and 2 more"
        );
    }
}
