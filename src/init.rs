//! `perseid init`: perseid.toml, from the spec found here and the SDKs asked for.

use std::{
    io::{BufRead, IsTerminal, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use heck::{ToKebabCase, ToUpperCamelCase};
use serde_json::{Value, json};

use crate::{
    config::{self, LANGUAGES, Source},
    fsx,
    scaffold::manifest_text,
    spec,
};

pub struct Init {
    pub spec: Option<String>,
    pub name: Option<String>,
    pub sdks: Vec<String>,
    pub repo: Option<String>,
    pub base_url: Option<String>,
}

const FLAGS: &str = "--sdks <rust,typescript,python,go,java,csharp> (required), --repo <owner/name-{lang}>, --spec <path|url>, --name <Name>";

/// Writes perseid.toml, asking what the flags and the spec here don't tell.
pub fn run(init: Init, root: &Path) -> Result<()> {
    let path = root.join(config::FILE);
    ensure!(
        !path.exists(),
        "{} already exists: edit it (`sdks`, `repo`, `spec`…), then run `perseid generate` or `perseid setup`",
        config::FILE
    );
    let interactive = std::io::stdin().is_terminal();
    if !interactive && init.sdks.is_empty() {
        bail!("perseid init asks which SDKs to generate: without a terminal, pass {FLAGS}");
    }
    let spec = match &init.spec {
        Some(spec) => Some(spec.clone()),
        None => pick_spec(root, interactive)?,
    };
    let readable = spec.as_deref().is_some_and(|s| match Source::parse(s) {
        Source::Url(_) => true,
        Source::File(file) => root.join(file).is_file(),
    });
    let doc: Value = match (&spec, readable) {
        (Some(spec), true) => serde_json::from_str(&spec::read(spec, root)?)?,
        _ => json!({}),
    };
    let here = crate::github::origin_repo(root);
    let name = init
        .name
        .clone()
        .or_else(|| doc["info"]["title"].as_str().map(|_| name_from_title(&doc)))
        .unwrap_or_else(|| {
            let fallback = here
                .as_deref()
                .and_then(|r| r.rsplit('/').next())
                .map(str::to_owned)
                .or_else(|| Some(root.file_name()?.to_string_lossy().into_owned()))
                .unwrap_or_default();
            name_from_title(&json!({ "info": { "title": fallback } }))
        })
        .to_upper_camel_case();
    let sdks = match init.sdks.is_empty() {
        true => ask_sdks()?,
        false => checked(&init.sdks)?,
    };
    let repo = match (&init.repo, interactive) {
        (Some(repo), _) => Some(repo.clone()),
        (None, true) => ask_layout(&sdks, &name, here.as_deref())?,
        (None, false) => None,
    };
    if let Some(repo) = &repo {
        ensure!(
            repo.split('/').count() == 2 && !repo.starts_with('/') && !repo.ends_with('/'),
            "`--repo {repo}` must read owner/name, or owner/name-{{lang}} for a repository per SDK"
        );
    }
    let base_url = init.base_url.clone().or_else(|| {
        doc["servers"][0]["url"]
            .as_str()
            .filter(|u| u.starts_with("http"))
            .map(|u| u.trim_end_matches('/').to_owned())
    });
    let quote = |v: &str| toml::Value::String(v.to_owned()).to_string();
    let spec_path = spec.clone().unwrap_or_else(|| "openapi.json".into());
    let mut toml = format!(
        "#:schema {}\nspec = {}\nname = {}\nsdks = [{}]\n",
        config::SCHEMA_URL,
        quote(&spec_path),
        quote(&name),
        sdks.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ")
    );
    if let Some(repo) = &repo {
        toml += &format!("repo = {}\n", quote(repo));
    }
    if let Some(url) = base_url {
        toml += &format!("base_url = {}\n", quote(&url));
    }
    let package = metadata(&doc, root);
    if !package.is_empty() {
        toml += &format!("\n[package]\n{package}");
    }
    fsx::write(&path, toml.as_bytes())?;
    println!("+ {}", config::FILE);
    let where_ = match &repo {
        Some(repo) if repo.contains("{lang}") => sdks
            .iter()
            .map(|s| repo.replace("{lang}", s))
            .collect::<Vec<_>>()
            .join(", "),
        Some(repo) => format!("{repo} ({})", folders(&sdks)),
        None => format!("{} here", folders(&sdks)),
    };
    println!("  {name} SDKs: {where_}");
    match readable {
        true => {
            println!("  from {spec_path}");
            println!(
                "\nNext: `perseid generate` to preview the SDKs here, `perseid setup` to automate them on GitHub"
            );
        }
        false => {
            let hub = here.unwrap_or_else(|| "<owner/this-repository>".into());
            println!(
                "\nNo OpenAPI spec here yet: perseid reads {spec_path}, which `npx perseid connect {hub}`, run in the repository holding the spec, pushes here."
            );
            println!(
                "Next: `perseid generate --spec <path|url>` to preview the SDKs, `perseid setup` to automate them on GitHub"
            );
        }
    }
    Ok(())
}

fn folders(sdks: &[String]) -> String {
    sdks.iter()
        .map(|s| format!("{s}/"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn checked(sdks: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for sdk in sdks.iter().map(|s| s.trim().to_lowercase()) {
        ensure!(
            LANGUAGES.contains(&sdk.as_str()),
            "unknown SDK `{sdk}`: pick among {}",
            LANGUAGES.join(", ")
        );
        if !out.contains(&sdk) {
            out.push(sdk);
        }
    }
    out.sort_by_key(|s| LANGUAGES.iter().position(|l| l == s));
    Ok(out)
}

fn ask(question: &str) -> Result<String> {
    print!("? {question} ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn ask_sdks() -> Result<Vec<String>> {
    let choices: Vec<String> = LANGUAGES
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{} {l}", i + 1))
        .collect();
    println!("  {}", choices.join("  "));
    loop {
        let answer = ask("Which SDKs? (names or numbers, comma-separated)")?;
        let picked: Vec<String> = answer
            .split([',', ' '])
            .filter(|a| !a.is_empty())
            .map(|a| match a.parse::<usize>() {
                Ok(n) if (1..=LANGUAGES.len()).contains(&n) => LANGUAGES[n - 1].to_owned(),
                _ => a.to_owned(),
            })
            .collect();
        match checked(&picked) {
            Ok(sdks) if !sdks.is_empty() => return Ok(sdks),
            Ok(_) => println!("  pick at least one"),
            Err(error) => println!("  {error}"),
        }
    }
}

fn ask_layout(sdks: &[String], name: &str, here: Option<&str>) -> Result<Option<String>> {
    let owner = here.and_then(|r| r.split('/').next()).unwrap_or("acme");
    let pattern = format!("{owner}/{}-{{lang}}", name.to_kebab_case());
    let repos: Vec<String> = sdks.iter().map(|s| pattern.replace("{lang}", s)).collect();
    println!("  1 here, a folder each: {}", folders(sdks));
    println!("  2 a repository each: {}", repos.join(", "));
    loop {
        match ask("Where do the SDKs live? [1]")?.as_str() {
            "" | "1" => return Ok(None),
            "2" => {
                let answer = ask(&format!("Repository of each SDK? [{pattern}]"))?;
                return Ok(Some(match answer.is_empty() {
                    true => pattern,
                    false => answer,
                }));
            }
            _ => continue,
        }
    }
}

/// The OpenAPI documents of the repository holding `root`, relative to `root`, shallowest first.
pub fn find_specs(root: &Path) -> Vec<String> {
    let listed = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            o.stdout
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect::<Vec<_>>()
        });
    let candidates = listed.unwrap_or_else(|| {
        let mut found = Vec::new();
        walk(root, "", 4, &mut found);
        found
    });
    let mut specs: Vec<String> = candidates
        .into_iter()
        .filter(|p| named_like_a_spec(p) && is_spec(&root.join(p)))
        .collect();
    specs.sort_by_key(|p| (p.matches('/').count(), p.clone()));
    specs.dedup();
    specs
}

fn walk(dir: &Path, prefix: &str, depth: usize, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                let skipped =
                    name.starts_with('.') || ["node_modules", "target"].contains(&name.as_str());
                if depth > 1 && !skipped {
                    walk(&entry.path(), &format!("{path}/"), depth - 1, found);
                }
            }
            Ok(_) => found.push(path),
            Err(_) => {}
        }
    }
}

fn named_like_a_spec(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    (name.contains("openapi") || name.contains("swagger"))
        && [".json", ".yaml", ".yml"].iter().any(|e| name.ends_with(e))
}

/// Whether `path` holds a document with a top-level `openapi` or `swagger` key.
fn is_spec(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    match text.trim_start().starts_with('{') {
        true => serde_json::from_str::<Value>(&text)
            .is_ok_and(|v| v.get("openapi").is_some() || v.get("swagger").is_some()),
        false => text.lines().any(|line| {
            let key = line.trim_start_matches(['"', '\'']);
            ["openapi", "swagger"].iter().any(|k| {
                key.strip_prefix(k)
                    .is_some_and(|rest| rest.trim_start_matches(['"', '\'']).starts_with(':'))
            })
        }),
    }
}

/// The spec found here: the only one, or the one picked among several.
pub fn pick_spec(root: &Path, interactive: bool) -> Result<Option<String>> {
    let specs = find_specs(root);
    match specs.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        [first, ..] if !interactive => {
            println!(
                "  Using {first}, of {}: pass --spec for another",
                specs.join(", ")
            );
            Ok(Some(first.clone()))
        }
        _ => {
            for (i, spec) in specs.iter().enumerate() {
                println!("  {} {spec}", i + 1);
            }
            loop {
                let answer = ask("Which OpenAPI spec? [1]")?;
                let n = match answer.as_str() {
                    "" => 1,
                    n => n.parse::<usize>().context("a number")?,
                };
                if let Some(spec) = specs.get(n.wrapping_sub(1)) {
                    return Ok(Some(spec.clone()));
                }
            }
        }
    }
}

/// The perseid.toml lines of the package metadata the spec and the git remote tell.
fn metadata(doc: &Value, root: &Path) -> String {
    let info = &doc["info"];
    let text = |v: &Value| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let mut out = String::new();
    let mut line = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            out += &format!("{key} = {}\n", toml::Value::String(value));
        }
    };
    let description = text(&info["summary"]).or_else(|| {
        let description = text(&info["description"])?;
        let markdown = crate::api::html::to_markdown(&description);
        let first = markdown.split("\n\n").next()?.trim();
        let first = first.split_inclusive(". ").next().unwrap_or(first).trim();
        (first.len() <= 200 && !first.starts_with('#')).then(|| manifest_text(first))
    });
    line("description", description.filter(|d| !d.is_empty()));
    let license = text(&info["license"]["identifier"])
        .or_else(|| text(&info["license"]["name"]).and_then(|n| spdx(&n)));
    line("license", license);
    line("repository", git_remote(root));
    line(
        "homepage",
        text(&info["contact"]["url"]).filter(|u| u.starts_with("http")),
    );
    let author = text(&info["contact"]["name"]).map(|name| match text(&info["contact"]["email"]) {
        Some(email) => format!("{name} <{email}>"),
        None => name,
    });
    if let Some(author) = author {
        out += &format!("authors = [{}]\n", toml::Value::String(author));
    }
    out
}

/// The SPDX identifier of a license name such as `Apache 2.0`.
fn spdx(name: &str) -> Option<String> {
    let known = [
        ("apache 2.0", "Apache-2.0"),
        ("apache-2.0", "Apache-2.0"),
        ("apache license 2.0", "Apache-2.0"),
        ("apache license, version 2.0", "Apache-2.0"),
        ("mit", "MIT"),
        ("mit license", "MIT"),
        ("bsd-3-clause", "BSD-3-Clause"),
        ("bsd 3-clause", "BSD-3-Clause"),
        ("isc", "ISC"),
        ("mpl-2.0", "MPL-2.0"),
        ("gpl-3.0", "GPL-3.0-only"),
        ("agpl-3.0", "AGPL-3.0-only"),
        ("unlicense", "Unlicense"),
    ];
    let lower = name.trim().to_lowercase();
    known
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, id)| (*id).to_owned())
}

/// The web URL of the `origin` remote of the repository `root` is in, when it is public.
pub(crate) fn git_remote(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    web_url(String::from_utf8(output.stdout).ok()?.trim())
}

/// `git@github.com:o/r.git` and `https://user@github.com/o/r` as `https://github.com/o/r`.
fn web_url(remote: &str) -> Option<String> {
    let (host, path) = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => remote.strip_prefix("git@")?.split_once(':')?,
    };
    let host = host.rsplit('@').next()?.split(':').next()?;
    let public = host.contains('.')
        && host != "localhost"
        && !host.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    (public && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

fn name_from_title(doc: &Value) -> String {
    let title = doc["info"]["title"].as_str().unwrap_or("Client");
    let words: Vec<_> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            !w.is_empty()
                && ![
                    "api", "rest", "openapi", "service", "sdk", "sdks", "clients",
                ]
                .contains(&w.to_lowercase().as_str())
        })
        .collect();
    match words.join(" ").to_upper_camel_case() {
        name if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) => {
            "Client".into()
        }
        name => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_remotes_become_web_urls() {
        let url = |remote| web_url(remote);
        assert_eq!(
            url("git@github.com:acme/sdk.git").as_deref(),
            Some("https://github.com/acme/sdk")
        );
        assert_eq!(
            url("https://token@github.com/acme/sdk").as_deref(),
            Some("https://github.com/acme/sdk")
        );
        assert_eq!(url("http://proxy@127.0.0.1:8080/git/acme/sdk"), None);
        assert_eq!(url("/tmp/origin.git"), None);
    }

    #[test]
    fn metadata_comes_from_the_spec_info() {
        let doc = serde_json::json!({ "info": {
            "title": "Acme", "description": "<p>The Acme API. Manage widgets.</p>",
            "license": { "name": "Apache 2.0" },
            "contact": { "name": "Acme", "email": "dev@acme.com", "url": "https://acme.com" }
        }});
        let toml = metadata(&doc, Path::new("/"));
        assert_eq!(
            toml,
            "description = \"The Acme API.\"\nlicense = \"Apache-2.0\"\nhomepage = \"https://acme.com\"\nauthors = [\"Acme <dev@acme.com>\"]\n"
        );
    }
}
