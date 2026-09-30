use std::{
    io::{BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use heck::{ToKebabCase, ToUpperCamelCase};
use serde_json::{Value, json};

use crate::{
    assets,
    config::{self, Config, LANGUAGES, Sdk, Source, manifest_version},
    fsx,
    generate::tokens,
    spec,
};

const SPECS: [&str; 6] = [
    "openapi.json",
    "openapi.yaml",
    "openapi.yml",
    "spec/openapi.json",
    "spec/openapi.yaml",
    "swagger.json",
];

pub struct Init {
    pub languages: Vec<String>,
    pub spec: Option<String>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub release: bool,
}

/// Writes perseid.toml, then the skeletons and release files of the SDKs it keeps here.
pub fn run(init: Init, root: &Path) -> Result<Vec<PathBuf>> {
    let config_path = root.join(config::FILE);
    let mut created = Vec::new();
    let (toml, changed) = draft(&init, root)?;
    if changed {
        fsx::write(&config_path, toml.as_bytes())?;
        created.push(config_path.clone());
    }
    let (config, root) = Config::load(&config_path)?;
    let read = |path: &str| Ok(std::fs::read_to_string(root.join(path)).ok());
    for (path, content) in hub_files(&config, Some(&root), init.release, read)? {
        let target = root.join(path);
        fsx::write(&target, &content)?;
        created.push(target);
    }
    Ok(created)
}

/// The spec `init` starts from: `--spec`, a conventional file, or the user's answer.
fn locate_spec(init: &Init, root: &Path) -> Result<String> {
    if let Some(spec) = init.spec.clone().or_else(|| {
        SPECS
            .iter()
            .find(|s| root.join(s).is_file())
            .map(|s| s.to_string())
    }) {
        return Ok(spec);
    }
    if std::io::stdin().is_terminal() {
        print!("? Where is the OpenAPI spec? (a path, an https URL, or github:owner/repo/path) ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if !line.trim().is_empty() {
            return Ok(line.trim().to_owned());
        }
    }
    bail!(
        "no OpenAPI spec found, pass --spec <path, url, or github:owner/repo/path of a spec pushed from another repository>"
    )
}

/// The text of perseid.toml once `init` has added its languages, and whether that is new.
pub fn draft(init: &Init, root: &Path) -> Result<(String, bool)> {
    let config_path = root.join(config::FILE);
    let mut toml = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        let spec = locate_spec(init, root)?;
        let source = Source::parse(&spec)?;
        let doc: Value = match &source {
            Source::GitHub { .. } => json!({}),
            _ => serde_json::from_str(&spec::read(&spec, root)?)?,
        };
        let name = init.name.clone().unwrap_or_else(|| match &source {
            Source::GitHub { repo, .. } => {
                name_from_title(&json!({ "info": { "title": repo.rsplit('/').next() } }))
            }
            _ => name_from_title(&doc),
        });
        let base_url = init.base_url.clone().or_else(|| {
            doc["servers"][0]["url"]
                .as_str()
                .filter(|u| u.starts_with("http"))
                .map(|u| u.trim_end_matches('/').to_owned())
        });
        let mut toml = format!("spec = {spec:?}\nname = {:?}\n", name.to_upper_camel_case());
        if let Some(url) = base_url {
            toml += &format!("base_url = {url:?}\n");
        }
        toml += "method_names = \"resource\"\n";
        toml += &metadata(&doc, root);
        toml += &layouts(&source, &name.to_kebab_case(), root);
        toml
    };
    let existing: toml::Table = toml.parse()?;
    let languages = match init.languages.is_empty() {
        true if existing.keys().any(|k| LANGUAGES.contains(&k.as_str())) => vec![],
        true => LANGUAGES.map(str::to_owned).to_vec(),
        false => init.languages.clone(),
    };
    let mut added = false;
    for language in languages
        .iter()
        .filter(|l| !existing.contains_key(l.as_str()))
    {
        toml += &format!("\n[{language}]\n");
        if language == "go" {
            toml += "initialisms = true\npatch_nullable = true\ntyped_unions = true\n";
        }
        if language == "csharp" {
            toml += "patch_nullable = true\ntyped_unions = true\n";
        }
        if ["rust", "typescript", "python"].contains(&language.as_str()) {
            toml += "typed_unions = true\n";
        }
        if language == "java" {
            toml += "edition = 2\n";
        }
        added = true;
    }
    Ok((toml, added || !config_path.exists()))
}

/// The repository layouts perseid.toml can declare, as comments to uncomment.
fn layouts(source: &Source, name: &str, root: &Path) -> String {
    let owner = git_remote(root)
        .and_then(|url| {
            Some(
                url.strip_prefix("https://github.com/")?
                    .split('/')
                    .next()?
                    .to_owned(),
            )
        })
        .unwrap_or_else(|| "acme".into());
    let mut lines = vec![
        (
            format!("repo = \"{owner}/{name}-{{lang}}\""),
            format!("one repository per SDK: {name}-node, {name}-python…"),
        ),
        (
            format!("repo = \"{owner}/{name}-sdks\""),
            "one repository holding every SDK, a folder each".to_owned(),
        ),
    ];
    if let Source::File(_) = source {
        lines.push((
            format!("push_spec = \"{owner}/{name}-sdks\""),
            "or send the spec to a repository that generates the SDKs itself".to_owned(),
        ));
    }
    if !matches!(source, Source::Url(_)) {
        lines.push((
            "generate = \"make openapi.json\"".to_owned(),
            "how the API repository's CI writes the spec, if not committed".to_owned(),
        ));
    }
    let width = lines
        .iter()
        .map(|(s, _)| s.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::from(
        "\n# Where the SDKs live: here, a folder per language, unless one of these is uncommented.\n\
         # See https://github.com/meteroid-oss/perseid/blob/main/docs/ci.md#repository-layouts\n",
    );
    for (setting, comment) in lines {
        let pad = width - setting.chars().count();
        out += &format!("# {setting}{}  # {comment}\n", " ".repeat(pad));
    }
    out
}

/// The files `init` gives the SDKs kept in the repository of perseid.toml (at `root` when
/// local): the skeletons and release files `read` lacks.
pub fn hub_files(
    config: &Config,
    root: Option<&Path>,
    release: bool,
    read: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<(String, Vec<u8>)>> {
    if config.push_spec.is_some() {
        return Ok(vec![]);
    }
    let sdks = config.sdks(&[])?;
    let local: Vec<&Sdk> = sdks.iter().filter(|s| s.repo.is_none()).collect();
    let mut files = Vec::new();
    for sdk in &local {
        let dir = root.map_or_else(|| PathBuf::from("/nonexistent"), |r| r.join(&sdk.path));
        for (path, content) in skeleton(config, sdk, &dir)? {
            let path = match sdk.path.as_str() {
                "." => path,
                folder => format!("{folder}/{path}"),
            };
            if read(&path)?.is_none() {
                files.push((path, content));
            }
        }
    }
    if release {
        files.extend(release_scaffold(config, &local, read)?);
    }
    Ok(files)
}

/// Gives an SDK repository without a package manifest the skeleton `init` gives local SDKs.
pub fn bootstrap(config: &Config, sdk: &Sdk, repo: &Path) -> Result<Vec<PathBuf>> {
    let dir = repo.join(&sdk.path);
    let manifests: &[&str] = match sdk.language {
        "rust" => &["Cargo.toml"],
        "typescript" => &["package.json"],
        "python" => &["pyproject.toml", "setup.py"],
        "go" => &["go.mod"],
        "csharp" => &[],
        _ => &["build.gradle", "build.gradle.kts", "pom.xml"],
    };
    let dotnet = || {
        std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "sln"))
        })
    };
    if manifests.iter().any(|m| dir.join(m).exists()) || (sdk.language == "csharp" && dotnet()) {
        return Ok(vec![]);
    }
    let mut created = Vec::new();
    for (path, content) in skeleton(config, sdk, &dir)? {
        let target = dir.join(path);
        if !target.exists() {
            fsx::write(&target, &content)?;
            created.push(target);
        }
    }
    created.extend(release(repo, &[package(config, sdk, &dir, None)])?);
    Ok(created)
}

/// The skeleton of an SDK checked out at `dir`, relative to it.
fn skeleton(config: &Config, sdk: &Sdk, dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut context = config.context(sdk, dir);
    let path = context["java_package"].as_str().unwrap().replace('.', "/");
    context["java_package_path"] = path.into();
    package_metadata(config, &mut context);
    let mut files = Vec::new();
    for (path, content) in assets::under(&format!("scaffold/{}", sdk.language)) {
        let content = without_unset_metadata(std::str::from_utf8(content)?, &context);
        files.push((
            tokens(path, &context)?,
            tokens(&content, &context)?.into_bytes(),
        ));
    }
    Ok(files)
}

/// A release-please package, `released` at its current version or never released.
struct Package {
    path: String,
    released: Option<String>,
    config: Value,
}

fn package(config: &Config, sdk: &Sdk, dir: &Path, released: Option<String>) -> Package {
    let context = config.context(sdk, dir);
    let java = context["java_package"].as_str().unwrap().replace('.', "/");
    let mut entry = match sdk.language {
        "rust" => json!({ "release-type": "rust" }),
        "typescript" => json!({ "release-type": "node", "extra-files": ["src/request.ts"] }),
        "python" => json!({ "release-type": "python" }),
        "go" => json!({ "release-type": "go", "version-file": "version.go" }),
        "csharp" => {
            let name = context["package_name"].as_str().unwrap();
            json!({ "release-type": "simple", "extra-files": [format!("{name}/{name}.csproj")] })
        }
        _ => json!({
            "release-type": "simple",
            "extra-files": ["gradle.properties", format!("src/main/java/{java}/Version.java")],
        }),
    };
    entry["component"] = sdk.language.into();
    if sdk.path == "." {
        entry["include-component-in-tag"] = false.into();
    }
    Package {
        path: sdk.path.clone(),
        released,
        config: entry,
    }
}

/// Adds missing `packages` to the release-please files of `repo`, and the workflow releasing them.
fn release(repo: &Path, packages: &[Package]) -> Result<Vec<PathBuf>> {
    let read = |path: &str| Ok(std::fs::read_to_string(repo.join(path)).ok());
    let mut created = Vec::new();
    for (path, content) in release_files(read, packages)? {
        let target = repo.join(path);
        fsx::write(&target, &content)?;
        created.push(target);
    }
    Ok(created)
}

/// The release files of a repository holding `sdks`, as `perseid init` gives them to a new one.
pub fn release_scaffold(
    config: &Config,
    sdks: &[&Sdk],
    read: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut packages = Vec::new();
    for sdk in sdks {
        let dir = tempfile::tempdir()?;
        for file in [
            "package.json",
            "Cargo.toml",
            "pyproject.toml",
            "gradle.properties",
            "version.go",
        ] {
            let path = match sdk.path.as_str() {
                "." => file.to_owned(),
                sdk => format!("{sdk}/{file}"),
            };
            if let Some(text) = read(&path)? {
                std::fs::write(dir.path().join(file), text)?;
            }
        }
        let released = manifest_version(dir.path());
        packages.push(package(config, sdk, dir.path(), released));
    }
    release_files(read, &packages)
}

/// The release-please files and workflow `read` lacks or holds without some of `packages`.
fn release_files(
    read: impl Fn(&str) -> Result<Option<String>>,
    packages: &[Package],
) -> Result<Vec<(String, Vec<u8>)>> {
    if packages.is_empty() {
        return Ok(vec![]);
    }
    let json = |path: &str| -> Result<Option<Value>> {
        read(path)?
            .map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
    };
    let config_path = "release-please-config.json";
    let manifest_path = ".release-please-manifest.json";
    let mut config = json(config_path)?.unwrap_or_else(|| {
        json!({
            "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
            "bump-minor-pre-major": true,
            "bump-patch-for-minor-pre-major": true,
            "initial-version": "0.1.0",
            "tag-separator": "/",
            "packages": {},
        })
    });
    let mut manifest = json(manifest_path)?.unwrap_or_else(|| json!({}));
    let (Some(entries), Some(versions)) = (
        config
            .as_object_mut()
            .map(|c| c.entry("packages").or_insert_with(|| json!({}))),
        manifest.as_object_mut(),
    ) else {
        bail!("release-please files must hold JSON objects");
    };
    let Some(entries) = entries.as_object_mut() else {
        bail!("`packages` of release-please-config.json must be an object");
    };
    let mut files = Vec::new();
    let mut added = false;
    for package in packages {
        if entries.contains_key(&package.path) {
            continue;
        }
        entries.insert(package.path.clone(), package.config.clone());
        if let Some(version) = &package.released {
            versions.insert(package.path.clone(), version.as_str().into());
        }
        added = true;
    }
    if added {
        for (path, value) in [(config_path, &config), (manifest_path, &manifest)] {
            let text = serde_json::to_string_pretty(value)? + "\n";
            files.push((path.to_owned(), text.into_bytes()));
        }
    }
    for (path, content) in assets::under("scaffold/release") {
        if read(path)?.is_none() {
            files.push((path.to_owned(), content.to_vec()));
        }
    }
    Ok(files)
}

/// Scaffold tokens of the package metadata, which manifests leave out when unset.
const METADATA: [&str; 10] = [
    "LICENSE",
    "LICENSE_URL",
    "REPOSITORY",
    "HOMEPAGE",
    "PROJECT_URL",
    "AUTHOR",
    "AUTHOR_ID",
    "AUTHOR_NAME",
    "AUTHORS_JSON",
    "AUTHORS_PYPROJECT",
];

/// Metadata values fit for any manifest: one line, without quotes, backslashes or markup.
fn manifest_text(text: &str) -> String {
    let text: String = text
        .chars()
        .map(|c| match c {
            '"' => '\'',
            '\\' | '<' | '>' | '&' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Jane Doe <jane@example.com>` as its name and email.
fn author_parts(author: &str) -> (String, Option<String>) {
    match author.split_once('<') {
        Some((name, email)) => (
            manifest_text(name),
            Some(manifest_text(email.trim_end_matches('>'))).filter(|e| !e.is_empty()),
        ),
        None => (manifest_text(author), None),
    }
}

fn package_metadata(config: &Config, context: &mut Value) {
    let text = |value: &Option<String>| {
        value
            .as_deref()
            .map(manifest_text)
            .filter(|v| !v.is_empty())
            .map_or(Value::Null, Value::from)
    };
    let license = text(&config.license);
    let project = config
        .homepage
        .clone()
        .or_else(|| config.repository.clone());
    context["license_url"] = license
        .as_str()
        .filter(|l| {
            !l.starts_with("LicenseRef-")
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.+".contains(&b))
        })
        .map_or(Value::Null, |l| {
            format!("https://spdx.org/licenses/{l}.html").into()
        });
    context["license"] = license;
    context["repository"] = text(&config.repository);
    context["homepage"] = text(&config.homepage);
    context["project_url"] = text(&project);
    context["description"] =
        manifest_text(context["description"].as_str().unwrap_or_default()).into();
    let authors: Vec<(String, Option<String>)> = config
        .authors
        .iter()
        .map(|a| author_parts(a))
        .filter(|(name, _)| !name.is_empty())
        .collect();
    let full = |(name, email): &(String, Option<String>)| match email {
        Some(email) => format!("{name} <{email}>"),
        None => name.clone(),
    };
    context["author"] = authors.first().map_or(Value::Null, |a| full(a).into());
    context["author_name"] = authors.first().map_or(Value::Null, |a| a.0.clone().into());
    context["author_id"] = authors
        .first()
        .map_or(Value::Null, |a| a.0.to_kebab_case().into());
    context["authors_names"] = match authors.is_empty() {
        true => config.name.clone().into(),
        false => authors
            .iter()
            .map(|a| a.0.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
    };
    context["authors_json"] = match authors.is_empty() {
        true => Value::Null,
        false => serde_json::to_string(&authors.iter().map(full).collect::<Vec<_>>())
            .unwrap_or_default()
            .into(),
    };
    context["authors_pyproject"] = match authors.is_empty() {
        true => Value::Null,
        false => {
            let entries = authors.iter().map(|(name, email)| match email {
                Some(email) => format!("{{ name = \"{name}\", email = \"{email}\" }}"),
                None => format!("{{ name = \"{name}\" }}"),
            });
            format!("[{}]", entries.collect::<Vec<_>>().join(", ")).into()
        }
    };
}

/// Drops the lines of `source` holding a metadata token without a value.
fn without_unset_metadata(source: &str, context: &Value) -> String {
    let unset = |key: &str| {
        context
            .get(key.to_lowercase())
            .is_none_or(|v| v.is_null() || v.as_str() == Some(""))
    };
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let dropped = METADATA
            .iter()
            .any(|key| line.contains(&format!("@@{key}@@")) && unset(key));
        if !dropped {
            out.push_str(line);
        }
    }
    out
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
                && !["api", "rest", "openapi", "service"].contains(&w.to_lowercase().as_str())
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

    #[test]
    fn unset_metadata_lines_are_dropped() {
        let context = serde_json::json!({ "license": null, "repository": "https://x.dev/r" });
        let source = "a\nlicense = \"@@LICENSE@@\"\nrepository = \"@@REPOSITORY@@\"\n";
        assert_eq!(
            without_unset_metadata(source, &context),
            "a\nrepository = \"@@REPOSITORY@@\"\n"
        );
    }
}
