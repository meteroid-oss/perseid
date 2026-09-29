use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use heck::{ToKebabCase, ToUpperCamelCase};
use serde_json::Value;

use crate::{
    assets,
    config::{self, Config, LANGUAGES},
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
            toml += &format!("module = \"github.com/{name}/{name}-go\"\n");
        }
        added = true;
    }
    if added || !config_path.exists() {
        fsx::write(&config_path, toml.as_bytes())?;
        created.push(config_path.clone());
    }
    let (config, root) = Config::load(&config_path)?;
    for sdk in config.sdks(&[])?.iter().filter(|s| s.repo.is_none()) {
        let dir = root.join(&sdk.path);
        let mut context = config.context(sdk, &dir);
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
