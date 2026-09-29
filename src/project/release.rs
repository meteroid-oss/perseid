use super::{assets::Assets, config::Project, io, source, workspace as ws};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// The controller records intent, never a duplicate of an independent SDK's version.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Requests {
    #[serde(default)]
    pub requests: BTreeMap<String, Request>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub bump: String,
    pub notes: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    #[serde(default)]
    pub history: BTreeMap<String, PastRelease>,
    pub request: String,
    pub base: String,
    pub version: String,
    pub notes: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PastRelease {
    pub version: String,
    pub notes: String,
}
pub struct Resolved {
    pub version: String,
    pub candidate: Option<Candidate>,
}
fn candidate_path(root: &Path, name: &str) -> Result<PathBuf> {
    io::relative(root, format!(".perseid/releases/{name}.json"))
}
fn tag_pattern(project: &Project, name: &str) -> String {
    let target = &project.config.targets[name];
    let default = if target.language(name) == "go" {
        let prefix = Path::new(target.directory(name))
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if prefix.is_empty() {
            "v{version}".into()
        } else {
            format!("{prefix}/v{{version}}")
        }
    } else {
        format!("{name}/v{{version}}")
    };
    target
        .release
        .as_ref()
        .and_then(|r| r.tag.as_deref())
        .unwrap_or(&default)
        .replace("{target}", name)
}
fn latest_tag(project: &Project, git_root: &Path, name: &str) -> Result<Option<String>> {
    if !git_root.join(".git").exists() {
        return Ok(None);
    }
    let pattern = tag_pattern(project, name);
    let (prefix, suffix) = pattern
        .split_once("{version}")
        .context("Release tag must contain {version}")?;
    let tags = io::git(git_root, &["tag", "--list"])?;
    Ok(tags
        .lines()
        .filter_map(|tag| {
            let version = tag.strip_prefix(prefix)?.strip_suffix(suffix)?;
            version_tuple(version)
                .ok()
                .map(|tuple| (tuple, version.to_owned()))
        })
        .max()
        .map(|(_, version)| version))
}
fn current_version(
    project: &Project,
    root: &Path,
    git_root: &Path,
    name: &str,
    assets: &Assets,
) -> Result<String> {
    let target = &project.config.targets[name];
    let directory = io::relative(root, target.directory(name))?;
    let hooks = target.release.as_ref();
    let value = if let Some(command) = hooks.and_then(|r| r.read_version.as_ref()) {
        Some(io::text(
            Command::new("bash")
                .args(["-e", "-o", "pipefail", "-c", command])
                .current_dir(&directory)
                .envs(assets.env()),
        )?)
    } else {
        match target.language(name) {
            "rust" | "python" => {
                let (file, section) = if target.language(name) == "rust" {
                    ("Cargo.toml", "package")
                } else {
                    ("pyproject.toml", "project")
                };
                let path = io::relative(&directory, file)?;
                if path.exists() {
                    let document: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
                    Some(document.get(section).and_then(|s| s.get("version")).and_then(toml::Value::as_str)
                        .context("Version must be a literal string; configure release.read_version for custom metadata")?.to_owned())
                } else {
                    None
                }
            }
            "typescript" => {
                let path = io::relative(&directory, "package.json")?;
                if path.exists() {
                    Some(
                        io::read_json(&path)?["version"]
                            .as_str()
                            .context("package.json needs a version")?
                            .to_owned(),
                    )
                } else {
                    None
                }
            }
            "java"
                if directory.join("pom.xml").exists()
                    || directory.join("build.gradle").exists()
                    || directory.join("build.gradle.kts").exists() =>
            {
                bail!("Target {name} needs release.read_version for Maven/Gradle metadata")
            }
            _ => None,
        }
    };
    let value = match value {
        Some(value) => Some(value),
        None => latest_tag(project, git_root, name)?,
    }
    .unwrap_or_else(|| project.versions.versions[name].clone());
    version_tuple(&value)?;
    Ok(value)
}
pub fn resolve(
    project: &Project,
    root: &Path,
    git_root: &Path,
    name: &str,
    assets: &Assets,
) -> Result<Resolved> {
    if !project.independent() {
        return Ok(Resolved {
            version: project.versions.versions[name].clone(),
            candidate: None,
        });
    }
    let current = current_version(project, root, git_root, name, assets)?;
    let path = candidate_path(root, name)?;
    let previous: Option<Candidate> = if path.exists() {
        Some(serde_json::from_value(io::read_json(&path)?)?)
    } else {
        None
    };
    let request = project.requests.requests.get(name);
    // An intent already applied in this repository must never bump again.
    let mut candidate = match (request, previous) {
        (Some(request), Some(previous)) if request.id == previous.request => Some(previous),
        (Some(request), previous) => {
            let mut history = BTreeMap::new();
            if let Some(previous) = previous {
                history = previous.history;
                ensure!(
                    !history.contains_key(&request.id),
                    "Release request for {name} is stale; a newer request has already been applied"
                );
                history.insert(
                    previous.request,
                    PastRelease {
                        version: previous.version,
                        notes: previous.notes,
                    },
                );
            }
            Some(Candidate {
                history,
                request: request.id.clone(),
                base: current.clone(),
                version: next(&current, &request.bump)?,
                notes: request.notes.clone(),
            })
        }
        (None, previous) => previous,
    };
    let mut version = current.clone();
    if let Some(candidate) = &mut candidate {
        version_tuple(&candidate.base)?;
        version_tuple(&candidate.version)?;
        if version_tuple(&current)? > version_tuple(&candidate.base)? {
            // The manifest (including edits during review/manual releases) wins.
            // Go has no manifest; its pending version may be ahead of its last tag.
            version = if project.config.targets[name].language(name) == "go" {
                if version_tuple(&current)? > version_tuple(&candidate.version)? {
                    current
                } else {
                    candidate.version.clone()
                }
            } else {
                current
            };
        } else {
            ensure!(
                current == candidate.base,
                "SDK {name} version moved backwards; review its release metadata"
            );
            version = candidate.version.clone();
        }
        // A published intent keeps its immutable release identity even if the
        // destination subsequently releases a manual version outside Perseid.
        let tag = tag_pattern(project, name).replace("{version}", &candidate.version);
        let tagged = git_root.join(".git").exists()
            && io::git(git_root, &["tag", "--list", "--", &tag])?
                .lines()
                .any(|line| line == tag);
        if !tagged {
            candidate.version = version.clone();
        }
    }
    Ok(Resolved { version, candidate })
}
pub fn version_tuple(value: &str) -> Result<[u64; 3]> {
    let parts = value.split('.').collect::<Vec<_>>();
    ensure!(
        parts.len() == 3
            && parts.iter().all(|p| !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (*p == "0" || !p.starts_with('0'))),
        "Expected a stable major.minor.patch version: {value}"
    );
    Ok([parts[0].parse()?, parts[1].parse()?, parts[2].parse()?])
}
fn next(current: &str, requested: &str) -> Result<String> {
    let mut parts = version_tuple(current)?;
    if let Some(index) = ["major", "minor", "patch"]
        .iter()
        .position(|p| *p == requested)
    {
        parts[index] = parts[index].checked_add(1).context("Version overflow")?;
        parts[index + 1..].fill(0);
        Ok(format!("{}.{}.{}", parts[0], parts[1], parts[2]))
    } else {
        ensure!(
            version_tuple(requested)? > parts,
            "New version must be greater than {current}"
        );
        Ok(requested.into())
    }
}
pub fn prepare_version(
    project: &Project,
    names: &[String],
    assets: &Assets,
    requested: &str,
    notes: &str,
) -> Result<Value> {
    source::locked(project, assets)?;
    ensure!(!notes.trim().is_empty(), "Provide release notes for review");
    let selected = project.selected(names)?;
    if project.independent() {
        if !["major", "minor", "patch"].contains(&requested) {
            version_tuple(requested)?;
        }
        let mut state = project.requests.clone();
        // A new invocation is a new intent even when bump and notes are identical.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        for name in selected {
            state.requests.insert(
                name.clone(),
                Request {
                    id: io::sha(format!("{name}:{nonce}:{requested}:{notes}").as_bytes()),
                    bump: requested.into(),
                    notes: notes.trim().into(),
                },
            );
        }
        io::write(&project.versions_path(), &io::json(&state)?)?;
        return Ok(serde_json::to_value(state)?);
    }
    let mut state = project.versions.clone();
    if project
        .config
        .release
        .as_ref()
        .is_none_or(|r| r.policy == "lockstep")
    {
        ensure!(
            selected.len() == project.config.targets.len(),
            "Lockstep releases must include every target"
        );
        let current = selected
            .iter()
            .map(|n| {
                Ok((
                    version_tuple(&state.versions[n])?,
                    state.versions[n].clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap()
            .1;
        let value = next(&current, requested)?;
        for name in &selected {
            state.versions.insert(name.clone(), value.clone());
        }
    } else {
        for name in &selected {
            state
                .versions
                .insert(name.clone(), next(&state.versions[name], requested)?);
        }
    }
    for name in selected {
        state.notes.insert(name, notes.trim().into());
    }
    io::write(
        &io::relative(&project.root, "perseid.versions.json")?,
        &io::json(&state)?,
    )?;
    Ok(serde_json::to_value(state)?)
}
pub fn record(root: &Path, name: &str, resolved: &Resolved) -> Result<()> {
    if let Some(candidate) = &resolved.candidate {
        io::write(&candidate_path(root, name)?, &io::json(candidate)?)?;
    }
    Ok(())
}
fn stamp_toml(path: &Path, section: &str, version: &str) -> Result<()> {
    let mut document = fs::read_to_string(path)?.parse::<toml_edit::DocumentMut>()?;
    let old = document
        .get(section)
        .and_then(|v| v.get("version"))
        .and_then(toml_edit::Item::as_value)
        .context("Manifest needs a literal version or release.version_commands")?;
    ensure!(
        old.as_str().is_some(),
        "Manifest needs a literal version or release.version_commands"
    );
    let decor = old.decor().clone();
    let mut value = toml_edit::Value::from(version);
    *value.decor_mut() = decor;
    document[section]["version"] = toml_edit::Item::Value(value);
    io::write(path, document.to_string().as_bytes())
}
pub fn stamp(
    project: &Project,
    name: &str,
    directory: &Path,
    assets: &Assets,
    version: &str,
) -> Result<()> {
    let target = &project.config.targets[name];
    if target.language(name) == "go" {
        let path = io::relative(directory, "go.mod")?;
        if path.exists() {
            let text = fs::read_to_string(path)?;
            let module = text
                .lines()
                .find_map(|line| line.strip_prefix("module "))
                .context("go.mod needs a module declaration")?
                .trim();
            let major = version_tuple(version)?[0];
            ensure!(
                major < 2 || module.ends_with(&format!("/v{major}")),
                "Go {version} requires a /v{major} module path; update go.mod before generation"
            );
        }
    }
    let mut env = assets.env();
    env.insert("PERSEID_VERSION".into(), version.into());
    if let Some(release) = &target.release
        && !release.version_commands.is_empty()
    {
        return io::hooks(&release.version_commands, directory, &env);
    }
    match target.language(name) {
        "rust" if directory.join("Cargo.toml").exists() => {
            let manifest = io::relative(directory, "Cargo.toml")?;
            stamp_toml(&manifest, "package", version)?;
            let lock_path = io::relative(directory, "Cargo.lock")?;
            if lock_path.exists() {
                let package: toml::Value = toml::from_str(&fs::read_to_string(&manifest)?)?;
                let name = package["package"]["name"]
                    .as_str()
                    .context("Cargo package needs a name")?;
                let mut lock = fs::read_to_string(&lock_path)?.parse::<toml_edit::DocumentMut>()?;
                if let Some(packages) = lock
                    .get_mut("package")
                    .and_then(toml_edit::Item::as_array_of_tables_mut)
                {
                    for entry in packages.iter_mut() {
                        if entry.get("name").and_then(toml_edit::Item::as_str) == Some(name)
                            && !entry.contains_key("source")
                        {
                            entry["version"] = toml_edit::value(version);
                        }
                    }
                }
                io::write(&lock_path, lock.to_string().as_bytes())?;
            }
        }
        "python" if directory.join("pyproject.toml").exists() => stamp_toml(
            &io::relative(directory, "pyproject.toml")?,
            "project",
            version,
        )?,
        "typescript" if directory.join("package.json").exists() => {
            let path = io::relative(directory, "package.json")?;
            let mut value = io::read_json(&path)?;
            value["version"] = version.into();
            io::write(&path, &io::json(&value)?)?;
            for name in ["package-lock.json", "npm-shrinkwrap.json"] {
                let path = io::relative(directory, name)?;
                if path.exists() {
                    let mut lock = io::read_json(&path)?;
                    lock["version"] = version.into();
                    if let Some(root) = lock.get_mut("packages").and_then(|p| p.get_mut("")) {
                        root["version"] = version.into();
                    }
                    io::write(&path, &io::json(&lock)?)?;
                }
            }
        }
        "java" => bail!("Target {name} needs release.version_commands for Maven/Gradle metadata"),
        _ => (),
    }
    Ok(())
}
struct Pending {
    temporary: tempfile::TempDir,
    root: PathBuf,
    directory: PathBuf,
    repo: String,
    name: String,
    version: String,
    tag: String,
    commit: String,
    exists: bool,
    notes: String,
    publish: String,
    probe: String,
}
pub fn release(
    project: &Project,
    names: &[String],
    assets: &Assets,
    dry_run: bool,
) -> Result<Value> {
    let lock = source::locked(project, assets)?;
    ensure!(
        project.versions_path().exists(),
        "Prepare and commit release metadata first"
    );
    for path in [
        project.path.clone(),
        project.snapshot()?,
        project.lock_path(),
        project.versions_path(),
    ] {
        let relative = path.strip_prefix(&project.root)?;
        let committed = io::output(
            Command::new("git")
                .args(["show", &format!("HEAD:{}", relative.display())])
                .current_dir(&project.root),
        )?
        .stdout;
        ensure!(
            committed == fs::read(&path)?,
            "Commit release input before publishing: {}",
            relative.display()
        );
    }
    let mut pending = Vec::new();
    for (repository, selected) in project.groups(names)? {
        let repo = ws::github_repo(&ws::repository_url(project, &repository)?)?;
        for name in selected {
            let target = &project.config.targets[&name];
            let config = target
                .release
                .as_ref()
                .context("Target needs release.publish and release.is_published")?;
            let publish = config.publish.clone().context("Missing publish hook")?;
            let probe = config
                .is_published
                .clone()
                .context("Missing is_published hook")?;
            let temporary = ws::delivery(project, &repository, std::slice::from_ref(&name))?;
            let root = temporary.path().join("repo");
            let (version, notes, historical) = if project.independent() {
                let candidate: Candidate =
                    serde_json::from_value(io::read_json(&candidate_path(&root, &name)?)?)
                        .context("Merge the SDK release metadata before publishing")?;
                let request = project
                    .requests
                    .requests
                    .get(&name)
                    .context("No release request for this target")?;
                if candidate.request == request.id {
                    (candidate.version, candidate.notes, false)
                } else {
                    let previous = candidate
                        .history
                        .get(&request.id)
                        .context("Merge the requested SDK release first")?;
                    (previous.version.clone(), previous.notes.clone(), true)
                }
            } else {
                (
                    project.versions.versions[&name].clone(),
                    project
                        .versions
                        .notes
                        .get(&name)
                        .cloned()
                        .context("No reviewed release notes")?,
                    false,
                )
            };
            ensure!(!notes.trim().is_empty(), "No reviewed release notes");
            let tag = tag_pattern(project, &name).replace("{version}", &version);
            ensure!(!tag.contains(['{', '}']), "Unknown tag placeholder");
            io::text(Command::new("git").args(["check-ref-format", &format!("refs/tags/{tag}")]))?;
            let reference = format!("refs/tags/{tag}");
            let exists =
                !io::git(&root, &["ls-remote", "--tags", "origin", &reference])?.is_empty();
            ensure!(
                !historical || exists,
                "A superseded release request can only resume an existing tag"
            );
            if !exists && let Some(latest) = latest_tag(project, &root, &name)? {
                ensure!(
                    version_tuple(&version)? > version_tuple(&latest)?,
                    "SDK {name} already has release {latest}; prepare a newer version"
                );
            }
            if exists {
                io::git(&root, &["fetch", "--quiet", "origin", &reference])?;
                io::git(&root, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
                io::files(&root, false)?;
            }
            let commit = io::git(&root, &["rev-parse", "HEAD"])?;
            // Validate the reviewed/tagged version exactly, even if later releases exist.
            let mut validation = project.clone();
            validation.config.release = Some(super::config::ReleaseSettings {
                policy: "lockstep".into(),
                initial_version: "0.1.0".into(),
            });
            validation
                .versions
                .versions
                .insert(name.clone(), version.clone());
            ws::render(
                &validation,
                &root,
                std::slice::from_ref(&name),
                &lock,
                assets,
                ws::RenderOptions {
                    no_format: false,
                    checks: true,
                    version_root: &root,
                },
            )?;
            ensure!(
                ws::tracked_changes(&root)?.is_empty(),
                "{repo}/{name} does not match reviewed release inputs; merge generated changes first"
            );
            let directory = io::relative(&root, target.directory(&name))?;
            pending.push(Pending {
                temporary,
                root,
                directory,
                repo: repo.clone(),
                name,
                version,
                tag,
                commit,
                exists,
                notes,
                publish,
                probe,
            });
        }
    }
    let tags = pending
        .iter()
        .map(|p| (&p.repo, &p.tag))
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(
        tags.len() == pending.len(),
        "Targets in one repository need distinct release tags"
    );
    let mut results = Vec::new();
    for p in pending {
        if dry_run {
            results.push(json!({"target":p.name,"repository":p.repo,"version":p.version,"tag":p.tag,"commit":p.commit}));
            continue;
        }
        if !p.exists {
            io::git(
                &p.root,
                &[
                    "push",
                    "origin",
                    &format!("{}:refs/tags/{}", p.commit, p.tag),
                ],
            )?;
        }
        let mut env = assets.env();
        env.insert("PERSEID_VERSION".into(), p.version.clone());
        let status = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", &p.probe])
            .current_dir(&p.directory)
            .envs(&env)
            .status()?;
        match status.code() {
            Some(0) => (),
            Some(1) => {
                io::hooks(&[p.publish], &p.directory, &env)?;
                io::hooks(&[p.probe], &p.directory, &env)?;
            }
            _ => bail!(
                "Publication probe failed for {} ({status}); no publish attempted",
                p.name
            ),
        }
        let releases = io::text(Command::new("gh").args([
            "api",
            "--paginate",
            &format!("repos/{}/releases", p.repo),
            "--jq",
            ".[].tag_name",
        ]))?;
        if !releases.lines().any(|tag| tag == p.tag) {
            let notes = p.temporary.path().join("notes.md");
            io::write(&notes, p.notes.as_bytes())?;
            io::run(
                Command::new("gh")
                    .args([
                        "release",
                        "create",
                        &p.tag,
                        "--repo",
                        &p.repo,
                        "--verify-tag",
                        "--title",
                        &format!("{} {}", p.name, p.version),
                        "--notes-file",
                    ])
                    .arg(notes),
            )?;
        }
        results.push(json!({"target":p.name,"repository":p.repo,"version":p.version,"tag":p.tag,"published":true}));
    }
    Ok(results.into())
}
