use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub const BRANCH: &str = "perseid/update";

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("running `{program}` (is it installed?)"))?;
    if !output.status.success() {
        bail!(
            "`{program} {}` failed in {}:\n{}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, "git", args)
}

/// A fresh checkout of the default branch of `repo`, in a disposable directory under `.perseid/`.
pub fn checkout(repo: &str, root: &Path) -> Result<PathBuf> {
    let url = if repo.contains(':') {
        repo.to_owned()
    } else {
        format!("https://github.com/{repo}.git")
    };
    let segments: Vec<_> = repo
        .trim_end_matches(".git")
        .split(['/', ':'])
        .filter(|s| !s.is_empty())
        .collect();
    let repos = root.join(".perseid/repos");
    crate::fsx::write(&repos.join(".gitignore"), b"*\n")?;
    let dir = repos.join(segments[segments.len().saturating_sub(2)..].join("/"));
    if dir.join(".git").is_dir() {
        git(
            &dir,
            &["fetch", "--quiet", "--depth", "1", "origin", "HEAD"],
        )?;
        git(
            &dir,
            &["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"],
        )?;
        git(&dir, &["clean", "--quiet", "-fd"])?;
    } else {
        std::fs::create_dir_all(&dir)?;
        git(&dir, &["clone", "--quiet", "--depth", "1", &url, "."])?;
    }
    Ok(dir)
}

pub fn toplevel(dir: &Path) -> Result<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .context("`--pr` needs the SDK to live in a git repository")
}

/// Commits `paths` of the repository at `dir` to the update branch and opens (or refreshes) its PR.
pub fn open(dir: &Path, paths: &[String], title: &str, body: &str) -> Result<Option<String>> {
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(paths.iter().map(String::as_str));
    if git(dir, &status)?.is_empty() {
        return Ok(None);
    }
    let base = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    git(dir, &["switch", "--quiet", "-C", BRANCH])?;
    let mut add = vec!["add", "--all", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(dir, &add)?;
    let identity = git(dir, &["config", "user.email"]).is_ok();
    let mut commit = vec![];
    if !identity {
        commit.extend([
            "-c",
            "user.name=perseid[bot]",
            "-c",
            "user.email=perseid@users.noreply.github.com",
        ]);
    }
    commit.extend(["commit", "--quiet", "-m", title]);
    git(dir, &commit)?;
    let unchanged = git(dir, &["fetch", "--quiet", "--depth", "1", "origin", BRANCH]).is_ok()
        && git(dir, &["rev-parse", "HEAD^{tree}"])?
            == git(dir, &["rev-parse", "FETCH_HEAD^{tree}"])?;
    if !unchanged {
        git(dir, &["push", "--quiet", "--force", "origin", BRANCH])?;
    }
    let existing = run(
        dir,
        "gh",
        &[
            "pr", "list", "--head", BRANCH, "--state", "open", "--json", "url", "--jq", ".[0].url",
        ],
    )?;
    if !existing.is_empty() {
        run(
            dir,
            "gh",
            &["pr", "edit", BRANCH, "--title", title, "--body", body],
        )?;
        return Ok(Some(existing));
    }
    let mut create = vec![
        "pr", "create", "--head", BRANCH, "--title", title, "--body", body,
    ];
    if base != "HEAD" {
        create.extend(["--base", &base]);
    }
    run(dir, "gh", &create).map(Some)
}
