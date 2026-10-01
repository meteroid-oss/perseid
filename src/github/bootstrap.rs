use std::collections::BTreeSet;

use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use super::api::{GitHub, check};

pub struct File {
    pub path: String,
    pub content: Vec<u8>,
}

/// The text of `path` on `branch`, `None` when missing or when the repository is empty.
pub fn read(api: &GitHub, repo: &str, branch: &str, path: &str) -> Result<Option<String>> {
    let url = format!("/repos/{repo}/contents/{path}?ref={branch}");
    let reply = api.send("GET", &url, None)?;
    if matches!(reply.status, 404 | 409) {
        return Ok(None);
    }
    let body = check("GET", &url, reply)?;
    let encoded: String = body["content"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    Ok(Some(String::from_utf8(BASE64.decode(encoded)?)?))
}

pub fn paths(api: &GitHub, repo: &str, branch: &str) -> Result<BTreeSet<String>> {
    let url = format!("/repos/{repo}/git/trees/{branch}?recursive=1");
    let reply = api.send("GET", &url, None)?;
    if matches!(reply.status, 404 | 409) {
        return Ok(BTreeSet::new());
    }
    let tree = check("GET", &url, reply)?;
    Ok(tree["tree"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "blob")
        .filter_map(|e| e["path"].as_str().map(str::to_owned))
        .collect())
}

pub fn default_branch(repo: &Value) -> String {
    repo["default_branch"].as_str().unwrap_or("main").to_owned()
}
