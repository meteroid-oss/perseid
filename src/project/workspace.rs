use super::{
    assets::Assets,
    config::Project,
    io,
    source::{self, Lock},
};
use crate::runner::{self, GenerateOptions};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn checkout(
    project: &Project,
    repository: &str,
    names: &[String],
    destination: &Path,
) -> Result<()> {
    let mut command = Command::new("git");
    command.args(["clone", "--quiet", "--single-branch"]);
    if let Some(branch) = names
        .first()
        .and_then(|name| project.config.targets[name].branch.as_deref())
    {
        ensure!(!branch.starts_with('-'), "Invalid destination branch");
        command.args(["--branch", branch]);
    }
    command
        .arg("--")
        .arg(source::git_url(repository, &project.root)?)
        .arg(destination);
    io::run(&mut command)?;
    io::files(destination, false)?; // Validate clone before ANY generation/control writes.
    Ok(())
}
pub fn working_root(project: &Project, repository: &str, names: &[String]) -> Result<PathBuf> {
    if repository == "." {
        return Ok(project.root.clone());
    }
    let destination = io::relative(
        &project.root,
        format!(".perseid-work/{}", &io::sha(repository.as_bytes())[..16]),
    )?;
    if !destination.exists() {
        fs::create_dir_all(destination.parent().unwrap())?;
        checkout(project, repository, names, &destination)?;
    }
    Ok(destination)
}
fn extensions(project: &Project, root: &Path, name: &str) -> Result<String> {
    let target = &project.config.targets[name];
    let mut hashes = BTreeMap::new();
    let overrides = io::relative(root, ".perseid/overrides.toml")?;
    if overrides.exists() {
        hashes.insert("overrides.toml".into(), io::sha(&fs::read(overrides)?));
    }
    if let Some(directory) = &target.template_overrides {
        for (path, state) in io::files(&io::relative(root, directory)?, false)? {
            hashes.insert(format!("template/{}", path.display()), state.digest);
        }
    }
    for (name, path) in &target.runtime_overrides {
        hashes.insert(
            format!("runtime/{name}"),
            io::sha(&fs::read(io::relative(root, path)?)?),
        );
    }
    Ok(io::sha(&io::json(&hashes)?))
}
pub struct RenderOptions<'a> {
    pub no_format: bool,
    pub checks: bool,
    pub version_root: &'a Path,
}
pub fn render(
    project: &Project,
    root: &Path,
    names: &[String],
    lock: &Lock,
    assets: &Assets,
    options: RenderOptions<'_>,
) -> Result<()> {
    let effective = project.with_overrides(root, names)?;
    let project = &effective;
    let bytes = fs::read(project.snapshot()?)?;
    ensure!(
        io::sha(&bytes) == lock.spec_sha256,
        "Spec snapshot changed during generation"
    );
    let input = source::read_spec(&bytes)?;
    for name in names {
        let target = &project.config.targets[name];
        let directory = io::relative(root, target.directory(name))?;
        fs::create_dir_all(&directory)?;
        let resolved = super::release::resolve(project, root, options.version_root, name, assets)?;
        let config = project.generator_config(name, &resolved.version)?;
        let language = target.language(name);
        let lang = &config.languages[language];
        let manifest = format!(".perseid/manifests/{name}.json");
        let manifest_path = io::relative(root, &manifest)?;
        let mut output_paths = lang
            .task
            .iter()
            .map(|t| t.output_dir.clone())
            .collect::<Vec<_>>();
        output_paths.extend(lang.runtime_output_dir.clone());
        if manifest_path.exists() {
            let previous: BTreeMap<String, Vec<String>> =
                serde_json::from_value(io::read_json(&manifest_path)?)?;
            if let Some(paths) = previous.get(language) {
                output_paths.extend(paths.iter().cloned());
            }
        }
        for path in output_paths {
            ensure!(
                io::relative(root, path)?.starts_with(&directory),
                "Target {name} output must stay within {}",
                target.directory(name)
            );
        }
        let extension_hash = extensions(project, root, name)?;
        runner::generate(
            &config,
            root,
            assets,
            GenerateOptions {
                languages: &[language.into()],
                inputs: Some(std::slice::from_ref(&input)),
                manifest: &manifest,
                coverage_file: &format!(".perseid/manifests/{name}.coverage.json"),
            },
        )?;
        if project.config.release.is_some()
            || target.release.is_some()
            || project.versions_path().exists()
            || !project.independent()
        {
            super::release::stamp(project, name, &directory, assets, &resolved.version)?;
        }
        let mut env = assets.env();
        env.insert(
            "PERSEID_REPOSITORY_ROOT".into(),
            root.to_string_lossy().into_owned(),
        );
        env.insert(
            "PERSEID_GENERATED_MANIFEST".into(),
            manifest_path.to_string_lossy().into_owned(),
        );
        if !options.no_format {
            if let Some(commands) = &target.format_commands {
                io::hooks(commands, &directory, &env)?;
            } else {
                crate::formatting::format(language, root, &directory, &manifest_path)?;
            }
        }
        if options.checks {
            io::hooks(&target.check_commands, &directory, &env)?;
        }
        ensure!(
            extensions(project, root, name)? == extension_hash,
            "Generation hooks changed target extensions; rerun with stable inputs"
        );
        super::release::record(root, name, &resolved)?;
        let receipt = json!({"lock":lock,"target":name,"version":resolved.version,"extensions_sha256":extension_hash});
        io::write(
            &io::relative(root, format!(".perseid/receipts/{name}.json"))?,
            &io::json(&receipt)?,
        )?;
    }
    Ok(())
}
pub fn generate(
    project: &Project,
    names: &[String],
    assets: &Assets,
    check: bool,
    no_format: bool,
    checks: bool,
) -> Result<Value> {
    let lock = source::locked(project, assets)?;
    let mut pending = Vec::new();
    let mut results = Vec::new();
    for (repository, selected) in project.groups(names)? {
        let root = working_root(project, &repository, &selected)?;
        let before = io::files(&root, true)?;
        let stage = io::stage(&root)?;
        render(
            project,
            stage.path(),
            &selected,
            &lock,
            assets,
            RenderOptions {
                no_format,
                checks: checks || check,
                version_root: &root,
            },
        )?;
        let changes = io::changes(&before, &io::files(stage.path(), true)?);
        results.push(
            json!({"repository":repository,"directory":root,"targets":selected,"changed":changes}),
        );
        pending.push((root, stage, before, changes));
    }
    if !check {
        for (root, _, before, changes) in &pending {
            io::unchanged(root, before, changes)?;
        }
        for (root, stage, before, changes) in pending {
            io::apply(&root, stage.path(), &before, &changes)?;
        }
    }
    Ok(results.into())
}
pub fn has_changes(result: &Value) -> bool {
    result.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["changed"]
                .as_array()
                .is_some_and(|paths| !paths.is_empty())
        })
    })
}
pub fn tracked_changes(root: &Path) -> Result<Vec<String>> {
    io::git(root, &["add", "--all"])?;
    Ok(io::git(root, &["diff", "--cached", "--name-only"])?
        .lines()
        .map(str::to_owned)
        .collect())
}
pub fn branch(root: &Path) -> Result<String> {
    let branch = io::git(root, &["branch", "--show-current"])?;
    ensure!(
        !branch.is_empty(),
        "Destination must name a branch, not a tag"
    );
    Ok(branch)
}
pub fn repository_url(project: &Project, repository: &str) -> Result<String> {
    if repository == "." {
        io::git(&project.root, &["config", "--get", "remote.origin.url"])
    } else {
        source::git_url(repository, &project.root)
    }
}
pub fn github_repo(url: &str) -> Result<String> {
    let repository = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .context("GitHub delivery requires a github.com repository")?
        .trim_end_matches('/')
        .trim_end_matches(".git");
    ensure!(
        repository.split('/').count() == 2
            && repository
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c)),
        "Invalid GitHub repository"
    );
    Ok(repository.into())
}
pub fn delivery(
    project: &Project,
    repository: &str,
    names: &[String],
) -> Result<tempfile::TempDir> {
    let temp = tempfile::tempdir()?;
    checkout(
        project,
        &repository_url(project, repository)?,
        names,
        &temp.path().join("repo"),
    )?;
    Ok(temp)
}
