use super::{assets::Assets, config::Project, io, source, workspace as ws};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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
pub fn stamp(project: &Project, name: &str, directory: &Path, assets: &Assets) -> Result<()> {
    let target = &project.config.targets[name];
    let version = &project.versions.versions[name];
    let mut env = assets.env();
    env.insert("PERSEID_VERSION".into(), version.clone());
    if let Some(release) = &target.release
        && !release.version_commands.is_empty()
    {
        return io::hooks(&release.version_commands, directory, &env);
    }
    match target.language(name) {
        "rust" if directory.join("Cargo.toml").exists() => {
            stamp_toml(&io::relative(directory, "Cargo.toml")?, "package", version)?
        }
        "python" if directory.join("pyproject.toml").exists() => stamp_toml(
            &io::relative(directory, "pyproject.toml")?,
            "project",
            version,
        )?,
        "typescript" if directory.join("package.json").exists() => {
            let path = io::relative(directory, "package.json")?;
            let mut value = io::read_json(&path)?;
            value["version"] = version.clone().into();
            io::write(&path, &io::json(&value)?)?;
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
            let version = project.versions.versions[&name].clone();
            let tag = config
                .tag
                .as_deref()
                .unwrap_or("{target}/v{version}")
                .replace("{target}", &name)
                .replace("{version}", &version);
            ensure!(!tag.contains(['{', '}']), "Unknown tag placeholder");
            io::text(Command::new("git").args(["check-ref-format", &format!("refs/tags/{tag}")]))?;
            let temporary = ws::delivery(project, &repository, std::slice::from_ref(&name))?;
            let root = temporary.path().join("repo");
            let reference = format!("refs/tags/{tag}");
            let exists =
                !io::git(&root, &["ls-remote", "--tags", "origin", &reference])?.is_empty();
            if exists {
                io::git(&root, &["fetch", "--quiet", "origin", &reference])?;
                io::git(&root, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
                io::files(&root, false)?;
            }
            let commit = io::git(&root, &["rev-parse", "HEAD"])?;
            ws::render(
                project,
                &root,
                std::slice::from_ref(&name),
                &lock,
                assets,
                false,
                true,
            )?;
            ensure!(
                ws::tracked_changes(&root)?.is_empty(),
                "{repo}/{name} does not match reviewed release inputs; merge generated changes first"
            );
            let notes = project
                .versions
                .notes
                .get(&name)
                .filter(|n| !n.is_empty())
                .context("No reviewed release notes")?
                .clone();
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
