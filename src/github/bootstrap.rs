use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use super::{
    Ui,
    api::{GitHub, check},
};
use crate::config::{Config, Sdk};

pub struct File {
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

pub enum Outcome {
    /// The branch already held these files.
    Unchanged,
    Committed,
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

fn head(api: &GitHub, repo: &str, branch: &str) -> Result<Option<String>> {
    let url = format!("/repos/{repo}/git/ref/heads/{branch}");
    let reply = api.send("GET", &url, None)?;
    if matches!(reply.status, 404 | 409) {
        return Ok(None);
    }
    Ok(check("GET", &url, reply)?["object"]["sha"]
        .as_str()
        .map(str::to_owned))
}

fn tree_of(api: &GitHub, repo: &str, commit: &str) -> Result<String> {
    let commit = api.get(&format!("/repos/{repo}/git/commits/{commit}"))?;
    Ok(commit["tree"]["sha"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

/// Commits `files` on top of the `base` branch to `branch`, which is replaced when it differs.
pub fn commit(
    api: &GitHub,
    repo: &str,
    base: &str,
    branch: &str,
    files: &[File],
    message: &str,
) -> Result<Outcome> {
    if files.is_empty() {
        return Ok(Outcome::Unchanged);
    }
    let Some(parent) = head(api, repo, base)? else {
        ensure!(base == branch, "{repo} has no `{base}` branch yet");
        let (first, rest) = files.split_first().expect("files to commit");
        // Git objects can't be created in an empty repository, the contents API can.
        api.put(
            &format!("/repos/{repo}/contents/{}", first.path),
            json!({ "message": message, "content": BASE64.encode(&first.content), "branch": base }),
        )?;
        commit(api, repo, base, branch, rest, message)?;
        return Ok(Outcome::Committed);
    };
    let base_tree = tree_of(api, repo, &parent)?;
    let mut entries = Vec::new();
    for file in files {
        let blob = api.post(
            &format!("/repos/{repo}/git/blobs"),
            json!({ "content": BASE64.encode(&file.content), "encoding": "base64" }),
        )?;
        entries.push(json!({
            "path": file.path,
            "mode": if file.executable { "100755" } else { "100644" },
            "type": "blob",
            "sha": blob["sha"],
        }));
    }
    let tree = api.post(
        &format!("/repos/{repo}/git/trees"),
        json!({ "base_tree": base_tree, "tree": entries }),
    )?;
    let tree = tree["sha"].as_str().unwrap_or_default();
    if tree == base_tree {
        return Ok(Outcome::Unchanged);
    }
    let existing = match branch == base {
        true => None,
        false => head(api, repo, branch)?,
    };
    if let Some(existing) = &existing
        && tree_of(api, repo, existing)? == tree
    {
        return Ok(Outcome::Committed);
    }
    let commit = api.post(
        &format!("/repos/{repo}/git/commits"),
        json!({ "message": message, "tree": tree, "parents": [parent] }),
    )?;
    let sha = commit["sha"].clone();
    match existing {
        None if branch != base => api.post(
            &format!("/repos/{repo}/git/refs"),
            json!({ "ref": format!("refs/heads/{branch}"), "sha": sha }),
        )?,
        _ => api.patch(
            &format!("/repos/{repo}/git/refs/heads/{branch}"),
            json!({ "sha": sha, "force": branch != base }),
        )?,
    };
    Ok(Outcome::Committed)
}

/// Commits the release-please files and `sdk-release.yml` an SDK repository lacks, with the
/// user's token: the App that later pushes SDK updates can't write workflow files.
pub fn release(api: &GitHub, config: &Config, repo: &str, sdks: &[&Sdk], ui: &Ui) -> Result<()> {
    let info = api.get(&format!("/repos/{repo}"))?;
    let branch = default_branch(&info);
    let files = crate::init::release_scaffold(config, sdks, |path| read(api, repo, &branch, path))?;
    let paths: Vec<_> = files.iter().map(|(p, _)| p.clone()).collect();
    let files: Vec<File> = files
        .into_iter()
        .map(|(path, content)| File {
            path,
            content,
            executable: false,
        })
        .collect();
    match commit(
        api,
        repo,
        &branch,
        &branch,
        &files,
        "ci: release the SDK with release-please",
    )? {
        Outcome::Unchanged => ui.ok(&format!("{repo} has its release workflow")),
        Outcome::Committed => ui.ok(&format!("{repo}: committed {}", paths.join(", "))),
    }
    Ok(())
}

pub fn default_branch(repo: &Value) -> String {
    repo["default_branch"].as_str().unwrap_or("main").to_owned()
}

/// The URL of the open pull request from `branch`, and whether it was just opened.
pub fn pull_request(
    api: &GitHub,
    repo: &str,
    base: &str,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<(String, bool)> {
    let owner = repo.split('/').next().unwrap_or_default();
    let open = api.get(&format!(
        "/repos/{repo}/pulls?head={owner}:{branch}&state=open"
    ))?;
    if let Some(url) = open[0]["html_url"].as_str() {
        return Ok((url.to_owned(), false));
    }
    let created = api.post(
        &format!("/repos/{repo}/pulls"),
        json!({ "title": title, "head": branch, "base": base, "body": body }),
    )?;
    match created["html_url"].as_str() {
        Some(url) => Ok((url.to_owned(), true)),
        None => bail!("GitHub opened no pull request on {repo}"),
    }
}
