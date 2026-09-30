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
    /// OAuth2 client credentials token endpoint, when the spec declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) token_url: Option<String>,
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
        let schemes: Vec<_> = match declared {
            Some(declared) if !declared.is_empty() => declared
                .iter()
                .filter_map(|(name, scheme)| parse_scheme(name, scheme))
                .collect(),
            _ => vec![SecurityScheme {
                name: IMPLICIT_BEARER.to_owned(),
                kind: SchemeKind::Bearer,
                location: None,
                param: None,
                description: None,
                token_url: None,
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

fn parse_scheme(name: &str, scheme: &Value) -> Option<SecurityScheme> {
    let text = |key: &str| scheme[key].as_str().map(str::to_owned);
    let (kind, location, param, token_url) = match scheme["type"].as_str() {
        Some("http") => match text("scheme")?.to_ascii_lowercase().as_str() {
            "bearer" => (SchemeKind::Bearer, None, None, None),
            "basic" => (SchemeKind::Basic, None, None, None),
            other => {
                tracing::warn!(
                    name,
                    scheme = other,
                    "unsupported http auth scheme, ignored"
                );
                return None;
            }
        },
        Some("apiKey") => (SchemeKind::ApiKey, text("in"), text("name"), None),
        Some("oauth2") => {
            let token_url = scheme["flows"]["clientCredentials"]["tokenUrl"]
                .as_str()
                .map(str::to_owned);
            (SchemeKind::Bearer, None, None, token_url)
        }
        Some("openIdConnect") => (SchemeKind::Bearer, None, None, None),
        other => {
            tracing::warn!(name, kind = ?other, "unsupported security scheme, ignored");
            return None;
        }
    };
    Some(SecurityScheme {
        name: name.to_owned(),
        kind,
        location,
        param,
        description: text("description"),
        token_url,
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
        let key = security.schemes.iter().find(|s| s.name == "key").unwrap();
        assert_eq!(
            (key.location.as_deref(), key.param.as_deref()),
            (Some("query"), Some("api_key"))
        );
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
}
