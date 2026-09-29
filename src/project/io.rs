use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::{NamedTempFile, TempDir};

pub const IGNORED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".gradle",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".perseid-work",
    "build",
    "dist",
    "coverage",
    ".coverage",
    ".codegen-tmp",
    ".perseid-tmp",
];

pub fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub fn read_json(path: &Path) -> Result<serde_json::Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .context("invalid JSON")
}

/// Resolve only normal relative components, and never follow a symlink. This
/// applies to reads and writes, even in disposable delivery clones / dry-runs.
pub fn relative(root: &Path, path: impl AsRef<Path>) -> Result<PathBuf> {
    let mut result = root.to_path_buf();
    for component in path.as_ref().components() {
        match component {
            Component::CurDir => (),
            Component::Normal(part) => {
                result.push(part);
                if let Ok(meta) = fs::symlink_metadata(&result) {
                    ensure!(
                        !meta.file_type().is_symlink(),
                        "Symlink path is unsupported: {}",
                        result.display()
                    );
                }
            }
            _ => bail!(
                "Path must stay inside its repository without '..': {}",
                path.as_ref().display()
            ),
        }
    }
    Ok(result)
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "Refusing to write symlink {}",
            path.display()
        );
    }
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let parent = path.parent().context("missing parent directory")?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    if let Ok(meta) = fs::metadata(path) {
        temporary.as_file().set_permissions(meta.permissions())?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o644))?;
        }
    }
    temporary.persist(path)?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileState {
    pub digest: String,
    pub executable: bool,
}
pub type Snapshot = BTreeMap<PathBuf, FileState>;

pub fn files(root: &Path, ignore_build: bool) -> Result<Snapshot> {
    fn walk(
        root: &Path,
        directory: &Path,
        result: &mut Snapshot,
        ignore_build: bool,
    ) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || (ignore_build && IGNORED.iter().any(|ignored| name == *ignored)) {
                continue;
            }
            let meta = fs::symlink_metadata(&path)?;
            ensure!(
                !meta.file_type().is_symlink(),
                "Symlinks in SDK workspaces are unsupported: {}",
                path.display()
            );
            if meta.is_dir() {
                walk(root, &path, result, ignore_build)?;
            } else if meta.is_file() {
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                result.insert(
                    path.strip_prefix(root)?.to_owned(),
                    FileState {
                        digest: sha(&fs::read(&path)?),
                        executable,
                    },
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result, ignore_build)?;
    Ok(result)
}

pub fn copy_files(source: &Path, destination: &Path, snapshot: &Snapshot) -> Result<()> {
    for path in snapshot.keys() {
        let from = relative(source, path)?;
        let to = relative(destination, path)?;
        write(&to, &fs::read(&from)?)?;
        fs::set_permissions(to, fs::metadata(from)?.permissions())?;
    }
    Ok(())
}
pub fn stage(root: &Path) -> Result<TempDir> {
    let temp = tempfile::tempdir()?;
    copy_files(root, temp.path(), &files(root, true)?)?;
    Ok(temp)
}
pub fn changes(before: &Snapshot, after: &Snapshot) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .cloned()
        .collect()
}
pub fn unchanged(root: &Path, before: &Snapshot, paths: &[PathBuf]) -> Result<()> {
    let current = files(root, true)?;
    ensure!(
        paths.iter().all(|p| current.get(p) == before.get(p)),
        "SDK files changed during generation; refusing to overwrite concurrent edits"
    );
    Ok(())
}
pub fn apply(root: &Path, staged: &Path, before: &Snapshot, paths: &[PathBuf]) -> Result<()> {
    unchanged(root, before, paths)?;
    for path in paths {
        let from = relative(staged, path)?;
        let to = relative(root, path)?;
        if from.exists() {
            write(&to, &fs::read(&from)?)?;
            fs::set_permissions(to, fs::metadata(from)?.permissions())?;
        } else if to.exists() {
            fs::remove_file(to)?;
        }
    }
    Ok(())
}

pub fn output(command: &mut Command) -> Result<Output> {
    let result = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("starting {:?}", command.get_program()))?;
    ensure!(
        result.status.success(),
        "Command {:?} failed ({})",
        command.get_program(),
        result.status
    );
    Ok(result)
}
pub fn text(command: &mut Command) -> Result<String> {
    Ok(String::from_utf8(output(command)?.stdout)?
        .trim()
        .to_owned())
}
pub fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("starting {:?}", command.get_program()))?;
    ensure!(
        status.success(),
        "Command {:?} failed ({status})",
        command.get_program()
    );
    Ok(())
}
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    text(Command::new("git").args(args).current_dir(root))
}
pub fn hooks(commands: &[String], root: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    for command in commands {
        run(Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", command])
            .current_dir(root)
            .envs(env))?;
    }
    Ok(())
}
