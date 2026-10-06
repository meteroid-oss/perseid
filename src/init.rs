//! `perseid init`: perseid.toml, from the spec found here and the SDKs asked for, and the
//! workflows regenerating and releasing the SDKs. Nothing leaves this checkout.

use std::{
    collections::BTreeSet,
    io::IsTerminal,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Result, bail, ensure};
use heck::{ToKebabCase, ToShoutySnakeCase};
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
    pub license: Option<String>,
}

const FLAGS: &str = "--sdks <rust,typescript,python,go,java,csharp> (required), --repo <owner/name-{lang}|owner/name>, --spec <path|url>, --name <Name>, --base-url <url>, --license <SPDX>";

/// The licenses `init` offers, all of which `perseid generate` writes the LICENSE of.
const LICENSES: [&str; 6] = [
    "MIT",
    "Apache-2.0",
    "BSD-3-Clause",
    "ISC",
    "MPL-2.0",
    "Unlicense",
];

/// Writes perseid.toml, asking what the flags and the spec here don't tell.
pub fn run(init: Init, root: &Path) -> Result<()> {
    let path = root.join(config::FILE);
    if path.exists() {
        ensure!(
            init.spec.is_none()
                && init.name.is_none()
                && init.sdks.is_empty()
                && init.repo.is_none()
                && init.license.is_none(),
            "{} already exists: edit it (`sdks`, `repo`, `spec`…), then run `perseid init` again",
            config::FILE
        );
        let (config, _) = Config::load(&path)?;
        match write_files(&config, root)? {
            true => next_steps(&config, root),
            false => println!(
                "✓ the workflows match {}: `perseid status` checks the setup on GitHub",
                config::FILE
            ),
        }
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
        None => match pick_spec(root, interactive)? {
            None if interactive => ask_spec()?,
            found => found,
        },
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
        });
    let name = match (&init.name, interactive) {
        (Some(name), _) => crate::client_name::parse(name).map_err(anyhow::Error::msg)?,
        (None, true) => crate::prompt::text_as(
            "API name, in any case: the SDKs' classes and packages are named after it",
            &derived,
            crate::client_name::parse,
        )?,
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
    let from_spec = spec_license(&doc);
    let license = match (&init.license, interactive) {
        (Some(license), _) => Some(license_expression(license).map_err(anyhow::Error::msg)?),
        (None, true) => ask_license(from_spec.as_deref())?,
        (None, false) => from_spec,
    };
    let base_url = init.base_url.clone().or_else(|| server_url(&doc));
    if base_url.is_none() && readable {
        println!(
            "! the spec has no absolute server URL: until you set `base_url`, clients need one passed in or from {}_BASE_URL",
            name.to_shouty_snake_case()
        );
    }
    if interactive {
        crate::prompt::outro(&format!("Writing {}", config::FILE))?;
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
    toml += &format!(
        "\n[metadata]\n{}",
        metadata(&doc, &name, license.as_deref())
    );
    let draft = Config::parse(&toml, config::FILE)?;
    for sdk in draft.sdks(&[])? {
        let package = match sdk.language {
            "java" => java_package(&doc, &name).unwrap_or_else(|| draft.package(&sdk)),
            _ => draft.package(&sdk),
        };
        toml += &format!("\n[{}]\npackage = {}\n", sdk.language, quote(&package));
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
    let from = match config.source() {
        Source::Url(_) => format!("{spec_path}, fetched daily"),
        Source::File(file) if !root.join(file).exists() => {
            format!("{file}, once `perseid connect` pushes it here")
        }
        Source::File(_) => spec_path,
    };
    println!("\n{name} SDKs: {where_}");
    println!("  Spec: {from}");
    println!("  Packages: {}", packages(&config)?);
    println!(
        "  Rename a package in its [language] table of {}, before its first release",
        config::FILE
    );
    match &license {
        None => println!(
            "! no license: the SDKs get no LICENSE, and crates.io and Maven Central refuse packages without one. Set `license` under [metadata]"
        ),
        Some(license) if !crate::scaffold::ships_license(license) => println!(
            "! perseid has no text of {license} to write as the SDKs' LICENSE: add one to each SDK"
        ),
        Some(_) => {}
    }
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
    let awaited = match config.source() {
        Source::File(file) if !root.join(file).exists() => Some(file),
        _ => None,
    };
    let sdks = config.sdks(&[]).unwrap_or_default();
    let remote: BTreeSet<&str> = sdks.iter().filter_map(|s| s.remote()).collect();
    let repos: Vec<&str> = std::iter::once(hub).chain(remote.iter().copied()).collect();
    let mut steps = vec![match awaited {
        Some(_) => {
            "Preview the SDKs from any spec: `perseid generate --spec <path|url> --out /tmp/sdks`"
                .to_owned()
        }
        None => "Preview the SDKs: `perseid generate --out /tmp/sdks`".to_owned(),
    }];
    if !remote.is_empty() {
        let create: Vec<String> = remote
            .iter()
            .map(|r| format!("`gh repo create {r} --private`"))
            .collect();
        let noun = match create.len() {
            1 => "repository",
            _ => "repositories",
        };
        steps.push(format!("Create the SDK {noun}: {}", create.join(", ")));
    }
    steps.push(format!(
        "Run `perseid sync`: it installs the perseid App on {}, so the workflows open pull requests with nothing stored{}.
       `perseid app` sets up a GitHub App of your own instead",
        repos.join(", "),
        match remote.is_empty() {
            true => "",
            false => ", and commits the CI and release workflows of each SDK repository",
        }
    ));
    steps.push(match awaited {
        Some(_) => "Commit and push what perseid wrote to the default branch".to_owned(),
        None => "Commit and push what perseid wrote to the default branch: sdks.yml then opens the SDK pull requests".to_owned(),
    });
    if let Some(file) = awaited {
        steps.push(format!(
            "In the repository holding the spec, run `npx perseid connect {hub}`: it writes a workflow pushing the spec here as {file}, and adds that repository as the `source` of perseid.toml"
        ));
    }
    steps.push("`perseid status` checks the setup on GitHub".to_owned());
    println!("\nNext steps");
    for (i, step) in steps.iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    if config.release != Some(false) {
        println!(
            "\nBefore the first release, on each registry (trusted publisher: workflow sdk-release.yml, environment release):"
        );
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
    let mut choices = vec![
        format!("Here, a folder each: {}", folders(sdks)),
        format!("A repository each: {}", repos.join(", ")),
    ];
    if !here.is_some_and(|h| h.eq_ignore_ascii_case(&shared)) {
        choices.push(format!("One other repository, a folder each: {shared}"));
    }
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

/// The license the SDKs are published under, the spec's offered first: none when declined.
fn ask_license(from_spec: Option<&str>) -> Result<Option<String>> {
    let mut choices: Vec<String> = LICENSES.iter().map(|l| (*l).to_owned()).collect();
    let default = match from_spec {
        Some(license) => match choices.iter().position(|c| c == license) {
            Some(i) => i,
            None => {
                choices.insert(0, license.to_owned());
                0
            }
        },
        None => 0,
    };
    if from_spec.is_some() {
        choices[default] += " (the spec's)";
    }
    let other = choices.len();
    choices.push("Another SPDX identifier".to_owned());
    choices.push("None yet: crates.io and Maven Central need one to publish".to_owned());
    let picked = crate::prompt::pick_one("License of the SDKs", &choices, default)?;
    Ok(match picked {
        i if i < other => Some(choices[i].trim_end_matches(" (the spec's)").to_owned()),
        i if i == other => Some(crate::prompt::text_as(
            "SPDX license expression (https://spdx.org/licenses)",
            "",
            license_expression,
        )?),
        _ => None,
    })
}

/// `text` as an SPDX license expression, such as `MIT` or `MIT OR Apache-2.0`.
fn license_expression(text: &str) -> Result<String, String> {
    let text = text.trim();
    let valid = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-.+:() ".contains(c));
    match valid {
        true => Ok(text.to_owned()),
        false => Err(format!(
            "`{text}` isn't an SPDX license expression, such as MIT or Apache-2.0: https://spdx.org/licenses"
        )),
    }
}

/// Where the spec comes from, when none is found here.
fn ask_spec() -> Result<Option<String>> {
    let choices = [
        "In another repository, which `perseid connect` sets up to push it here".to_owned(),
        "At a URL, fetched daily".to_owned(),
        "In a file here".to_owned(),
    ];
    match crate::prompt::pick_one("No OpenAPI spec found here: where is it?", &choices, 0)? {
        0 => Ok(None),
        1 => Ok(Some(crate::prompt::ask(
            "URL of the spec",
            "https://api.acme.com/openapi.json",
        )?)),
        _ => Ok(Some(crate::prompt::ask(
            "Path of the spec",
            "api/openapi.yaml",
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

/// The [metadata] lines of perseid.toml: what the spec tells, with `license`, and commented
/// examples for the rest.
fn metadata(doc: &Value, name: &str, license: Option<&str>) -> String {
    let info = &doc["info"];
    let text = |v: &Value| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let quote = |v: &str| toml::Value::String(v.to_owned()).to_string();
    let mut out = String::new();
    let mut line = |key: &str, value: Option<String>, example: String| {
        out += &match value {
            Some(value) => format!("{key} = {value}\n"),
            None => format!("# {key} = {example}\n"),
        };
    };
    let description = text(&info["summary"]).or_else(|| {
        let description = text(&info["description"])?;
        let markdown = crate::api::html::to_markdown(&description);
        let first = markdown.split("\n\n").next()?.trim();
        let first = first.split_inclusive(". ").next().unwrap_or(first).trim();
        (first.len() <= 200 && !first.starts_with('#')).then(|| manifest_text(first))
    });
    line(
        "description",
        description.filter(|d| !d.is_empty()).map(|d| quote(&d)),
        quote(&format!("{name} API client")),
    );
    line("license", license.map(quote), quote("MIT"));
    line(
        "homepage",
        text(&info["contact"]["url"])
            .filter(|u| u.starts_with("http"))
            .map(|u| quote(&u)),
        quote("https://example.com"),
    );
    let author = text(&info["contact"]["name"]).map(|name| match text(&info["contact"]["email"]) {
        Some(email) => format!("{name} <{email}>"),
        None => name,
    });
    line(
        "authors",
        author.map(|a| format!("[{}]", quote(&a))),
        format!("[{}]", quote(&format!("{name} <dev@example.com>"))),
    );
    out
}

/// The SPDX license of the spec's `info.license`.
fn spec_license(doc: &Value) -> Option<String> {
    let license = &doc["info"]["license"];
    let text = |v: &Value| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    text(&license["identifier"]).or_else(|| text(&license["name"]).and_then(|n| spdx(&n)))
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
    crate::client_name::from_title(doc["info"]["title"].as_str().unwrap_or("Client"))
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
        let toml = metadata(&doc, "Acme", spec_license(&doc).as_deref());
        assert_eq!(
            toml,
            "description = \"The Acme API.\"\nlicense = \"Apache-2.0\"\nhomepage = \"https://acme.com\"\nauthors = [\"Acme <dev@acme.com>\"]\n"
        );
        assert_eq!(
            metadata(&json!({}), "Acme", None),
            "# description = \"Acme API client\"\n# license = \"MIT\"\n# homepage = \"https://example.com\"\n# authors = [\"Acme <dev@example.com>\"]\n"
        );
    }

    #[test]
    fn licenses_are_spdx_expressions_and_offered_ones_ship_their_text() {
        assert_eq!(
            license_expression(" MIT OR Apache-2.0 ").unwrap(),
            "MIT OR Apache-2.0"
        );
        assert!(license_expression("").is_err());
        assert!(license_expression("\"MIT\"").is_err());
        for license in LICENSES {
            assert!(crate::scaffold::ships_license(license), "{license}");
        }
    }
}
