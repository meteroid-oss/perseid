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
}

pub(crate) struct Security {
    pub(crate) schemes: Vec<SecurityScheme>,
    /// Requirement shared by most operations, applied by the client unless an operation overrides it.
    pub(crate) default: Requirement,
    effective: BTreeMap<String, Requirement>,
}

impl Security {
    pub(crate) fn from_spec(spec: &Value) -> Self {
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
            }],
        };
        let global = match spec.get("security") {
            Some(security) => requirement(security),
            None if declared.is_none_or(|d| d.is_empty()) => vec![vec![IMPLICIT_BEARER.to_owned()]],
            None => Vec::new(),
        };

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
            let Some(id) = op["operationId"].as_str() else {
                continue;
            };
            let req = op
                .get("security")
                .map_or_else(|| global.clone(), requirement);
            match counts.iter_mut().find(|(r, _)| *r == req) {
                Some((_, count)) => *count += 1,
                None => counts.push((req.clone(), 1)),
            }
            effective.insert(id.to_owned(), req);
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
        Self {
            schemes,
            default,
            effective,
        }
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
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn specs_without_schemes_keep_sending_the_bearer_token() {
        let security =
            Security::from_spec(&json!({"paths": {"/a": {"get": {"operationId": "a"}}}}));
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
        let security = Security::from_spec(&spec);
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
        let security = Security::from_spec(&spec);
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
    }

    #[test]
    fn global_security_is_the_default() {
        let spec = json!({
            "security": [{"b": []}],
            "components": {"securitySchemes": {"b": {"type": "http", "scheme": "bearer"}}},
            "paths": {"/a": {"get": {"operationId": "a", "security": []}}, "/b": {"get": {"operationId": "b", "security": []}}}
        });
        let security = Security::from_spec(&spec);
        assert_eq!(security.default, vec![vec!["b".to_owned()]]);
        assert_eq!(security.override_for("a"), Some(vec![]));
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
