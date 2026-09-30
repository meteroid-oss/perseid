use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use heck::{ToKebabCase, ToUpperCamelCase};
use serde_json::{Value, json};

use crate::{
    assets,
    config::{self, Config, LANGUAGES, Sdk, manifest_version},
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

pub fn run(init: Init, root: &Path) -> Result<Vec<PathBuf>> {
    let config_path = root.join(config::FILE);
    let mut created = Vec::new();
    let mut toml = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        let Some(spec) = init.spec.or_else(|| {
            SPECS
                .iter()
                .find(|s| root.join(s).is_file())
                .map(|s| s.to_string())
        }) else {
            bail!("no OpenAPI spec found, pass --spec <path or url>");
        };
        let doc: Value = serde_json::from_str(&spec::read(&spec, root)?)?;
        let name = init.name.unwrap_or_else(|| name_from_title(&doc));
        let base_url = init.base_url.or_else(|| {
            doc["servers"][0]["url"]
                .as_str()
                .filter(|u| u.starts_with("http"))
                .map(|u| u.trim_end_matches('/').to_owned())
        });
        let mut toml = format!("spec = {spec:?}\nname = {:?}\n", name.to_upper_camel_case());
        if let Some(url) = base_url {
            toml += &format!("base_url = {url:?}\n");
        }
        toml
    };
    let existing: toml::Table = toml.parse()?;
    let languages = match init.languages.is_empty() {
        true if existing.keys().any(|k| LANGUAGES.contains(&k.as_str())) => vec![],
        true => LANGUAGES.map(str::to_owned).to_vec(),
        false => init.languages,
    };
    let name = existing
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Client")
        .to_kebab_case();
    let mut added = false;
    for language in languages
        .iter()
        .filter(|l| !existing.contains_key(l.as_str()))
    {
        toml += &format!("\n[{language}]\n");
        if language == "go" {
            toml += &format!(
                "module = \"github.com/{name}/{name}-go\"\ninitialisms = true\npatch_nullable = true\n"
            );
        }
        if language == "csharp" {
            toml += "patch_nullable = true\n";
        }
        added = true;
    }
    if added || !config_path.exists() {
        fsx::write(&config_path, toml.as_bytes())?;
        created.push(config_path.clone());
    }
    let (config, root) = Config::load(&config_path)?;
    let mut packages = Vec::new();
    for sdk in config.sdks(&[])?.iter().filter(|s| s.repo.is_none()) {
        let dir = root.join(&sdk.path);
        let released = manifest_version(&dir);
        created.extend(scaffold(&config, sdk, &dir)?);
        packages.push(package(&config, sdk, &dir, released));
    }
    if init.release {
        created.extend(release(&root, &packages)?);
    }
    Ok(created)
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
    let mut created = scaffold(config, sdk, &dir)?;
    created.extend(release(repo, &[package(config, sdk, &dir, None)])?);
    Ok(created)
}

fn scaffold(config: &Config, sdk: &Sdk, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let mut context = config.context(sdk, dir);
    let path = context["java_package"].as_str().unwrap().replace('.', "/");
    context["java_package_path"] = path.into();
    for (path, content) in assets::under(&format!("scaffold/{}", sdk.language)) {
        let target = dir.join(tokens(path, &context)?);
        if target.exists() {
            continue;
        }
        let content = tokens(std::str::from_utf8(content)?, &context)?;
        fsx::write(&target, content.as_bytes())?;
        created.push(target);
    }
    Ok(created)
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
    if packages.is_empty() {
        return Ok(vec![]);
    }
    let read = |path: &Path| -> Result<Option<Value>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
            Err(_) => Ok(None),
        }
    };
    let config_path = repo.join("release-please-config.json");
    let manifest_path = repo.join(".release-please-manifest.json");
    let mut config = read(&config_path)?.unwrap_or_else(|| {
        json!({
            "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
            "bump-minor-pre-major": true,
            "bump-patch-for-minor-pre-major": true,
            "initial-version": "0.1.0",
            "tag-separator": "/",
            "packages": {},
        })
    });
    let mut manifest = read(&manifest_path)?.unwrap_or_else(|| json!({}));
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
    let mut created = Vec::new();
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
            fsx::write(
                &path,
                (serde_json::to_string_pretty(value)? + "\n").as_bytes(),
            )?;
            created.push(path);
        }
    }
    for (path, content) in assets::under("scaffold/release") {
        let target = repo.join(path);
        if !target.exists() {
            fsx::write(&target, content)?;
            created.push(target);
        }
    }
    Ok(created)
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
