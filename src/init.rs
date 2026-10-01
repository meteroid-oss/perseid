//! `perseid init`: perseid.toml, from the spec found here and the SDKs asked for, and the
//! workflows regenerating and releasing the SDKs. Nothing leaves this checkout.

use std::{
    collections::BTreeSet,
    io::IsTerminal,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Result, bail, ensure};
use heck::{ToKebabCase, ToUpperCamelCase};
use serde_json::{Value, json};

use crate::{
    config::{self, Config, LANGUAGES, Source},
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

const FLAGS: &str = "--sdks <rust,typescript,python,go,java,csharp> (required), --repo <owner/name-{lang}|owner/name>, --spec <path|url>, --name <Name>, --base-url <url>";

/// Writes perseid.toml, asking what the flags and the spec here don't tell.
pub fn run(init: Init, root: &Path) -> Result<()> {
    let path = root.join(config::FILE);
    if path.exists() {
        ensure!(
            init.spec.is_none()
                && init.name.is_none()
                && init.sdks.is_empty()
                && init.repo.is_none(),
            "{} already exists: edit it (`sdks`, `repo`, `spec`…), then run `perseid init` again",
            config::FILE
        );
        let (config, _) = Config::load(&path)?;
        if !write_files(&config, root)? {
            println!("✓ the workflows match {}", config::FILE);
        }
        next_steps(&config, root);
        return Ok(());
    }
    let interactive = std::io::stdin().is_terminal();
    if !interactive && init.sdks.is_empty() {
        bail!("perseid init asks which SDKs to generate: without a terminal, pass {FLAGS}");
    }
    if interactive {
        crate::prompt::intro("perseid init")?;
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
        (Some(spec), true) => match read_spec(spec, root) {
            Ok(doc) => doc,
            Err(error) => {
                println!(
                    "! {spec} can't be read, so nothing is taken from it: {error:#}. Fix it, then `perseid generate` checks it"
                );
                json!({})
            }
        },
        _ => json!({}),
    };
    let here = crate::github::origin_repo(root);
    let derived = doc["info"]["title"]
        .as_str()
        .map(|_| name_from_title(&doc))
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
    let name = match (&init.name, interactive) {
        (Some(name), _) => name.clone(),
        (None, true) => {
            crate::prompt::text("Client name, as the SDKs' class names start", &derived)?
        }
        (None, false) => derived,
    };
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
    let base_url = init.base_url.clone().or_else(|| server_url(&doc));
    if base_url.is_none() && readable {
        println!(
            "! the spec has no absolute server URL: the SDKs default to http://localhost until you set `base_url`"
        );
    }
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
    let package = metadata(&doc);
    if !package.is_empty() {
        toml += &format!("\n[metadata]\n{package}");
    }
    if let Some(package) = sdks
        .iter()
        .any(|s| s == "java")
        .then(|| java_package(&doc, &name))
        .flatten()
    {
        toml += &format!("\n[java]\npackage = {}\n", quote(&package));
    }
    fsx::write(&path, toml.as_bytes())?;
    println!("+ {}", config::FILE);
    let (config, _) = Config::load(&path)?;
    write_files(&config, root)?;
    let where_ = match &repo {
        Some(repo) if repo.contains("{lang}") => sdks
            .iter()
            .map(|s| repo.replace("{lang}", s))
            .collect::<Vec<_>>()
            .join(", "),
        Some(repo) => format!("{repo} ({})", folders(&sdks)),
        None => format!("{} here", folders(&sdks)),
    };
    println!("\n  {name} SDKs: {where_}");
    if readable {
        println!("  from {spec_path}");
    }
    println!("  Packages: {}", packages(&config)?);
    println!(
        "  (rename one with `package = \"…\"` under its [language] in {}, before its first release)",
        config::FILE
    );
    next_steps(&config, root);
    Ok(())
}

/// Writes the workflows `config` calls for, saying which: whether any was written.
fn write_files(config: &Config, root: &Path) -> Result<bool> {
    let written = crate::github::write_files(config, root)?;
    for file in &written {
        match file {
            crate::github::Written::Added(path) => println!("+ {path}"),
            crate::github::Written::Updated(path) => println!("~ {path}"),
            crate::github::Written::Kept(message) => println!("! {message}"),
        }
    }
    Ok(written
        .iter()
        .any(|w| !matches!(w, crate::github::Written::Kept(_))))
}

/// The package each SDK publishes, as registries will know it.
fn packages(config: &Config) -> Result<String> {
    let mut names = Vec::new();
    for sdk in config.sdks(&[])? {
        let context = config.context(&sdk, Path::new("/nonexistent"));
        let text = |key: &str| context[key].as_str().unwrap_or_default().to_owned();
        names.push(match sdk.language {
            "typescript" => format!("npm {}", text("npm_package")),
            "python" => format!("PyPI {}", text("package_name")),
            "rust" => format!("crates.io {}", text("rust_crate")),
            "go" => format!("Go {}", text("go_module")),
            "java" => format!("Maven {}", text("java_package")),
            _ => format!("NuGet {}", text("package_name")),
        });
    }
    Ok(names.join(", "))
}

fn next_steps(config: &Config, root: &Path) {
    let here = crate::github::origin_repo(root);
    let hub = here.as_deref().unwrap_or("<owner/this-repository>");
    let spec = match config.source() {
        Source::File(file) if !root.join(file).exists() => Some(file),
        _ => None,
    };
    if let Some(file) = spec {
        println!(
            "\nNo OpenAPI spec here yet: perseid reads {file}, which `npx perseid connect {hub}`, run in the repository holding the spec, pushes here."
        );
    }
    println!("\nNext steps");
    let mut steps = vec![
        "Review and commit what perseid wrote, then push: the workflows run from the default branch".to_owned(),
        match spec {
            Some(_) => "`perseid generate --spec <path|url>` previews the SDKs meanwhile".to_owned(),
            None => "`perseid generate` previews the SDKs (`--out <dir>` for those living in other repositories)".to_owned(),
        },
    ];
    let sdks = config.sdks(&[]).unwrap_or_default();
    let remote: BTreeSet<&str> = sdks.iter().filter_map(|s| s.remote()).collect();
    if !remote.is_empty() {
        let create: Vec<String> = remote
            .iter()
            .map(|r| format!("`gh repo create {r}`"))
            .collect();
        steps.push(format!(
            "Create the SDK repositories: {}. sdks.yml opens their first pull request",
            create.join(", ")
        ));
    }
    let repos: Vec<&str> = std::iter::once(hub).chain(remote.iter().copied()).collect();
    steps.push(format!(
        "Add the {} secret to {}: a fine-grained token (https://github.com/settings/personal-access-tokens/new) with Contents, Pull requests and Workflows read and write. Or `perseid app` sets up a GitHub App, which doesn't expire",
        crate::github::TOKEN,
        repos.join(", ")
    ));
    if spec.is_some() {
        steps.push(format!(
            "In the repository holding the spec: `npx perseid connect {hub}`. It writes a workflow there and, once you agree, a deploy key letting that repository push its spec here, and nothing else"
        ));
    }
    for (i, step) in steps.iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    if config.release != Some(false) {
        println!("\nTo publish, once per registry:");
        for step in crate::github::publishing(config, hub).unwrap_or_default() {
            println!("  - {step}");
        }
    }
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

fn ask_sdks() -> Result<Vec<String>> {
    let items: Vec<(&str, &str)> = LANGUAGES
        .iter()
        .map(|l| match *l {
            "rust" => ("Rust", "crates.io"),
            "typescript" => ("TypeScript", "npm"),
            "python" => ("Python", "PyPI"),
            "go" => ("Go", "Go modules"),
            "java" => ("Java", "Maven Central"),
            _ => ("C#", "NuGet"),
        })
        .collect();
    let picked = crate::prompt::pick_many("Which SDKs?", &items)?;
    Ok(picked
        .into_iter()
        .map(|i| LANGUAGES[i].to_owned())
        .collect())
}

fn ask_layout(sdks: &[String], name: &str, here: Option<&str>) -> Result<Option<String>> {
    let owner = here.and_then(|r| r.split('/').next()).unwrap_or("acme");
    let pattern = format!("{owner}/{}-{{lang}}", name.to_kebab_case());
    let repos: Vec<String> = sdks.iter().map(|s| pattern.replace("{lang}", s)).collect();
    let shared = format!("{owner}/{}-sdks", name.to_kebab_case());
    let choices = [
        format!("Here, a folder each: {}", folders(sdks)),
        format!("A repository each: {}", repos.join(", ")),
        format!("One other repository, a folder each: {shared}"),
    ];
    match crate::prompt::pick_one("Where do the SDKs live?", &choices, 0)? {
        0 => Ok(None),
        1 => Ok(Some(crate::prompt::text(
            "Repository of each SDK",
            &pattern,
        )?)),
        _ => Ok(Some(crate::prompt::text(
            "Repository of the SDKs",
            &shared,
        )?)),
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
            let picked = crate::prompt::pick_one("Which OpenAPI spec?", &specs, 0)?;
            Ok(Some(specs[picked].clone()))
        }
    }
}

/// The perseid.toml lines of the package metadata the spec tells.
fn metadata(doc: &Value) -> String {
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

fn read_spec(spec: &str, root: &Path) -> Result<Value> {
    let doc: Value = serde_json::from_str(&spec::read(spec, root)?)?;
    ensure!(
        doc.get("swagger").is_none(),
        "Swagger 2.0 isn't supported, convert it with `npx swagger2openapi`"
    );
    Ok(doc)
}

/// The first absolute server URL, its `{variables}` set to their defaults.
fn server_url(doc: &Value) -> Option<String> {
    let server = &doc["servers"][0];
    let mut url = server["url"].as_str()?.to_owned();
    if let Some(variables) = server["variables"].as_object() {
        for (name, variable) in variables {
            if let Some(default) = variable["default"].as_str() {
                url = url.replace(&format!("{{{name}}}"), default);
            }
        }
    }
    (url.starts_with("http") && !url.contains('{')).then(|| url.trim_end_matches('/').to_owned())
}

/// A Java package Maven Central can verify: the reversed domain of the API's homepage, then the
/// client name, as `com.acme.petstore` for `https://www.acme.com`.
fn java_package(doc: &Value, name: &str) -> Option<String> {
    let url = doc["info"]["contact"]["url"]
        .as_str()
        .or(doc["externalDocs"]["url"].as_str())?;
    let host = url.split("://").nth(1)?.split(['/', ':']).next()?;
    let labels: Vec<&str> = host
        .split('.')
        .filter(|l| !l.is_empty())
        .skip_while(|l| matches!(*l, "www" | "api" | "docs" | "developer" | "developers"))
        .collect();
    if labels.len() < 2 || labels.iter().any(|l| l.parse::<u8>().is_ok()) {
        return None;
    }
    let mut parts: Vec<String> = labels.iter().rev().map(|l| l.replace('-', "_")).collect();
    parts.push(
        name.to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), ""),
    );
    Some(parts.join("."))
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
    fn server_variables_take_their_defaults() {
        let doc = serde_json::json!({ "servers": [{
            "url": "https://{region}.acme.com/{version}/",
            "variables": { "region": { "default": "eu" }, "version": { "default": "v2" } }
        }]});
        assert_eq!(server_url(&doc).as_deref(), Some("https://eu.acme.com/v2"));
        let relative = serde_json::json!({ "servers": [{ "url": "/v1" }] });
        assert_eq!(server_url(&relative), None);
    }

    #[test]
    fn java_packages_follow_the_homepage_domain() {
        let doc = |url: &str| serde_json::json!({ "info": { "contact": { "url": url } } });
        let package = |url: &str| java_package(&doc(url), "PetStore");
        assert_eq!(
            package("https://www.acme.com/about").as_deref(),
            Some("com.acme.petstore")
        );
        assert_eq!(
            package("https://api.pets.co.uk").as_deref(),
            Some("uk.co.pets.petstore")
        );
        assert_eq!(package("http://localhost:8080"), None);
        assert_eq!(package("http://127.0.0.1"), None);
    }

    #[test]
    fn metadata_comes_from_the_spec_info() {
        let doc = serde_json::json!({ "info": {
            "title": "Acme", "description": "<p>The Acme API. Manage widgets.</p>",
            "license": { "name": "Apache 2.0" },
            "contact": { "name": "Acme", "email": "dev@acme.com", "url": "https://acme.com" }
        }});
        let toml = metadata(&doc);
        assert_eq!(
            toml,
            "description = \"The Acme API.\"\nlicense = \"Apache-2.0\"\nhomepage = \"https://acme.com\"\nauthors = [\"Acme <dev@acme.com>\"]\n"
        );
    }
}
