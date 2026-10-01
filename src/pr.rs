use std::{
    collections::BTreeSet,
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

/// Where the generated spec comes from, for pull request descriptions: the commit that pushed
/// it here, or the URL and a digest of what it served.
pub fn origin(config: &crate::config::Config, root: &Path, spec: &str) -> Option<String> {
    use sha2::Digest;
    if let crate::config::Source::Url(url) = config.source() {
        let digest = sha2::Sha256::digest(spec.as_bytes());
        let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        return Some(format!("<{url}> (sha256 `{hex}`)"));
    }
    let text = std::fs::read_to_string(root.join(SOURCE)).ok()?;
    let source: serde_json::Value = serde_json::from_str(&text).ok()?;
    let sha = source["sha"].as_str()?;
    let short = sha.get(..7)?;
    let tag = source["ref"].as_str();
    let Some(repo) = source["repo"].as_str() else {
        return Some(match tag {
            Some(tag) => format!("{tag} (`{short}`)"),
            None => format!("`{short}`"),
        });
    };
    let commit = format!("https://github.com/{repo}/commit/{sha}");
    Some(match tag {
        Some(tag) => format!(
            "[{repo}@{tag}](https://github.com/{repo}/releases/tag/{tag}) ([{short}]({commit}))"
        ),
        None => format!("[{repo}@{short}]({commit})"),
    })
}

/// Where `perseid connect`'s workflow records the commit it pushed the spec of.
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

const BOT_NAME: &str = "github-actions[bot]";
const BOT_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";

/// A detached worktree of `repo`, removed on drop, so the user's checkout is never touched.
struct Worktree {
    repo: PathBuf,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Worktree {
    fn add(repo: &Path, commit: &str) -> Result<Self> {
        let dir = tempfile::Builder::new().prefix("perseid-pr-").tempdir()?;
        let path = dir.path().join("tree");
        let target = path.to_str().context("non UTF-8 temporary path")?;
        git(
            repo,
            &["worktree", "add", "--quiet", "--detach", target, commit],
        )?;
        Ok(Worktree {
            repo: repo.to_owned(),
            path,
            _dir: dir,
        })
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if let Some(path) = self.path.to_str() {
            let _ = git(&self.repo, &["worktree", "remove", "--force", path]);
        }
    }
}

/// Files under `paths` that `ls-files` lists with `args` and carry perseid's marker.
fn generated(dir: &Path, args: &[&str], paths: &[String]) -> Result<BTreeSet<String>> {
    let mut ls = vec!["ls-files", "-z"];
    ls.extend(args);
    ls.push("--");
    ls.extend(paths.iter().map(String::as_str));
    Ok(git(dir, &ls)?
        .split('\0')
        .filter(|p| {
            let path = dir.join(p);
            !p.is_empty()
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| crate::generate::marked(&path, &text))
        })
        .map(str::to_owned)
        .collect())
}

/// Brings the generated files under `paths`, and `files`, from the checkout `from` to the
/// worktree `to`. Other files under `paths` keep their content in `to`.
fn mirror(from: &Path, to: &Path, paths: &[String], files: &[String]) -> Result<()> {
    let ours = generated(from, &["--cached", "--others", "--exclude-standard"], paths)?;
    for stale in generated(to, &[], paths)?.difference(&ours) {
        std::fs::remove_file(to.join(stale))?;
    }
    for path in ours.iter().chain(files) {
        let target = to.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from.join(path), &target).with_context(|| format!("copying {path}"))?;
    }
    Ok(())
}

/// Commits the generated files under `paths`, and `files`, of the repository at `dir` on top of
/// its upstream branch to the update branch, and opens (or refreshes) its PR. The commit is made
/// in a temporary worktree: the checkout at `dir` keeps its branch, index and files.
/// An open PR keeps its bump if larger: it releases every spec change since the last merge.
pub fn open(
    dir: &Path,
    paths: &[String],
    files: &[String],
    bump: Bump,
    subject: &str,
    body: &str,
) -> Result<Option<String>> {
    let shallow = git(dir, &["rev-parse", "--is-shallow-repository"])? == "true";
    let fetch = |refspec: &str| {
        let mut args = vec!["fetch", "--quiet"];
        if shallow {
            args.extend(["--depth", "1"]);
        }
        args.extend(["origin", refspec]);
        git(dir, &args)?;
        git(dir, &["rev-parse", "FETCH_HEAD"])
    };
    let branch = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let (base, start) = match branch.map(|b| fetch(&b).map(|sha| (b, sha))) {
        Some(Ok((branch, sha))) => (Some(branch), sha),
        _ => (
            None,
            fetch("HEAD").context("fetching the default branch of `origin`")?,
        ),
    };
    let tree = Worktree::add(dir, &start)?;
    let work = tree.path.as_path();
    mirror(dir, work, paths, files)?;
    let mut add = vec!["add", "--all", "--"];
    add.extend(paths.iter().chain(files).map(String::as_str));
    git(work, &add)?;
    if git(work, &["status", "--porcelain"])?.is_empty() {
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
    let name = format!("user.name={BOT_NAME}");
    let email = format!("user.email={BOT_EMAIL}");
    let mut commit = vec![];
    if git(dir, &["config", "user.name"]).is_err() {
        commit.extend(["-c", &name]);
    }
    if git(dir, &["config", "user.email"]).is_err() {
        commit.extend(["-c", &email]);
    }
    commit.extend(["commit", "--quiet", "-m", &title]);
    git(work, &commit)?;
    let head = git(work, &["rev-parse", "HEAD"])?;
    drop(tree);
    let tree_of = |sha: &str| git(dir, &["rev-parse", &format!("{sha}^{{tree}}")]);
    let unchanged = fetch(BRANCH).is_ok_and(|sha| tree_of(&sha).ok() == tree_of(&head).ok());
    if !unchanged {
        let refspec = format!("{head}:refs/heads/{BRANCH}");
        git(dir, &["push", "--quiet", "--force", "origin", &refspec])?;
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
    if let Some(base) = &base {
        create.extend(["--base", base]);
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
        let local = config("openapi.json");
        assert_eq!(origin(&local, dir.path(), "{}"), None);
        let source = "{ \"repo\": \"acme/api\", \"sha\": \"a1b2c3d4e5\" }";
        crate::fsx::write(&dir.path().join(SOURCE), source.as_bytes()).unwrap();
        assert_eq!(
            origin(&local, dir.path(), "{}").as_deref(),
            Some("[acme/api@a1b2c3d](https://github.com/acme/api/commit/a1b2c3d4e5)")
        );
        assert_eq!(
            origin(&config("https://acme.dev/openapi.json"), dir.path(), "{}").as_deref(),
            Some("<https://acme.dev/openapi.json> (sha256 `44136fa355b3`)")
        );
        let released = "{ \"repo\": \"acme/api\", \"sha\": \"a1b2c3d4e5\", \"ref\": \"v1.4.0\" }";
        crate::fsx::write(&dir.path().join(SOURCE), released.as_bytes()).unwrap();
        assert_eq!(
            origin(&local, dir.path(), "{}").as_deref(),
            Some(
                "[acme/api@v1.4.0](https://github.com/acme/api/releases/tag/v1.4.0) ([a1b2c3d](https://github.com/acme/api/commit/a1b2c3d4e5))"
            )
        );
        let private = "{ \"sha\": \"a1b2c3d4e5\", \"ref\": \"v1.4.0\" }";
        crate::fsx::write(&dir.path().join(SOURCE), private.as_bytes()).unwrap();
        assert_eq!(
            origin(&local, dir.path(), "{}").as_deref(),
            Some("v1.4.0 (`a1b2c3d`)")
        );
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
