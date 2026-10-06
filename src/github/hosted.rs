//! The hosted perseid App: installed on the repositories of a setup, its tokens come from the
//! perseid broker in exchange for the workflows' OIDC tokens, so nothing is stored in them.

use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use super::{Ui, api::GitHub, api::web_base};

pub const SLUG: &str = "perseid-sdks";

/// Whether the hosted App is installed on all of `owner`'s repositories, which its broker refuses:
/// each repository could then write to all the others.
pub fn everywhere(api: &GitHub, owner: &str) -> Result<bool> {
    let reply = api.send("GET", "/user/installations?per_page=100", None)?;
    Ok(reply.status == 200
        && reply.body["installations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| {
                i["app_slug"] == SLUG
                    && i["repository_selection"] == "all"
                    && i["account"]["login"]
                        .as_str()
                        .is_some_and(|l| l.eq_ignore_ascii_case(owner))
            }))
}

/// The repositories among `repos` the hosted App isn't installed on, as the signed-in user sees
/// them, or `None` when GitHub doesn't tell this token.
pub fn missing(api: &GitHub, repos: &[String]) -> Result<Option<Vec<String>>> {
    let reply = api.send("GET", "/user/installations?per_page=100", None)?;
    if reply.status != 200 {
        return Ok(None);
    }
    let installations = reply.body["installations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut covered = Vec::new();
    for installation in installations.iter().filter(|i| i["app_slug"] == SLUG) {
        let Some(id) = installation["id"].as_u64() else {
            continue;
        };
        if installation["repository_selection"] == "all" {
            let login = installation["account"]["login"]
                .as_str()
                .unwrap_or_default();
            covered.extend(repos.iter().filter(|r| owner_is(r, login)).cloned());
            continue;
        }
        for page in 1.. {
            let path = format!("/user/installations/{id}/repositories?per_page=100&page={page}");
            let reply = api.send("GET", &path, None)?;
            if reply.status != 200 {
                return Ok(None);
            }
            let listed = reply.body["repositories"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            covered.extend(
                listed
                    .iter()
                    .filter_map(|r| r["full_name"].as_str().map(str::to_owned)),
            );
            if listed.len() < 100 {
                break;
            }
        }
    }
    Ok(Some(
        repos
            .iter()
            .filter(|r| !covered.iter().any(|c| c.eq_ignore_ascii_case(r)))
            .cloned()
            .collect(),
    ))
}

fn owner_is(repo: &str, login: &str) -> bool {
    repo.split('/')
        .next()
        .is_some_and(|o| o.eq_ignore_ascii_case(login))
}

/// The App's installation page on `owner`, with `repos` selected.
pub fn install_url(api: &GitHub, owner: &str, repos: &[String]) -> Result<String> {
    let mut url = format!("{}/apps/{SLUG}/installations/new", web_base());
    let Some(account) = api.find(&format!("/users/{owner}"))? else {
        return Ok(url);
    };
    let mut query = vec![format!("suggested_target_id={}", account["id"])];
    for repo in repos {
        if let Some(info) = api.find(&format!("/repos/{repo}"))? {
            query.push(format!("repository_ids[]={}", info["id"]));
        }
    }
    url.push_str(&format!("/permissions?{}", query.join("&")));
    Ok(url)
}

/// Sends the user to the App's installation page, then checks it covers `repos`.
pub fn install(api: &GitHub, owner: &str, repos: &[String], ui: &Ui) -> Result<()> {
    let url = install_url(api, owner, repos)?;
    let names: Vec<&str> = repos
        .iter()
        .map(|r| r.rsplit('/').next().unwrap_or(r))
        .collect();
    ui.say(&format!(
        "Install the perseid App on {owner}, choosing \"Only select repositories\": {}",
        names.join(", ")
    ));
    ui.info(&url);
    ui.open(&url);
    if ui.yes {
        return Ok(());
    }
    loop {
        ui.pause("Press enter once it is installed")?;
        match missing(api, repos)? {
            None => return Ok(()),
            Some(left) if left.is_empty() => {
                ui.ok(&format!(
                    "The perseid App is installed on {}",
                    names.join(", ")
                ));
                return Ok(());
            }
            Some(left) => {
                ui.warn(&format!("Not installed on {} yet", left.join(", ")));
                if !ui.confirm("Check again?", true)? {
                    bail!(
                        "the perseid App isn't installed on {}: install it from {url}, then run the command again",
                        left.join(", ")
                    );
                }
            }
        }
    }
}

/// The `source` line of perseid.toml letting `repo` push the spec to the repository `target_id`.
pub fn source_line(repo: &str, id: u64, target_id: u64, private: bool) -> String {
    match private {
        true => format!("source = {{ id = {id}, target_id = {target_id} }}"),
        false => format!("source = {{ repo = {repo:?}, id = {id}, target_id = {target_id} }}"),
    }
}

/// `text` with `line` as its top-level `source`: in place of the one it has, else after `spec`,
/// else before the first table.
pub fn with_source(text: &str, line: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let top = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let key = |l: &str, name: &str| {
        l.trim_start()
            .strip_prefix(name)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    if let Some(at) = lines[..top].iter().position(|l| key(l, "source")) {
        lines[at] = line.to_owned();
    } else if let Some(at) = lines[..top].iter().position(|l| key(l, "spec")) {
        lines.insert(at + 1, line.to_owned());
    } else {
        let at = lines[..top]
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |i| i + 1);
        lines.insert(at, line.to_owned());
    }
    lines.join("\n") + "\n"
}

/// A file to commit to `repo` with the user's credentials: on `base` directly when `direct` and
/// allowed, else through a pull request from `branch`.
pub struct Commit {
    pub repo: String,
    pub base: String,
    pub path: String,
    pub content: Vec<u8>,
    pub message: String,
    pub branch: String,
    pub body: String,
    pub direct: bool,
}

pub fn commit(api: &GitHub, commit: &Commit, ui: &Ui) -> Result<()> {
    let Commit {
        repo, base, path, ..
    } = commit;
    if commit.direct {
        let reply = put(api, commit, base)?;
        if (200..300).contains(&reply.status) {
            ui.ok(&format!("{repo}: committed {path} to `{base}`"));
            return Ok(());
        }
        if !matches!(reply.status, 403 | 409 | 422) {
            super::api::check("PUT", &format!("/repos/{repo}/contents/{path}"), reply)?;
        }
    }
    let head = api.get(&format!("/repos/{repo}/git/ref/heads/{base}"))?;
    let sha = head["object"]["sha"].as_str().unwrap_or_default();
    let branch = &commit.branch;
    let created = api.send(
        "POST",
        &format!("/repos/{repo}/git/refs"),
        Some(&json!({ "ref": format!("refs/heads/{branch}"), "sha": sha })),
    )?;
    if created.status == 422 {
        api.patch(
            &format!("/repos/{repo}/git/refs/heads/{branch}"),
            json!({ "sha": sha, "force": true }),
        )?;
    } else {
        super::api::check("POST", &format!("/repos/{repo}/git/refs"), created)?;
    }
    let reply = put(api, commit, branch)?;
    super::api::check("PUT", &format!("/repos/{repo}/contents/{path}"), reply)?;
    let owner = repo.split('/').next().unwrap_or_default();
    let open = api.get(&format!(
        "/repos/{repo}/pulls?state=open&head={owner}:{branch}&base={base}"
    ))?;
    let url = match open.as_array().and_then(|p| p.first()) {
        Some(pull) => pull["html_url"].clone(),
        None => api.post(
            &format!("/repos/{repo}/pulls"),
            json!({ "title": commit.message, "head": branch, "base": base, "body": commit.body }),
        )?["html_url"]
            .clone(),
    };
    ui.ok(&format!(
        "{repo}: {path} is in a pull request, to merge: {}",
        url.as_str().unwrap_or_default()
    ));
    Ok(())
}

fn put(api: &GitHub, commit: &Commit, branch: &str) -> Result<super::api::Reply> {
    let path = format!("/repos/{}/contents/{}", commit.repo, commit.path);
    let current = api.send("GET", &format!("{path}?ref={branch}"), None)?;
    let mut body = json!({
        "message": commit.message,
        "content": BASE64.encode(&commit.content),
        "branch": branch,
    });
    if let Some(sha) = current.body.get("sha").and_then(Value::as_str) {
        body["sha"] = sha.into();
    }
    api.send("PUT", &path, Some(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_goes_after_the_spec_or_replaces_the_one_there() {
        let line = source_line("acme/api", 1, 2, false);
        assert_eq!(
            line,
            r#"source = { repo = "acme/api", id = 1, target_id = 2 }"#
        );
        assert_eq!(
            with_source(
                "name = \"Acme\"\nspec = \"openapi.json\"\n\n[python]\npath = \"py\"\n",
                &line
            ),
            format!(
                "name = \"Acme\"\nspec = \"openapi.json\"\n{line}\n\n[python]\npath = \"py\"\n"
            )
        );
        assert_eq!(
            with_source(
                "name = \"Acme\"\nsource = { id = 9, target_id = 2 }\n",
                &line
            ),
            format!("name = \"Acme\"\n{line}\n")
        );
        assert_eq!(
            with_source(
                "#:schema x\nname = \"Acme\"\n\n[python]\nsource = 1\n",
                &line
            ),
            format!("#:schema x\nname = \"Acme\"\n{line}\n\n[python]\nsource = 1\n")
        );
        assert_eq!(
            source_line("acme/api", 1, 2, true),
            "source = { id = 1, target_id = 2 }"
        );
    }

    #[test]
    fn the_added_source_parses_back() {
        let text = with_source(
            "name = \"Acme\"\nspec = \"openapi.json\"\nsdks = [\"go\"]\n",
            &source_line("acme/api", 11, 22, false),
        );
        let config = crate::config::Config::parse(&text, "perseid.toml").unwrap();
        let source = config.pushed_from.unwrap();
        assert_eq!(
            (source.repo.as_deref(), source.id, source.target_id),
            (Some("acme/api"), 11, 22)
        );
    }
}
