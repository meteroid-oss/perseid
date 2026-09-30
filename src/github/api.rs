use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use ureq::{Agent, http};

/// The REST API root, `PERSEID_GITHUB_API` in tests.
pub fn api_base() -> String {
    base("PERSEID_GITHUB_API", "https://api.github.com")
}

/// The web root serving OAuth and App pages, `PERSEID_GITHUB_WEB` in tests.
pub fn web_base() -> String {
    base("PERSEID_GITHUB_WEB", "https://github.com")
}

fn base(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.into())
        .trim_end_matches('/')
        .to_owned()
}

fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(concat!("perseid/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into()
}

pub struct Reply {
    pub status: u16,
    pub body: Value,
    /// Scopes of a classic token, absent for other tokens.
    pub scopes: Option<String>,
}

/// A GitHub REST client authenticated with a user token, an App JWT, or nothing.
pub struct GitHub {
    agent: Agent,
    base: String,
    token: Option<String>,
}

impl GitHub {
    pub fn new(token: Option<String>) -> Self {
        Self {
            agent: agent(),
            base: api_base(),
            token,
        }
    }

    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply> {
        let (status, text, scopes) =
            self.exchange(method, path, body, "application/vnd.github+json")?;
        Ok(Reply {
            status,
            body: serde_json::from_str(&text).unwrap_or(Value::Null),
            scopes,
        })
    }

    /// The raw bytes of a file of a repository, `None` when it or the repository is missing.
    pub fn raw(&self, repo: &str, branch: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("/repos/{repo}/contents/{path}?ref={branch}");
        let (status, text, scopes) =
            self.exchange("GET", &url, None, "application/vnd.github.raw")?;
        match status {
            404 | 409 => Ok(None),
            200 => Ok(Some(text.into_bytes())),
            _ => {
                let body = serde_json::from_str(&text).unwrap_or(Value::Null);
                check(
                    "GET",
                    &url,
                    Reply {
                        status,
                        body,
                        scopes,
                    },
                )
                .map(|_| None)
            }
        }
    }

    fn exchange(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        accept: &str,
    ) -> Result<(u16, String, Option<String>)> {
        let url = format!("{}{path}", self.base);
        let mut request = http::Request::builder()
            .method(method)
            .uri(&url)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = match body {
            None => self.agent.run(request.body(())?),
            Some(body) => self.agent.run(
                request
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(body)?)?,
            ),
        };
        let mut response = response.with_context(|| format!("reaching GitHub ({method} {url})"))?;
        let scopes = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = response
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .read_to_string()?;
        Ok((response.status().as_u16(), text, scopes))
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.expect("GET", path, None)
    }

    /// The resource at `path`, or `None` when GitHub answers 404.
    pub fn find(&self, path: &str) -> Result<Option<Value>> {
        let reply = self.send("GET", path, None)?;
        match reply.status {
            404 => Ok(None),
            _ => check("GET", path, reply).map(Some),
        }
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.expect("POST", path, Some(&body))
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.expect("PUT", path, Some(&body))
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        self.expect("DELETE", path, None).map(|_| ())
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.expect("PATCH", path, Some(&body))
    }

    fn expect(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        check(method, path, self.send(method, path, body)?)
    }
}

pub fn check(method: &str, path: &str, reply: Reply) -> Result<Value> {
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    let message = reply.body["message"].as_str().unwrap_or("no details");
    let hint = match reply.status {
        401 => ": the token is invalid or expired",
        403 if message.contains("OAuth App access restrictions") => {
            ": the organization restricts OAuth apps, approve perseid in its settings or set GH_TOKEN"
        }
        403 | 404 => ": check that the token can administer this repository",
        _ => "",
    };
    bail!(
        "GitHub answered {} to {method} {path}: {message}{hint}",
        reply.status
    )
}

/// Posts an OAuth form to the web root, which answers JSON when asked to.
pub fn post_form(path: &str, form: &[(&str, &str)]) -> Result<Value> {
    let url = format!("{}{path}", web_base());
    let mut response = agent()
        .post(&url)
        .header("Accept", "application/json")
        .send_form(form.iter().copied())
        .with_context(|| format!("reaching GitHub ({url})"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    let body: Value = serde_json::from_str(&text)
        .with_context(|| format!("GitHub answered {status} to {url} without JSON"))?;
    Ok(body)
}
