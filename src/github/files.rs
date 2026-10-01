//! The files perseid keeps in the repository holding perseid.toml: sdks.yml, and the release
//! files of the SDKs it holds. `perseid init` writes them; the user commits them.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{Workflow, git, join, layout, relative, workflow};
use crate::{
    config::{self, Config, Source},
    scaffold::RELEASE_WORKFLOW,
};

const OWNED: &str = "# Written by `perseid";

/// Whether perseid wrote `text`, or there is none: perseid rewrites it then.
pub(super) fn owned(text: Option<&[u8]>) -> bool {
    text.is_none_or(|t| t.starts_with(OWNED.as_bytes()))
}

/// The files perseid.toml at `root` calls for, relative to the repository root `top`.
pub struct Expected {
    pub top: PathBuf,
    pub branch: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// What perseid wrote, or would have.
pub enum Written {
    Added(String),
    Updated(String),
    /// A workflow the user wrote, kept: the message says what it must do.
    Kept(String),
}

pub fn expected(config: &Config, root: &Path) -> Result<Expected> {
    let top = super::toplevel(root).unwrap_or_else(|_| root.to_owned());
    let dir = relative(root, &top).unwrap_or_default();
    let branch = default_branch(&top);
    let here = layout::origin_repo(root);
    let sdks = config.sdks(&[])?;
    let triggers = match config.source() {
        Source::File(spec) => vec![
            join(&dir, spec),
            join(&dir, config::FILE),
            layout::SDKS_WORKFLOW.to_owned(),
        ],
        Source::Url(_) => vec![join(&dir, config::FILE)],
    };
    let spec = match config.source() {
        Source::File(spec) => Some(join(&dir, spec)),
        Source::Url(_) => None,
    };
    let daily = here.clone().unwrap_or_else(|| config.name.clone());
    let yaml = workflow(&Workflow {
        branch: &branch,
        paths: &triggers,
        dir: &dir,
        daily: matches!(config.source(), Source::Url(_)).then_some(daily.as_str()),
        requires: spec.as_deref(),
    });
    let mut files = vec![(layout::SDKS_WORKFLOW.to_owned(), yaml.into_bytes())];
    if config.release != Some(false) {
        let local: Vec<&config::Sdk> = sdks.iter().filter(|s| s.local).collect();
        let read = |path: &str| Ok(std::fs::read_to_string(top.join(path)).ok());
        files.extend(crate::scaffold::release_scaffold(
            config, &local, &dir, &branch, read,
        )?);
    }
    Ok(Expected { top, branch, files })
}

/// Writes the files perseid.toml at `root` calls for, keeping workflows the user wrote.
pub fn write(config: &Config, root: &Path) -> Result<Vec<Written>> {
    let expected = expected(config, root)?;
    let mut written = Vec::new();
    for (path, content) in expected.files {
        let target = expected.top.join(&path);
        let current = std::fs::read(&target).ok();
        if current.as_deref() == Some(content.as_slice()) {
            continue;
        }
        let workflow = path == layout::SDKS_WORKFLOW || path == RELEASE_WORKFLOW;
        if workflow && !owned(current.as_deref()) {
            written.push(Written::Kept(kept(&path, &expected.branch)));
            continue;
        }
        crate::fsx::write(&target, &content)?;
        written.push(match current {
            Some(_) => Written::Updated(path),
            None => Written::Added(path),
        });
    }
    Ok(written)
}

/// Writes the release files of `sdks` into `top`, a checkout of the repository holding them, so
/// the SDK pull request carries them: the paths written. A release workflow the user wrote stays.
pub fn write_release_files(
    config: &Config,
    sdks: &[&config::Sdk],
    top: &Path,
) -> Result<Vec<PathBuf>> {
    let branch = default_branch(top);
    let read = |path: &str| Ok(std::fs::read_to_string(top.join(path)).ok());
    let mut written = Vec::new();
    for (path, content) in crate::scaffold::release_scaffold(config, sdks, "", &branch, read)? {
        let target = top.join(&path);
        let current = std::fs::read(&target).ok();
        if current.as_deref() == Some(content.as_slice()) {
            continue;
        }
        if path == RELEASE_WORKFLOW && !owned(current.as_deref()) {
            println!("! {}: {}", top.display(), kept(&path, &branch));
            continue;
        }
        crate::fsx::write(&target, &content)?;
        written.push(target);
    }
    Ok(written)
}

pub(super) fn kept(path: &str, branch: &str) -> String {
    match path == RELEASE_WORKFLOW {
        true => format!(
            "{path} is yours: check it runs release-please on `{branch}` and publishes from the `release` environment"
        ),
        false => format!(
            "{path} is yours: check it runs {} on `{branch}`",
            super::uses("")
        ),
    }
}

/// The branch the workflows run on: the remote's default branch, else the current one.
fn default_branch(top: &Path) -> String {
    let read = |args: &[&str]| {
        git(top, args)
            .ok()
            .and_then(|out| String::from_utf8(out).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    read(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .map(|b| b.trim_start_matches("origin/").to_owned())
        .or_else(|| read(&["symbolic-ref", "--short", "HEAD"]))
        .unwrap_or_else(|| "main".to_owned())
}
