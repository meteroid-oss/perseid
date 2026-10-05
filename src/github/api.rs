use std::{sync::Mutex, time::Duration};

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
    crate::http::config(Duration::from_secs(60))
        .http_status_as_error(false)
        .build()
        .into()
}

pub struct Reply {
    pub status: u16,
    pub body: Value,
}

/// A GitHub REST client authenticated with a user token, an App JWT, or nothing.
pub struct GitHub {
    agent: Agent,
    base: String,
    token: Option<String>,
    expiration: Mutex<Option<String>>,
}

impl GitHub {
    pub fn new(token: Option<String>) -> Self {
        Self {
            agent: agent(),
            base: api_base(),
            token,
            expiration: Mutex::default(),
        }
    }

    /// When the token expires, as GitHub last wrote it (`2026-11-01 12:00:00 UTC`), for tokens
    /// that do.
    pub fn expiration(&self) -> Option<String> {
        self.expiration.lock().ok()?.clone()
    }

    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply> {
        let (status, text) = self.exchange(method, path, body, "application/vnd.github+json")?;
        Ok(Reply {
            status,
            body: serde_json::from_str(&text).unwrap_or(Value::Null),
        })
    }

    /// The raw bytes of a file of a repository, `None` when it or the repository is missing.
    pub fn raw(&self, repo: &str, branch: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("/repos/{repo}/contents/{path}?ref={branch}");
        let (status, text) = self.exchange("GET", &url, None, "application/vnd.github.raw")?;
        match status {
            404 | 409 => Ok(None),
            200 => Ok(Some(text.into_bytes())),
            _ => {
                let body = serde_json::from_str(&text).unwrap_or(Value::Null);
                check("GET", &url, Reply { status, body }).map(|_| None)
            }
        }
    }

    fn exchange(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        accept: &str,
    ) -> Result<(u16, String)> {
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
        if let Some(date) = response
            .headers()
            .get("github-authentication-token-expiration")
            .and_then(|v| v.to_str().ok())
            && let Ok(mut expiration) = self.expiration.lock()
        {
            *expiration = Some(date.to_owned());
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .read_to_string()?;
        Ok((response.status().as_u16(), text))
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

/// Seconds since the Unix epoch of a token expiration date: `2026-11-01 12:00:00 UTC`, or with
/// a `+0200` offset.
pub fn expiration_epoch(date: &str) -> Option<i64> {
    let mut parts = date.split_whitespace();
    let [year, month, day] = numbers(parts.next()?, '-')?;
    let [hour, minute, second] = numbers(parts.next()?, ':')?;
    let offset = match parts.next().unwrap_or("UTC") {
        "UTC" | "GMT" | "Z" => 0,
        zone => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(3..5)?.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86400 + hour * 3600 + minute * 60 + second - offset)
}

fn numbers<const N: usize>(text: &str, separator: char) -> Option<[i64; N]> {
    let parsed: Vec<i64> = text
        .split(separator)
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    parsed.try_into().ok()
}

/// Days since 1970-01-01 of a Gregorian date, after Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

#[cfg(test)]
mod tests {
    use super::expiration_epoch;

    #[test]
    fn token_expiration_dates_parse_to_epoch_seconds() {
        assert_eq!(expiration_epoch("1970-01-01 00:00:00 UTC"), Some(0));
        assert_eq!(
            expiration_epoch("2026-11-01 12:00:00 UTC"),
            Some(1_793_534_400)
        );
        assert_eq!(
            expiration_epoch("2024-02-29 00:00:00 UTC"),
            Some(1_709_164_800)
        );
        assert_eq!(
            expiration_epoch("2026-11-01 14:30:00 +0230"),
            expiration_epoch("2026-11-01 12:00:00 UTC")
        );
        assert_eq!(expiration_epoch("2026-11-01"), None);
        assert_eq!(expiration_epoch("soon"), None);
    }
}
