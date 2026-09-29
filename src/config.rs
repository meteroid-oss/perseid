use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use heck::{ToKebabCase, ToSnakeCase, ToUpperCamelCase};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::spec::{Filters, IncludeMode};

pub const FILE: &str = "perseid.toml";
pub const LANGUAGES: [&str; 5] = ["rust", "typescript", "python", "go", "java"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Path (relative to perseid.toml) or http(s) URL of the OpenAPI document.
    pub spec: String,
    /// Client name: `Acme` gives the `Acme` client and the `acme` package.
    pub name: String,
    pub base_url: Option<String>,
    pub version: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    pub patch_nullable: Option<bool>,
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
    /// Directory mirroring `templates/<lang>/…` and `runtime/<lang>/…` to override built-ins.
    /// Defaults to `.perseid`.
    pub overrides: Option<String>,
    #[serde(default)]
    pub include: IncludeMode,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub only: Vec<String>,
    pub rust: Option<Target>,
    pub typescript: Option<Target>,
    pub python: Option<Target>,
    pub go: Option<Target>,
    pub java: Option<Target>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Output directory, relative to the repository the SDK lives in.
    pub path: Option<String>,
    /// `owner/name` of a GitHub repository to generate into instead of this one.
    pub repo: Option<String>,
    /// Crate, npm, PyPI or Go package name, or Java package.
    pub package: Option<String>,
    /// Go module path.
    pub module: Option<String>,
    /// TypeScript modules re-exported from the entry point.
    #[serde(default)]
    pub exports: Vec<String>,
    pub base_url: Option<String>,
    pub version: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    pub patch_nullable: Option<bool>,
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
}

pub struct Sdk<'a> {
    pub language: &'static str,
    pub repo: Option<&'a str>,
    pub path: String,
    target: &'a Target,
}

impl Config {
    pub fn load(path: &Path) -> Result<(Self, PathBuf)> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "reading {} (run `perseid init` to create one)",
                path.display()
            )
        })?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        ensure!(
            config.name == config.name.to_upper_camel_case(),
            "`name` must be UpperCamelCase, e.g. \"{}\"",
            config.name.to_upper_camel_case()
        );
        let root = std::path::absolute(path)?.parent().unwrap().to_owned();
        Ok((config, root))
    }

    pub fn overrides_dir(&self, root: &Path) -> PathBuf {
        root.join(self.overrides.as_deref().unwrap_or(".perseid"))
    }

    pub fn filters(&self) -> Filters {
        Filters {
            include_mode: if self.only.is_empty() {
                self.include
            } else {
                IncludeMode::OnlySpecified
            },
            excluded: self.exclude.iter().cloned().collect(),
            specified: self.only.iter().cloned().collect(),
        }
    }

    pub fn sdks(&self, selected: &[String]) -> Result<Vec<Sdk<'_>>> {
        for name in selected {
            ensure!(
                LANGUAGES.contains(&name.as_str()),
                "unknown language `{name}`"
            );
        }
        let targets = [
            &self.rust,
            &self.typescript,
            &self.python,
            &self.go,
            &self.java,
        ];
        let sdks = LANGUAGES
            .into_iter()
            .zip(targets)
            .filter_map(|(language, target)| Some((language, target.as_ref()?)))
            .filter(|(language, _)| selected.is_empty() || selected.iter().any(|s| s == language))
            .map(|(language, target)| Sdk {
                language,
                repo: target.repo.as_deref(),
                path: target
                    .path
                    .clone()
                    .unwrap_or_else(|| if target.repo.is_some() { "." } else { language }.into()),
                target,
            })
            .collect::<Vec<_>>();
        ensure!(
            !sdks.is_empty(),
            "nothing to generate: add a [rust], [typescript], [python], [go] or [java] table"
        );
        Ok(sdks)
    }

    /// Values exposed to templates as `sdk`, for an SDK checked out at `dir`.
    pub fn context(&self, sdk: &Sdk, dir: &Path) -> Value {
        let (language, target) = (sdk.language, sdk.target);
        let snake = self.name.to_snake_case();
        let kebab = self.name.to_kebab_case();
        let package = target.package.clone().unwrap_or_else(|| match language {
            "java" => format!("com.{}", snake.replace('_', "")),
            "typescript" => kebab.clone(),
            _ => snake.clone(),
        });
        let version = target.version.clone().or_else(|| self.version.clone());
        let version = version
            .or_else(|| manifest_version(dir))
            .unwrap_or_else(|| "0.1.0".into());
        let pick = |own: &Option<String>, shared: &Option<String>, default: &str| {
            own.clone()
                .or_else(|| shared.clone())
                .unwrap_or_else(|| default.into())
        };
        let mut context = json!({
            "client_name": self.name,
            "package_name": if language == "typescript" { &snake } else { &package },
            "rust_crate": if language == "rust" { package.replace('-', "_") } else { snake.clone() },
            "java_package": if language == "java" { package.clone() } else { format!("com.{snake}") },
            "npm_package": if language == "typescript" { &package } else { &kebab },
            "go_module": target.module,
            "default_base_url": pick(&target.base_url, &self.base_url, "http://localhost"),
            "user_agent_prefix": pick(&target.user_agent, &self.user_agent, &kebab),
            "header_prefix": pick(&target.header_prefix, &self.header_prefix, &kebab),
            "patch_nullable": target.patch_nullable.or(self.patch_nullable).unwrap_or(false),
            "version": version,
            "extra_exports": target.exports,
        });
        let map = context.as_object_mut().unwrap();
        map.extend(self.context.clone());
        map.extend(target.context.clone());
        context
    }
}

/// The SDK's own package manifest owns its version, so release tooling keeps working.
fn manifest_version(dir: &Path) -> Option<String> {
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).ok();
    if let Some(json) = read("package.json") {
        let value: Value = serde_json::from_str(&json).ok()?;
        return value["version"].as_str().map(str::to_owned);
    }
    for (file, table) in [("Cargo.toml", "package"), ("pyproject.toml", "project")] {
        if let Some(text) = read(file) {
            let value: toml::Value = toml::from_str(&text).ok()?;
            return value
                .get(table)?
                .get("version")?
                .as_str()
                .map(str::to_owned);
        }
    }
    if let Some(properties) = read("gradle.properties") {
        let version = properties.lines().find_map(|l| {
            let (key, value) = l.split_once('=')?;
            ["VERSION_NAME", "version"]
                .contains(&key.trim())
                .then(|| value.trim().to_owned())
        });
        if version.is_some() {
            return version;
        }
    }
    read("version.txt").map(|v| v.trim().to_owned())
}
