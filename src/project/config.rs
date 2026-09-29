use super::{assets::Assets, io, prepare::Preparation};
use crate::runner::{Config as GeneratorConfig, Global, LANGUAGES, LanguageConfig, Task};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub file: Option<String>,
    pub git: Option<String>,
    pub r#ref: Option<String>,
    pub path: Option<String>,
    pub url: Option<String>,
    pub command: Option<Vec<String>>,
    #[serde(default = "snapshot")]
    pub snapshot: String,
}
fn snapshot() -> String {
    "spec/openapi.json".into()
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseHooks {
    #[serde(default)]
    pub version_commands: Vec<String>,
    pub read_version: Option<String>,
    pub publish: Option<String>,
    pub is_published: Option<String>,
    pub tag: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub language: Option<String>,
    pub directory: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    #[serde(default)]
    pub sdk: BTreeMap<String, Value>,
    pub task: Option<Vec<Task>>,
    pub runtime_output_dir: Option<String>,
    #[serde(default)]
    pub runtime_overrides: BTreeMap<String, String>,
    pub template_overrides: Option<String>,
    pub prepare: Option<Preparation>,
    #[serde(default)]
    pub extra_codegen_args: Vec<String>,
    #[serde(default)]
    pub format_commands: Option<Vec<String>>,
    #[serde(default)]
    pub check_commands: Vec<String>,
    pub release: Option<ReleaseHooks>,
}
impl Target {
    pub fn language<'a>(&'a self, name: &'a str) -> &'a str {
        self.language.as_deref().unwrap_or(name)
    }
    pub fn directory<'a>(&'a self, name: &'a str) -> &'a str {
        self.directory
            .as_deref()
            .unwrap_or_else(|| self.language(name))
    }
    pub fn repository(&self) -> &str {
        self.repository.as_deref().unwrap_or(".")
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSettings {
    #[serde(default = "policy")]
    pub policy: String,
    #[serde(default = "initial")]
    pub initial_version: String,
}
fn policy() -> String {
    "independent".into()
}
fn initial() -> String {
    "0.1.0".into()
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub perseid_version: String,
    pub source: Source,
    #[serde(default)]
    pub sdk: BTreeMap<String, Value>,
    pub targets: BTreeMap<String, Target>,
    pub release: Option<ReleaseSettings>,
}
#[derive(Clone)]
pub struct Project {
    pub path: PathBuf,
    pub root: PathBuf,
    pub config: Config,
    pub versions: Versions,
    pub requests: super::release::Requests,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub schema: u32,
    pub versions: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}
pub(super) fn identifier(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}
/// Destination-owned customizations. Repository layout/source/release policy stay in the controller.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overrides {
    #[serde(default)]
    targets: BTreeMap<String, TargetOverride>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetOverride {
    #[serde(default)]
    sdk: BTreeMap<String, Value>,
    template_overrides: Option<String>,
    #[serde(default)]
    runtime_overrides: BTreeMap<String, String>,
    format_commands: Option<Vec<String>>,
    check_commands: Option<Vec<String>>,
    read_version: Option<String>,
    version_commands: Option<Vec<String>>,
}
impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        let root = path.parent().unwrap().to_owned();
        let config: Config = toml::from_str(&fs::read_to_string(&path)?)?;
        ensure!(identifier(&config.name), "Invalid project name");
        ensure!(
            config.perseid_version == env!("CARGO_PKG_VERSION"),
            "Project pins a different Perseid version"
        );
        let source = &config.source;
        ensure!(
            [
                source.file.is_some(),
                source.git.is_some(),
                source.url.is_some(),
                source.command.is_some()
            ]
            .iter()
            .filter(|v| **v)
            .count()
                == 1,
            "Configure exactly one source: file, git, url, command"
        );
        ensure!(
            source.git.is_some() || (source.r#ref.is_none() && source.path.is_none()),
            "source.path/ref require source.git"
        );
        ensure!(
            source.git.is_none() || source.path.is_some(),
            "Git source requires path"
        );
        ensure!(
            source.command.as_ref().is_none_or(|c| !c.is_empty()),
            "Exporter command cannot be empty"
        );
        ensure!(!config.targets.is_empty(), "Configure at least one target");
        let mut destinations = Vec::new();
        for (name, target) in &config.targets {
            ensure!(identifier(name), "Invalid target name {name}");
            ensure!(
                LANGUAGES.contains(&target.language(name)),
                "Unsupported target language"
            );
            let directory = io::relative(Path::new("/destination"), target.directory(name))?;
            ensure!(
                !Path::new(target.directory(name))
                    .components()
                    .any(|part| io::IGNORED
                        .iter()
                        .chain([".perseid"].iter())
                        .any(|reserved| part.as_os_str() == *reserved)),
                "Target uses a reserved metadata/build directory"
            );
            ensure!(
                !target.repository().is_empty() && !target.repository().starts_with('-'),
                "Invalid repository"
            );
            for (repository, previous, branch) in &destinations {
                if repository == &target.repository() {
                    ensure!(
                        branch == &target.branch,
                        "Targets in one repository must use the same base branch"
                    );
                    ensure!(
                        !directory.starts_with(previous)
                            && !Path::new(previous).starts_with(&directory),
                        "Target directories must not overlap"
                    );
                }
            }
            destinations.push((target.repository(), directory, target.branch.clone()));
            if let Some(release) = &target.release {
                ensure!(
                    release.publish.is_some() == release.is_published.is_some(),
                    "Publishing requires publish and is_published hooks"
                );
            }
        }
        if let Some(release) = &config.release {
            ensure!(
                ["lockstep", "independent"].contains(&release.policy.as_str()),
                "Invalid release policy"
            );
            super::release::version_tuple(&release.initial_version)?;
        }
        let initial = config
            .release
            .as_ref()
            .map(|r| r.initial_version.clone())
            .unwrap_or_else(initial);
        let mut versions = Versions {
            schema: 1,
            versions: config
                .targets
                .keys()
                .map(|name| (name.clone(), initial.clone()))
                .collect(),
            notes: BTreeMap::new(),
        };
        let version_file = io::relative(&root, "perseid.versions.json")?;
        if version_file.exists()
            && config
                .release
                .as_ref()
                .is_some_and(|r| r.policy == "lockstep")
        {
            let stored: Versions = serde_json::from_value(io::read_json(&version_file)?)?;
            ensure!(stored.schema == 1, "Unsupported versions schema");
            versions.versions.extend(stored.versions);
            versions.notes = stored.notes;
        }
        for version in versions.versions.values() {
            super::release::version_tuple(version)?;
        }
        let request_path = io::relative(&root, "perseid.releases.json")?;
        let requests = if request_path.exists() {
            serde_json::from_value(io::read_json(&request_path)?)?
        } else {
            super::release::Requests::default()
        };
        let project = Self {
            path,
            root,
            config,
            versions,
            requests,
        };
        let snapshot = project.snapshot()?;
        ensure!(
            ![
                project.path.clone(),
                project.lock_path(),
                project.versions_path()
            ]
            .contains(&snapshot),
            "Snapshot would overwrite project metadata"
        );
        ensure!(
            !Path::new(&project.config.source.snapshot)
                .components()
                .any(
                    |c| io::IGNORED.contains(&c.as_os_str().to_str().unwrap_or(""))
                        || c.as_os_str() == ".perseid"
                ),
            "Snapshot uses a reserved path"
        );
        Ok(project)
    }
    pub fn with_overrides(&self, root: &Path, names: &[String]) -> Result<Self> {
        let mut project = self.clone();
        let path = io::relative(root, ".perseid/overrides.toml")?;
        if !path.exists() {
            return Ok(project);
        }
        let overrides: Overrides = toml::from_str(&fs::read_to_string(path)?)?;
        for name in overrides.targets.keys() {
            ensure!(
                self.config.targets.contains_key(name),
                "Unknown override target {name}"
            );
        }
        for (name, custom) in overrides.targets {
            if !names.contains(&name) {
                continue;
            }
            let target = project.config.targets.get_mut(&name).unwrap();
            target.sdk.extend(custom.sdk);
            if custom.template_overrides.is_some() {
                target.template_overrides = custom.template_overrides;
            }
            target.runtime_overrides.extend(custom.runtime_overrides);
            if custom.format_commands.is_some() {
                target.format_commands = custom.format_commands;
            }
            if let Some(commands) = custom.check_commands {
                target.check_commands = commands;
            }
            if custom.read_version.is_some() || custom.version_commands.is_some() {
                let release = target.release.get_or_insert_with(ReleaseHooks::default);
                if custom.read_version.is_some() {
                    release.read_version = custom.read_version;
                }
                if let Some(commands) = custom.version_commands {
                    release.version_commands = commands;
                }
            }
        }
        Ok(project)
    }
    pub fn snapshot(&self) -> Result<PathBuf> {
        io::relative(&self.root, &self.config.source.snapshot)
    }
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("perseid.lock.json")
    }
    pub fn versions_path(&self) -> PathBuf {
        self.root.join(if self.independent() {
            "perseid.releases.json"
        } else {
            "perseid.versions.json"
        })
    }
    pub fn independent(&self) -> bool {
        self.config
            .release
            .as_ref()
            .is_none_or(|r| r.policy == "independent")
    }
    pub fn fingerprint(&self) -> Result<String> {
        Ok(io::sha(&io::json(&self.config)?))
    }
    pub fn selected(&self, names: &[String]) -> Result<Vec<String>> {
        ensure!(
            names
                .iter()
                .all(|name| self.config.targets.contains_key(name)),
            "Unknown target"
        );
        Ok(if names.is_empty() {
            self.config.targets.keys().cloned().collect()
        } else {
            names
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
    }
    pub fn groups(&self, names: &[String]) -> Result<BTreeMap<String, Vec<String>>> {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in self.selected(names)? {
            groups
                .entry(self.config.targets[&name].repository().into())
                .or_default()
                .push(name);
        }
        Ok(groups)
    }
    pub fn generator_config(&self, name: &str, version: &str) -> Result<GeneratorConfig> {
        let target = &self.config.targets[name];
        let language = target.language(name);
        let directory = Path::new(target.directory(name));
        let package = self.config.name.replace('-', "_");
        let client = package
            .split('_')
            .filter(|p| !p.is_empty())
            .map(|p| format!("{}{}", p[..1].to_ascii_uppercase(), &p[1..]))
            .collect::<String>();
        let mut sdk: BTreeMap<String, Value> = serde_json::from_value(
            json!({"client_name":client, "package_name":package, "rust_crate":package, "java_package":format!("com.{package}"), "default_base_url":"http://localhost", "user_agent_prefix":self.config.name, "header_prefix":self.config.name, "patch_nullable":false}),
        )?;
        sdk.extend(self.config.sdk.clone());
        sdk.extend(target.sdk.clone());
        sdk.insert("version".into(), version.into());
        let setting = |key| -> Result<&str> {
            sdk.get(key)
                .and_then(Value::as_str)
                .with_context(|| format!("SDK {key} must be a string"))
        };
        let runtime = match language {
            "rust" | "typescript" => directory.join("src"),
            "python" => directory.join(setting("package_name")?),
            "java" => directory
                .join("src/main/java")
                .join(setting("java_package")?.replace('.', "/")),
            _ => directory.to_owned(),
        };
        let extension = match language {
            "rust" => "rs",
            "typescript" => "ts",
            "python" => "py",
            "java" => "java",
            _ => "go",
        };
        let mut tasks = vec!["api_resource", "api_summary", "component_type"];
        if ["rust", "typescript", "python"].contains(&language) {
            tasks.push("component_type_summary");
        }
        if language == "java" {
            tasks.push("operation_options");
        }
        let tasks = tasks
            .iter()
            .map(|task| {
                let folder = if language == "go" {
                    runtime.clone()
                } else if task.starts_with("component") {
                    runtime.join("models")
                } else if *task != "api_summary" || ["rust", "python"].contains(&language) {
                    runtime.join("api")
                } else {
                    runtime.clone()
                };
                Task {
                    template: format!("{language}/{task}.{extension}.jinja"),
                    output_dir: folder.to_string_lossy().into_owned(),
                    extra_codegen_args: vec![],
                }
            })
            .collect();
        Ok(GeneratorConfig {
            global: Global {
                perseid_version: self.config.perseid_version.clone(),
                input_files: vec![self.config.source.snapshot.clone()],
                version_file: None,
                template_overrides: target.template_overrides.clone(),
                prepare: target.prepare.clone(),
                sdk,
            },
            languages: BTreeMap::from([(
                language.into(),
                LanguageConfig {
                    task: target.task.clone().unwrap_or(tasks),
                    runtime_output_dir: Some(
                        target
                            .runtime_output_dir
                            .clone()
                            .unwrap_or_else(|| runtime.to_string_lossy().into_owned()),
                    ),
                    runtime_overrides: target.runtime_overrides.clone(),
                    extra_codegen_args: target.extra_codegen_args.clone(),
                    ..Default::default()
                },
            )]),
        })
    }
    pub fn plan(&self, names: &[String], assets: &Assets) -> Result<Value> {
        let lock = super::source::locked(self, assets)?;
        let mut targets = Vec::new();
        for (repository, selected) in self.groups(names)? {
            let root = super::workspace::working_root(self, &repository, &selected)?;
            let effective = self.with_overrides(&root, &selected)?;
            for name in selected {
                let target = &self.config.targets[&name];
                let resolved = super::release::resolve(&effective, &root, &root, &name, assets)?;
                targets.push(json!({"name":name,"language":target.language(&name),"repository":target.repository(),"directory":target.directory(&name),"version":resolved.version}));
            }
        }
        Ok(json!({"source":lock.source,"spec_sha256":lock.spec_sha256,"targets":targets}))
    }
}
