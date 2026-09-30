use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub const BRANCH: &str = "perseid/update";

/// Semver bump requested from release tooling through the conventional-commit type of the PR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn title(self, subject: &str) -> String {
        let kind = match self {
            Bump::Patch => "fix(api)",
            Bump::Minor => "feat(api)",
            Bump::Major => "feat(api)!",
        };
        format!("{kind}: {subject}")
    }

    pub fn of_title(title: &str) -> Option<Self> {
        let (kind, _) = title.split_once(':')?;
        match kind {
            _ if kind.ends_with('!') => Some(Bump::Major),
            _ if kind.starts_with("feat") => Some(Bump::Minor),
            _ if kind.starts_with("fix") => Some(Bump::Patch),
            _ => None,
        }
    }
}

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

/// Where the generated spec comes from, for pull request descriptions: the commit of the
/// repository that pushed it, or the URL and a digest of what it served.
pub fn origin(config: &crate::config::Config, root: &Path, spec: &str) -> Option<String> {
    use sha2::Digest;
    match config.source() {
        crate::config::Source::GitHub { .. } => {
            let text = std::fs::read_to_string(root.join(SOURCE)).ok()?;
            let source: serde_json::Value = serde_json::from_str(&text).ok()?;
            let (repo, sha) = (source["repo"].as_str()?, source["sha"].as_str()?);
            let short = sha.get(..7)?;
            Some(format!(
                "[{repo}@{short}](https://github.com/{repo}/commit/{sha})"
            ))
        }
        crate::config::Source::Url(url) => {
            let digest = sha2::Sha256::digest(spec.as_bytes());
            let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
            Some(format!("<{url}> (sha256 `{hex}`)"))
        }
        crate::config::Source::File(_) => None,
    }
}

/// Where a repository receiving the spec records which commit it comes from.
pub const SOURCE: &str = ".perseid/source.json";

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
/// An open PR keeps its bump if larger: it releases every spec change since the last merge.
pub fn open(
    dir: &Path,
    paths: &[String],
    bump: Bump,
    subject: &str,
    body: &str,
) -> Result<Option<String>> {
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(paths.iter().map(String::as_str));
    if git(dir, &status)?.is_empty() {
        return Ok(None);
    }
    let existing = run(
        dir,
        "gh",
        &[
            "pr",
            "list",
            "--head",
            BRANCH,
            "--state",
            "open",
            "--json",
            "url,title",
            "--jq",
            r#".[0] | select(.) | .url + "\t" + .title"#,
        ],
    )?;
    let (existing, previous) = existing.split_once('\t').unwrap_or_default();
    let title = bump
        .max(Bump::of_title(previous).unwrap_or(bump))
        .title(subject);
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
    commit.extend(["commit", "--quiet", "-m", &title]);
    git(dir, &commit)?;
    let unchanged = git(dir, &["fetch", "--quiet", "--depth", "1", "origin", BRANCH]).is_ok()
        && git(dir, &["rev-parse", "HEAD^{tree}"])?
            == git(dir, &["rev-parse", "FETCH_HEAD^{tree}"])?;
    if !unchanged {
        git(dir, &["push", "--quiet", "--force", "origin", BRANCH])?;
    }
    if !existing.is_empty() {
        run(
            dir,
            "gh",
            &["pr", "edit", BRANCH, "--title", &title, "--body", body],
        )?;
        return Ok(Some(existing.to_owned()));
    }
    let mut create = vec![
        "pr", "create", "--head", BRANCH, "--title", &title, "--body", body,
    ];
    if base != "HEAD" {
        create.extend(["--base", &base]);
    }
    run(dir, "gh", &create).map(Some)
}

#[cfg(test)]
mod tests {
    use super::{Bump, SOURCE, origin};

    #[test]
    fn pull_requests_name_where_the_spec_comes_from() {
        let dir = tempfile::tempdir().unwrap();
        let config = |spec: &str| -> crate::config::Config {
            toml::from_str(&format!("spec = \"{spec}\"\nname = \"Acme\"\n")).unwrap()
        };
        let source =
            "{ \"repo\": \"acme/api\", \"path\": \"openapi.json\", \"sha\": \"a1b2c3d4e5\" }";
        crate::fsx::write(&dir.path().join(SOURCE), source.as_bytes()).unwrap();
        assert_eq!(
            origin(&config("github:acme/api/openapi.json"), dir.path(), "{}").as_deref(),
            Some("[acme/api@a1b2c3d](https://github.com/acme/api/commit/a1b2c3d4e5)")
        );
        assert_eq!(
            origin(&config("https://acme.dev/openapi.json"), dir.path(), "{}").as_deref(),
            Some("<https://acme.dev/openapi.json> (sha256 `44136fa355b3`)")
        );
        assert_eq!(origin(&config("openapi.json"), dir.path(), "{}"), None);
    }

    #[test]
    fn bump_round_trips_through_conventional_titles() {
        for bump in [Bump::Patch, Bump::Minor, Bump::Major] {
            assert_eq!(Bump::of_title(&bump.title("update SDKs")), Some(bump));
        }
        assert_eq!(Bump::title(Bump::Major, "x"), "feat(api)!: x");
        assert_eq!(Bump::of_title("fix!: drop field"), Some(Bump::Major));
        assert_eq!(Bump::of_title("Update SDKs to Acme 1.0"), None);
        assert_eq!(Bump::of_title(""), None);
    }
}
