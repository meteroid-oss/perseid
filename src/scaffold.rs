//! The files an SDK starts from: its package skeleton, and the release-please setup releasing it.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use heck::ToKebabCase;
use serde_json::{Value, json};

use crate::{
    assets,
    config::{Config, Sdk, manifest_version},
    fsx,
    generate::tokens,
};

/// Gives the SDK at `dir` its skeleton when it has no package manifest: manifest, README, error
/// types.
pub fn bootstrap(config: &Config, sdk: &Sdk, dir: &Path) -> Result<Vec<PathBuf>> {
    let manifests: &[&str] = match sdk.language {
        "rust" => &["Cargo.toml"],
        "typescript" => &["package.json"],
        "python" => &["pyproject.toml", "setup.py"],
        "go" => &["go.mod"],
        "csharp" => &[],
        _ => &["build.gradle", "build.gradle.kts", "pom.xml"],
    };
    let dotnet = || {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "sln"))
        })
    };
    if manifests.iter().any(|m| dir.join(m).exists()) || (sdk.language == "csharp" && dotnet()) {
        return Ok(vec![]);
    }
    let mut created = Vec::new();
    for (path, content) in skeleton(config, sdk, dir)? {
        let target = dir.join(path);
        if !target.exists() {
            fsx::write(&target, &content)?;
            created.push(target);
        }
    }
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

fn package(
    config: &Config,
    sdk: &Sdk,
    path: String,
    dir: &Path,
    released: Option<String>,
) -> Package {
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
    // Go resolves a module in a subdirectory from tags prefixed with that directory.
    entry["component"] = match sdk.language {
        "go" if path != "." => path.as_str().into(),
        language => language.into(),
    };
    if path == "." {
        entry["include-component-in-tag"] = false.into();
    }
    Package {
        path,
        released,
        config: entry,
    }
}

/// `path` under the folder `dir`, relative to the repository root.
fn package_path(dir: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    match (dir, path) {
        ("" | ".", path) => path.to_owned(),
        (dir, "." | "") => dir.to_owned(),
        (dir, path) => format!("{dir}/{path}"),
    }
}

pub const RELEASE_WORKFLOW: &str = ".github/workflows/sdk-release.yml";

/// The release workflow of a repository whose default branch is `branch`.
pub fn release_workflow(branch: &str) -> Vec<u8> {
    let (_, template) = assets::under("scaffold/release")
        .find(|(path, _)| *path == RELEASE_WORKFLOW)
        .expect("the embedded release workflow");
    String::from_utf8_lossy(template)
        .replace("\"@@BRANCH@@\"", &Value::from(branch).to_string())
        .replace(
            "meteroid-oss/perseid/publish@v0",
            &crate::github::uses("publish"),
        )
        .into_bytes()
}

/// The release workflow, and the release-please files `read` lacks or holds without some of
/// `sdks`, for a repository whose root `read` reads and where perseid.toml lives in `dir`.
pub fn release_scaffold(
    config: &Config,
    sdks: &[&Sdk],
    dir: &str,
    branch: &str,
    read: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut packages = Vec::new();
    for sdk in sdks {
        let path = package_path(dir, &sdk.path);
        let manifests = tempfile::tempdir()?;
        for file in [
            "package.json",
            "Cargo.toml",
            "pyproject.toml",
            "gradle.properties",
            "version.go",
        ] {
            if let Some(text) = read(&package_path(&path, file))? {
                std::fs::write(manifests.path().join(file), text)?;
            }
        }
        let released = manifest_version(manifests.path());
        packages.push(package(config, sdk, path, manifests.path(), released));
    }
    let mut files = release_files(read, &packages)?;
    if !packages.is_empty() {
        files.push((RELEASE_WORKFLOW.to_owned(), release_workflow(branch)));
    }
    Ok(files)
}

/// The release-please files `read` lacks or holds without some of `packages`.
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
pub(crate) fn manifest_text(text: &str) -> String {
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
    let license = text(&config.package.license);
    let repository = context["repository"].as_str().map(str::to_owned);
    let project = config.package.homepage.clone().or(repository.clone());
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
    context["repository"] = text(&repository);
    context["homepage"] = text(&config.package.homepage);
    context["project_url"] = text(&project);
    context["description"] =
        manifest_text(context["description"].as_str().unwrap_or_default()).into();
    let authors: Vec<(String, Option<String>)> = config
        .package
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_please_releases_each_sdk_from_its_folder() {
        let toml =
            "name = \"Petstore\"\nsdks = [\"rust\", \"python\", \"go\", \"java\", \"csharp\"]\n";
        let config: Config = toml::from_str(toml).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        let held: Vec<&Sdk> = sdks.iter().collect();
        let pyproject = "[project]\nname = \"petstore\"\nversion = \"1.4.0\"\n";
        let read = |path: &str| Ok((path == "python/pyproject.toml").then(|| pyproject.to_owned()));
        let files: std::collections::BTreeMap<String, Vec<u8>> =
            release_scaffold(&config, &held, "", "main", read)
                .unwrap()
                .into_iter()
                .collect();
        let json = |path: &str| -> Value { serde_json::from_slice(&files[path]).unwrap() };
        let release = json("release-please-config.json");
        assert_eq!(release["tag-separator"], "/");
        assert_eq!(release["packages"]["rust"]["release-type"], "rust");
        assert_eq!(release["packages"]["go"]["version-file"], "version.go");
        assert_eq!(release["packages"]["python"]["release-type"], "python");
        assert_eq!(
            release["packages"]["java"]["extra-files"][1],
            "src/main/java/com/petstore/Version.java"
        );
        assert_eq!(
            release["packages"]["csharp"]["extra-files"][0],
            "Petstore/Petstore.csproj"
        );
        assert_eq!(
            json(".release-please-manifest.json"),
            json!({ "python": "1.4.0" }),
            "never released SDKs start at initial-version"
        );
        let workflow = String::from_utf8_lossy(&files[RELEASE_WORKFLOW]);
        assert!(workflow.contains("branches: [\"main\"]\n"), "{workflow}");

        let root = "name = \"Petstore\"\nsdks = [\"go\"]\nrepo = \"acme/petstore-{lang}\"\n";
        let config: Config = toml::from_str(root).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        let files = release_scaffold(&config, &[&sdks[0]], "", "main", |_| Ok(None)).unwrap();
        let release: Value = serde_json::from_slice(&files[0].1).unwrap();
        assert_eq!(release["packages"]["."]["include-component-in-tag"], false);
        assert_eq!(release["packages"]["."]["component"], "go");
    }

    #[test]
    fn release_please_packages_are_relative_to_the_repository_root() {
        let toml = "name = \"Petstore\"\nsdks = [\"typescript\", \"go\"]\n";
        let config: Config = toml::from_str(toml).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        let held: Vec<&Sdk> = sdks.iter().collect();
        let package = r#"{ "name": "petstore", "version": "2.0.0" }"#;
        let read =
            |path: &str| Ok((path == "api/typescript/package.json").then(|| package.to_owned()));
        let files: std::collections::BTreeMap<String, Vec<u8>> =
            release_scaffold(&config, &held, "api", "trunk", read)
                .unwrap()
                .into_iter()
                .collect();
        let json = |path: &str| -> Value { serde_json::from_slice(&files[path]).unwrap() };
        let release = json("release-please-config.json");
        assert_eq!(
            release["packages"]["api/typescript"]["component"],
            "typescript"
        );
        assert_eq!(release["packages"]["api/go"]["component"], "api/go");
        assert_eq!(
            json(".release-please-manifest.json"),
            json!({ "api/typescript": "2.0.0" })
        );
        let workflow = String::from_utf8_lossy(&files[RELEASE_WORKFLOW]);
        assert!(workflow.contains("branches: [\"trunk\"]\n"), "{workflow}");
        assert_eq!(package_path("api", "."), "api");
        assert_eq!(package_path(".", "Cargo.toml"), "Cargo.toml");
    }

    #[test]
    fn each_manifest_names_the_repository_its_sdk_lives_in() {
        let toml = "name = \"Acme\"\nsdks = [\"rust\", \"typescript\", \"python\", \"go\", \"java\", \"csharp\"]\n\
                    [rust]\nrepo = \"acme/acme-rust\"\n[typescript]\nrepo = \"acme/acme-node\"\n\
                    [python]\nrepo = \"acme/acme-python\"\n[csharp]\nrepo = \"acme/acme-dotnet\"\n";
        let mut config: Config = toml::from_str(toml).unwrap();
        config.home = crate::config::Home {
            url: Some("https://github.com/acme/api".into()),
            dir: "sdks".into(),
        };
        let file = |config: &Config, language: &str, path: &str| {
            let sdk = config.sdks(&[language.to_owned()]).unwrap().remove(0);
            let files = skeleton(config, &sdk, Path::new("/nonexistent")).unwrap();
            let (_, content) = files.into_iter().find(|(p, _)| p == path).unwrap();
            String::from_utf8(content).unwrap()
        };
        assert!(
            file(&config, "rust", "Cargo.toml")
                .contains("repository = \"https://github.com/acme/acme-rust\"\n")
        );
        let package: Value =
            serde_json::from_str(&file(&config, "typescript", "package.json")).unwrap();
        assert_eq!(
            package["repository"]["url"],
            "git+https://github.com/acme/acme-node.git"
        );
        assert!(
            file(&config, "python", "pyproject.toml")
                .contains("Repository = \"https://github.com/acme/acme-python\"\n")
        );
        assert!(
            file(&config, "csharp", "Acme/Acme.csproj")
                .contains("<RepositoryUrl>https://github.com/acme/acme-dotnet</RepositoryUrl>")
        );
        assert!(
            file(&config, "java", "gradle.properties")
                .contains("POM_SCM_URL=https://github.com/acme/api\n")
        );
        assert!(file(&config, "go", "go.mod").starts_with("module github.com/acme/api/sdks/go\n"));
        assert!(
            file(&config, "go", "README.md").contains("- Source: https://github.com/acme/api\n")
        );

        config.package.repository = Some("https://git.acme.dev/api".into());
        assert!(file(&config, "go", "go.mod").starts_with("module git.acme.dev/api/sdks/go\n"));
        assert!(file(&config, "rust", "Cargo.toml").contains("https://github.com/acme/acme-rust"));
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
