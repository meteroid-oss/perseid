//! The files an SDK starts from: its package skeleton, and the release-please setup releasing it.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use heck::ToKebabCase;
use serde_json::{Value, json};

use crate::{
    assets,
    config::{Config, Sdk, manifest_version},
    fsx,
    generate::tokens,
};

/// Gives the SDK at `dir` its skeleton when it has no package manifest: manifest, README, error
/// types. The README's examples call operations of `spec`. Gives it a LICENSE when it has none.
pub fn bootstrap(
    config: &Config,
    root: &Path,
    sdk: &Sdk,
    dir: &Path,
    spec: &str,
) -> Result<Vec<PathBuf>> {
    let manifests: &[&str] = match sdk.language {
        "rust" => &["Cargo.toml"],
        "typescript" => &["package.json"],
        "python" => &["pyproject.toml", "setup.py"],
        "go" => &["go.mod"],
        "csharp" => &[],
        _ => &["build.gradle", "build.gradle.kts", "pom.xml"],
    };
    let mut created = Vec::new();
    if !has_license(dir)
        && let Some(text) = license_file(config, sdk, dir, this_year())
    {
        let target = dir.join("LICENSE");
        fsx::write(&target, &text)?;
        created.push(target);
    }
    let dotnet = || {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "sln"))
        })
    };
    if manifests.iter().any(|m| dir.join(m).exists()) || (sdk.language == "csharp" && dotnet()) {
        return Ok(created);
    }
    let examples = tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        crate::spec::api(spec, &config.filters_for(sdk)).map(|mut api| {
            api.drop_unsendable(sdk.language);
            crate::docs::examples(&api)
        })
    })
    .unwrap_or(Value::Null);
    let docs = Docs {
        overrides: Some(config.overrides_dir(root)),
        examples,
    };
    for (path, content) in skeleton(config, sdk, dir, &docs)? {
        let target = dir.join(path);
        if !target.exists() {
            fsx::write(&target, &content)?;
            created.push(target);
        }
    }
    Ok(created)
}

fn has_license(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().to_uppercase();
            ["LICENSE", "LICENCE", "COPYING"]
                .iter()
                .any(|l| name.starts_with(l))
        })
    })
}

/// The text of the license perseid.toml names, when it is one perseid ships: its copyright line
/// names the authors, or the API.
fn license_file(config: &Config, sdk: &Sdk, dir: &Path, year: i64) -> Option<Vec<u8>> {
    let license = config.package.license.as_deref()?.trim();
    let (_, text) = assets::under("scaffold/licenses").find(|(id, _)| *id == license)?;
    let mut context = config.context(sdk, dir);
    package_metadata(config, &mut context);
    let holder = context["authors_names"].as_str().unwrap_or(&config.name);
    let text = String::from_utf8_lossy(text)
        .replace("@@YEAR@@", &year.to_string())
        .replace("@@HOLDER@@", holder);
    Some(text.into_bytes())
}

/// The current year, from the mean length of a Gregorian year: off by a day at most.
fn this_year() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    1970 + (seconds / 31_556_952) as i64
}

/// Whether `perseid generate` writes the text of `license` as the SDKs' LICENSE.
pub fn ships_license(license: &str) -> bool {
    assets::under("scaffold/licenses").any(|(id, _)| id == license.trim())
}

/// What the README is rendered with: the templates overriding the built-in ones, whose
/// `docs.jinja` names the API in the SDK's language, and the operations its examples call.
struct Docs {
    overrides: Option<PathBuf>,
    examples: Value,
}

/// The skeleton of an SDK checked out at `dir`, relative to it.
fn skeleton(config: &Config, sdk: &Sdk, dir: &Path, docs: &Docs) -> Result<Vec<(String, Vec<u8>)>> {
    let context = skeleton_context(config, sdk, dir);
    let mut files = Vec::new();
    for (path, content) in assets::under(&format!("scaffold/{}", sdk.language)) {
        let mut content = without_unset_metadata(std::str::from_utf8(content)?, &context);
        if path == "README.md" {
            content = readme(&content, sdk.language, &context, docs)
                .with_context(|| format!("rendering the {} README", sdk.language))?;
        }
        let content = tokens(&content, &context)?;
        files.push((tokens(path, &context)?, content.into_bytes()));
    }
    Ok(files)
}

/// The manifests of the skeleton of an SDK checked out at `dir`, relative to it.
pub fn manifests(config: &Config, sdk: &Sdk, dir: &Path) -> Result<Vec<(String, String)>> {
    let context = skeleton_context(config, sdk, dir);
    let mut files = Vec::new();
    for (path, content) in assets::under(&format!("scaffold/{}", sdk.language)) {
        let name = Path::new(path).file_name().and_then(|n| n.to_str());
        if name.is_some_and(crate::manifest::is_manifest) {
            let content = without_unset_metadata(std::str::from_utf8(content)?, &context);
            files.push((tokens(path, &context)?, tokens(&content, &context)?));
        }
    }
    Ok(files)
}

fn skeleton_context(config: &Config, sdk: &Sdk, dir: &Path) -> Value {
    let mut context = config.context(sdk, dir);
    let path = context["java_package"].as_str().unwrap().replace('.', "/");
    context["java_package_path"] = path.into();
    package_metadata(config, &mut context);
    context
}

/// Renders the README template `source` with the `examples` of `docs`.
fn readme(source: &str, language: &str, context: &Value, docs: &Docs) -> Result<String> {
    let assets = tempfile::tempdir()?;
    assets::materialize(language, docs.overrides.as_deref(), assets.path())?;
    let templates = assets.path().join("templates").join(language);
    let templates = camino::Utf8Path::from_path(&templates).context("non UTF-8 path")?;
    let mut env = crate::template::env_with_dir(templates)?;
    env.set_keep_trailing_newline(true);
    env.add_global("sdk", minijinja::Value::from_serialize(context));
    let examples = minijinja::Value::from_serialize(&docs.examples);
    Ok(env.render_str(source, minijinja::context! { examples })?)
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
pub(crate) fn package_path(dir: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    match (dir, path) {
        ("" | ".", path) => path.to_owned(),
        (dir, "." | "") => dir.to_owned(),
        (dir, path) => format!("{dir}/{path}"),
    }
}

pub const RELEASE_WORKFLOW: &str = ".github/workflows/sdk-release.yml";
pub const CI_WORKFLOW: &str = ".github/workflows/sdk-ci.yml";

/// A workflow of the repositories holding SDKs, on their default `branch`. Its actions follow
/// `@v0`, so perseid releases don't change it: it is only written with the user's own credentials.
fn repository_workflow(path: &str, branch: &str) -> String {
    let (_, template) = assets::under("scaffold/release")
        .find(|(file, _)| *file == path)
        .expect("the embedded workflow");
    String::from_utf8_lossy(template).replace("\"@@BRANCH@@\"", &Value::from(branch).to_string())
}

/// The job of the release workflow reporting its releases.
const REPORT_JOB: &str = "\n  # Tells the repository holding perseid.toml";

/// The release workflow of a repository whose default branch is `branch`, reporting its
/// releases to the repository `report` names.
pub fn release_workflow(branch: &str, report: Option<&str>) -> Vec<u8> {
    let text = repository_workflow(RELEASE_WORKFLOW, branch);
    match report {
        Some(hub) => text.replace("\"@@HUB@@\"", &Value::from(hub).to_string()),
        None => text[..text.find(REPORT_JOB).unwrap_or(text.len())].to_owned(),
    }
    .into_bytes()
}

/// The workflow testing the SDKs at `paths` of a repository whose default branch is `branch`: on
/// changes under them only, unless one is the whole repository.
pub fn ci_workflow(branch: &str, paths: &[String]) -> Vec<u8> {
    let list = |items: Vec<String>| Value::from(items).to_string().replace(',', ", ");
    let watched = match paths.iter().any(|p| p == ".") {
        true => None,
        false => Some(list(
            paths
                .iter()
                .map(|p| format!("{p}/**"))
                .chain([CI_WORKFLOW.to_owned()])
                .collect(),
        )),
    };
    let mut out = String::new();
    for line in repository_workflow(CI_WORKFLOW, branch).split_inclusive('\n') {
        match &watched {
            None if line.contains("\"@@PATHS@@\"") => {}
            _ => out.push_str(line),
        }
    }
    out.replace("[\"@@PATHS@@\"]", watched.as_deref().unwrap_or_default())
        .replace("[\"@@SDKS@@\"]", &list(paths.to_vec()))
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
        let workflow = release_workflow(branch, config.reports_to());
        files.push((RELEASE_WORKFLOW.to_owned(), workflow));
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
        for action in ["release", "publish"] {
            let step = format!("uses: meteroid-oss/perseid/{action}@v0\n");
            assert!(workflow.contains(&step), "{workflow}");
        }
        assert!(workflow.contains("      id-token: write\n"), "{workflow}");

        let root = "name = \"Petstore\"\nsdks = [\"go\"]\nrepo = \"acme/petstore-{lang}\"\n";
        let config: Config = toml::from_str(root).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        let files = release_scaffold(&config, &[&sdks[0]], "", "main", |_| Ok(None)).unwrap();
        let release: Value = serde_json::from_slice(&files[0].1).unwrap();
        assert_eq!(release["packages"]["."]["include-component-in-tag"], false);
        assert_eq!(release["packages"]["."]["component"], "go");
    }

    #[test]
    fn release_workflows_report_to_the_repository_holding_perseid_toml_when_targets_wait() {
        let without = String::from_utf8(release_workflow("main", None)).unwrap();
        assert!(!without.contains("report"), "{without}");
        assert!(
            without.ends_with("nuget-api-key: ${{ secrets.NUGET_API_KEY }}\n"),
            "{without}"
        );
        let with = String::from_utf8(release_workflow("main", Some("acme/api-sdks"))).unwrap();
        assert!(with.starts_with(&without), "{with}");
        let job = &with[without.len()..];
        for line in [
            "  report:\n    needs: [release, publish]\n",
            "    if: ${{ !cancelled() && needs.publish.result == 'success' }}\n",
            "      - uses: meteroid-oss/perseid/report@v0\n",
            "          to: \"acme/api-sdks\"\n",
            "          tags: ${{ needs.release.outputs.tags }}\n",
        ] {
            assert!(job.contains(line), "{line} in {job}");
        }

        let workflow = |url: Option<&str>| {
            let toml = "name = \"A\"\nsdks = [\"go\"]\n[targets.docs]\nrepo = \"acme/docs\"\n";
            let mut config: Config = toml::from_str(toml).unwrap();
            config.home.url = url.map(str::to_owned);
            let sdks = config.sdks(&[]).unwrap();
            let files = release_scaffold(&config, &[&sdks[0]], "", "main", |_| Ok(None)).unwrap();
            files
                .into_iter()
                .find(|(p, _)| p == RELEASE_WORKFLOW)
                .unwrap()
                .1
        };
        assert_eq!(workflow(None), release_workflow("main", None));
        assert_eq!(
            workflow(Some("https://github.com/acme/api-sdks")),
            release_workflow("main", Some("acme/api-sdks"))
        );
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
            let docs = Docs {
                overrides: None,
                examples: Value::Null,
            };
            let files = skeleton(config, &sdk, Path::new("/nonexistent"), &docs).unwrap();
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
    fn the_csharp_readme_shows_dependency_injection_only_when_generated() {
        let readme = |toml: &str| {
            let config: Config = toml::from_str(toml).unwrap();
            let sdk = config.sdks(&["csharp".to_owned()]).unwrap().remove(0);
            let docs = Docs {
                overrides: None,
                examples: Value::Null,
            };
            let files = skeleton(&config, &sdk, Path::new("/nonexistent"), &docs).unwrap();
            let (_, content) = files.into_iter().find(|(p, _)| p == "README.md").unwrap();
            String::from_utf8(content).unwrap()
        };
        let plain = readme("name = \"Acme\"\nsdks = [\"csharp\"]\n");
        assert!(!plain.contains("AddAcmeClient"), "{plain}");
        assert!(!plain.contains("perseid.toml"), "{plain}");
        let injected = readme(
            "name = \"Acme\"\nsdks = [\"csharp\"]\n[csharp.context]\ndependency_injection = true\n",
        );
        assert!(injected.contains("## Dependency injection"), "{injected}");
        assert!(
            injected.contains("builder.Services.AddAcmeClient("),
            "{injected}"
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
