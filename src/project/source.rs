use super::{assets::Assets, config::Project, io};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::Read, path::Path, process::Command, time::Duration};
const MAX_SPEC: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lock {
    pub schema: u32,
    pub perseid_version: String,
    pub engine_sha256: String,
    pub config_sha256: String,
    pub source: Value,
    pub snapshot: String,
    pub spec_sha256: String,
}

pub fn git_url(repository: &str, root: &Path) -> Result<String> {
    ensure!(!repository.starts_with('-'), "Invalid Git repository");
    let local = root.join(repository);
    if local.exists() {
        return Ok(fs::canonicalize(local)?.to_string_lossy().into_owned());
    }
    if repository.contains("://") || repository.starts_with("git@") {
        return Ok(repository.into());
    }
    ensure!(
        repository.split('/').count() == 2,
        "Expected Git URL, owner/repository, or an existing local repository"
    );
    Ok(format!("https://github.com/{repository}.git"))
}
pub fn read_spec(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= MAX_SPEC, "Spec exceeds 64 MiB");
    let value: Value = serde_json::from_slice(bytes)?;
    ensure!(
        value
            .get("openapi")
            .and_then(Value::as_str)
            .is_some_and(|v| v.starts_with("3."))
            && value.get("info").is_some_and(Value::is_object)
            && value.get("paths").is_some_and(Value::is_object),
        "Expected an OpenAPI 3 JSON document with info and paths"
    );
    Ok(value)
}
pub fn sync(project: &Project, assets: &Assets) -> Result<Value> {
    let source = &project.config.source;
    let (bytes, resolved) = if let Some(file) = &source.file {
        (
            fs::read(project.root.join(file))?,
            json!({"kind":"file", "file":file}),
        )
    } else if let Some(repository) = &source.git {
        let path = source.path.as_ref().context("Git source requires path")?;
        io::relative(Path::new("/source"), path)?;
        ensure!(!path.contains(':'), "Invalid source path");
        let reference = source.r#ref.as_deref().unwrap_or("HEAD");
        ensure!(!reference.starts_with('-'), "Invalid source ref");
        let temp = tempfile::tempdir()?;
        io::git(temp.path(), &["init", "--quiet"])?;
        io::git(
            temp.path(),
            &[
                "fetch",
                "--quiet",
                "--depth=1",
                &git_url(repository, &project.root)?,
                reference,
            ],
        )?;
        let commit = io::git(temp.path(), &["rev-parse", "FETCH_HEAD"])?;
        let bytes = io::output(
            Command::new("git")
                .args(["show", &format!("{commit}:{path}")])
                .current_dir(temp.path()),
        )?
        .stdout;
        (
            bytes,
            json!({"kind":"git", "repository":repository, "commit":commit,"path":path}),
        )
    } else if let Some(url) = &source.url {
        ensure!(
            url.starts_with("https://") || url.starts_with("http://"),
            "Spec URL must use HTTP(S)"
        );
        ensure!(
            !url.split('/').nth(2).unwrap_or("").contains('@'),
            "Spec URL cannot contain embedded credentials"
        );
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        let mut response = agent.get(url).call()?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((MAX_SPEC + 1) as u64)
            .read_to_end(&mut bytes)?;
        (bytes, json!({"kind":"url","url":url}))
    } else {
        let command = source.command.as_ref().context("Missing source")?;
        let bytes = io::output(
            Command::new(&command[0])
                .args(&command[1..])
                .current_dir(&project.root),
        )?
        .stdout;
        (bytes, json!({"kind":"command","command":command}))
    };
    read_spec(&bytes)?;
    let lock = Lock {
        schema: 2,
        perseid_version: env!("CARGO_PKG_VERSION").into(),
        engine_sha256: assets.fingerprint.clone(),
        config_sha256: project.fingerprint()?,
        source: resolved,
        snapshot: source.snapshot.clone(),
        spec_sha256: io::sha(&bytes),
    };
    io::write(&project.snapshot()?, &bytes)?;
    io::write(
        &io::relative(&project.root, "perseid.lock.json")?,
        &io::json(&lock)?,
    )?;
    Ok(serde_json::to_value(lock)?)
}
pub fn locked(project: &Project, assets: &Assets) -> Result<Lock> {
    let lock: Lock = serde_json::from_value(
        io::read_json(&io::relative(&project.root, "perseid.lock.json")?)
            .context("No spec snapshot; run `perseid sync`")?,
    )?;
    ensure!(
        lock.schema == 2
            && lock.perseid_version == env!("CARGO_PKG_VERSION")
            && lock.engine_sha256 == assets.fingerprint
            && lock.config_sha256 == project.fingerprint()?
            && lock.snapshot == project.config.source.snapshot
            && lock.spec_sha256 == io::sha(&fs::read(project.snapshot()?)?),
        "Spec, configuration, or generator changed; run `perseid sync` to refresh the lock"
    );
    Ok(lock)
}
