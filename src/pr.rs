use std::{
    cell::{Cell, OnceCell},
    collections::BTreeSet,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use crate::{
    changelog::Changelog,
    github::{
        Options, TOKEN, Ui,
        api::{GitHub, check, expiration_epoch},
        auth,
    },
};

pub const BRANCH: &str = "perseid/update";

/// The update branch of the SDK `name` in a repository holding several, when its update is
/// split: one pull request each.
pub fn branch_of(name: &str) -> String {
    format!("{BRANCH}-{name}")
}

/// Whether `branch` is an update branch perseid pushes.
pub fn is_update_branch(branch: &str) -> bool {
    branch == BRANCH || is_sdk_branch(branch)
}

fn is_sdk_branch(branch: &str) -> bool {
    branch.starts_with(&format!("{BRANCH}-"))
}

/// The most files an update of several SDKs changes in one pull request. release-please finds the
/// SDKs a commit changed from its files, of which GitHub lists the first 3000: past that, it
/// misses the last SDKs. The margin covers the base branch moving before the merge.
pub fn split_above() -> usize {
    std::env::var("PERSEID_SPLIT_ABOVE")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2500)
}

/// Marks pull requests whose release PR `meteroid-oss/perseid/release` auto-merges.
pub const AUTO_RELEASE: &str = "perseid:auto-release";

/// Semver bump requested from release tooling through the conventional-commit type of the PR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Bump {
    /// Sized by oasdiff, comparing the spec with its previous version.
    Auto,
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn title(self, subject: &str) -> String {
        let kind = match self {
            Bump::Patch => "fix(api)",
            Bump::Minor | Bump::Auto => "feat(api)",
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

fn run(dir: &Path, args: &[&str], config: &[(&str, String)]) -> Result<String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    if !config.is_empty() {
        let count: usize = std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        for (i, (key, value)) in config.iter().enumerate() {
            command.env(format!("GIT_CONFIG_KEY_{}", count + i), key);
            command.env(format!("GIT_CONFIG_VALUE_{}", count + i), value);
        }
        command.env("GIT_CONFIG_COUNT", (count + config.len()).to_string());
    }
    let output = command
        .output()
        .context("running `git` (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "`git {}` failed in {}:\n{}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, args, &[])
}

/// `git` reaching github.com with the token of the environment, when there is one, else with
/// git's own credentials.
fn remote_git(dir: &Path, args: &[&str]) -> Result<String> {
    let config = auth::env_token().map(|(token, _)| token_config(&token));
    run(dir, args, config.as_ref().map_or(&[], |c| &c[..]))
}

/// The extra headers of git, reset (actions/checkout persists one of its token, and two
/// `Authorization` headers fail) then holding `token`. As environment, out of logs.
fn token_config(token: &str) -> [(&'static str, String); 2] {
    const KEY: &str = "http.https://github.com/.extraheader";
    let basic = BASE64.encode(format!("x-access-token:{token}"));
    [
        (KEY, String::new()),
        (KEY, format!("AUTHORIZATION: basic {basic}")),
    ]
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

/// Where a checkout keeps the tree perseid last generated in it.
const GENERATED: &str = ".git/perseid-generated";

/// The tree of the files in the worktree of `dir`, untracked ones included, its index untouched.
fn worktree_tree(dir: &Path) -> Result<String> {
    let index = dir.join(".git/perseid-index");
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_INDEX_FILE", &index)
            .output()?;
        ensure!(
            output.status.success(),
            "`git {}` failed in {}:\n{}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let tree = git(&["add", "--all"]).and_then(|_| git(&["write-tree"]));
    let _ = std::fs::remove_file(&index);
    tree
}

/// Records what perseid generated in the checkout at `dir`, which the next run may then reset.
pub fn record(dir: &Path) -> Result<()> {
    crate::fsx::write(&dir.join(GENERATED), worktree_tree(dir)?.as_bytes())
}

/// A fresh checkout of the default branch of `repo`, in a disposable directory under `.perseid/`.
/// Changes there that perseid didn't generate stop it, unless `discard`.
pub fn checkout(repo: &str, root: &Path, discard: bool) -> Result<PathBuf> {
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
        let pristine = || -> Result<bool> {
            let tree = worktree_tree(&dir)?;
            let generated = std::fs::read_to_string(dir.join(GENERATED)).unwrap_or_default();
            Ok(tree == generated || tree == git(&dir, &["rev-parse", "HEAD^{tree}"])?)
        };
        ensure!(
            discard || pristine()?,
            "{} has local changes, which `perseid generate` would discard: commit and push them, \
             or delete that directory (`--pr` discards them)",
            dir.display()
        );
        remote_git(
            &dir,
            &["fetch", "--quiet", "--depth", "1", "origin", "HEAD"],
        )?;
        git(
            &dir,
            &["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"],
        )?;
        git(&dir, &["clean", "--quiet", "-fd"])?;
    } else {
        let parent = dir.parent().unwrap_or(&repos);
        std::fs::create_dir_all(parent)?;
        let target = dir.to_string_lossy();
        let clone = ["clone", "--quiet", "--depth", "1", &url, &target];
        if let Err(error) = remote_git(&repos, &clone) {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir(parent);
            let message = format!("{error:#}").to_lowercase();
            let missing = ["not found", "does not appear to be a git repository"];
            ensure!(
                !missing.iter().any(|m| message.contains(m)),
                "{repo} doesn't exist yet: create it on GitHub, or preview with \
                 `perseid generate --out <dir>`"
            );
            return Err(error);
        }
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
    let ours = match paths.is_empty() {
        // `ls-files` without paths would list the whole repository.
        true => BTreeSet::new(),
        false => generated(from, &["--cached", "--others", "--exclude-standard"], paths)?,
    };
    if !paths.is_empty() {
        for stale in generated(to, &[], paths)?.difference(&ours) {
            std::fs::remove_file(to.join(stale))?;
        }
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

/// GitHub's REST API, authenticated on first use.
#[derive(Default)]
pub struct Client {
    api: OnceCell<GitHub>,
    expiration_checked: Cell<bool>,
}

impl Client {
    fn api(&self) -> Result<&GitHub> {
        if let Some(api) = self.api.get() {
            return Ok(api);
        }
        let api = GitHub::new(Some(token()?));
        Ok(self.api.get_or_init(|| api))
    }

    /// Warns, once, when the token expires within 30 days.
    fn check_expiration(&self, api: &GitHub) {
        if self.expiration_checked.replace(true) {
            return;
        }
        let Some(date) = api.expiration() else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        if expiration_epoch(&date).is_none_or(|expires| expires - now > 30 * 86400) {
            return;
        }
        let day = date.split_whitespace().next().unwrap_or(&date);
        warn(&format!(
            "the GitHub token expires on {day}: renew the {TOKEN} secret, or use a GitHub App \
             (`perseid app`)"
        ));
    }
}

/// `GH_TOKEN`, `GITHUB_TOKEN`, gh's token or, in a terminal, a browser login.
fn token() -> Result<String> {
    if let Some((token, _)) = auth::stored_token() {
        return Ok(token);
    }
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    ensure!(
        env("GITHUB_ACTIONS") != "true",
        "no token to open the SDK pull requests: run `perseid sync` to install the perseid App \
         (the job needs `permissions: id-token: write`), or `perseid app` for an App of your own, \
         or add the {TOKEN} secret (a fine-grained token with Contents and Pull requests read and \
         write on the SDK repositories)"
    );
    ensure!(
        std::io::stdin().is_terminal() && env("CI").is_empty(),
        "no token to open the pull requests: set GH_TOKEN to a GitHub token with Contents and \
         Pull requests write on their repositories"
    );
    let ui = Ui::new(&Options {
        yes: false,
        dry_run: false,
        browser: true,
    });
    auth::token(&ui).map(|(token, _)| token)
}

/// An open pull request of an update branch.
pub struct Pull {
    pub url: String,
    repo: String,
    branch: String,
    number: u64,
    node_id: String,
}

impl Pull {
    fn new(repo: &str, branch: &str, pull: &Value) -> Result<Self> {
        Ok(Self {
            url: pull["html_url"].as_str().unwrap_or_default().to_owned(),
            repo: repo.to_owned(),
            branch: branch.to_owned(),
            number: pull["number"]
                .as_u64()
                .context("GitHub answered a pull request without a number")?,
            node_id: pull["node_id"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// `owner/name` of a remote URL: its last two segments.
fn repo_of(remote: &str) -> Option<String> {
    let path = remote.trim_end_matches('/').trim_end_matches(".git");
    let mut segments = path.rsplit(['/', ':']).filter(|s| !s.is_empty());
    let (name, owner) = (segments.next()?, segments.next()?);
    Some(format!("{owner}/{name}"))
}

fn default_branch(dir: &Path) -> Result<String> {
    let head = remote_git(dir, &["ls-remote", "--symref", "origin", "HEAD"])?;
    head.lines()
        .find_map(|l| l.strip_prefix("ref: refs/heads/")?.split_once('\t'))
        .map(|(branch, _)| branch.to_owned())
        .context("finding the default branch of `origin`")
}

fn warn(message: &str) {
    match std::env::var("GITHUB_ACTIONS").as_deref() {
        Ok("true") => println!("::warning::{message}"),
        _ => eprintln!("warning: {message}"),
    }
}

/// Fetches `refspec` from `origin`, as shallow as the checkout at `dir`: its commit.
fn fetch(dir: &Path, refspec: &str) -> Result<String> {
    let mut args = vec!["fetch", "--quiet"];
    if git(dir, &["rev-parse", "--is-shallow-repository"])? == "true" {
        args.extend(["--depth", "1"]);
    }
    args.extend(["origin", refspec]);
    remote_git(dir, &args)?;
    git(dir, &["rev-parse", "FETCH_HEAD"])
}

/// The upstream branch of the checkout at `dir`, when it is on one, and the commit to build on:
/// its tip, else that of the default branch.
fn upstream(dir: &Path) -> Result<(Option<String>, String)> {
    let branch = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    match branch.map(|b| fetch(dir, &b).map(|sha| (b, sha))) {
        Some(Ok((branch, sha))) => Ok((Some(branch), sha)),
        _ => Ok((
            None,
            fetch(dir, "HEAD").context("fetching the default branch of `origin`")?,
        )),
    }
}

/// Stages the files of `update` from the checkout at `dir` in the worktree `work`: false when
/// none of its own changed.
fn stage(dir: &Path, work: &Path, update: &Update) -> Result<bool> {
    let Update {
        paths,
        files,
        shared,
        ..
    } = *update;
    let own: Vec<&str> = paths.iter().chain(files).map(String::as_str).collect();
    if own.is_empty() {
        return Ok(false);
    }
    let carried: Vec<String> = files.iter().chain(shared).cloned().collect();
    mirror(dir, work, paths, &carried)?;
    git(work, &[&["add", "--all", "--"][..], &own].concat())?;
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(&own);
    if git(work, &status)?.is_empty() {
        return Ok(false);
    }
    if !shared.is_empty() {
        let mut add = vec!["add", "--all", "--"];
        add.extend(shared.iter().map(String::as_str));
        git(work, &add)?;
    }
    Ok(true)
}

/// How many files the pull request of `update` would change.
pub fn changed_files(dir: &Path, update: &Update) -> Result<usize> {
    let (_, start) = upstream(dir)?;
    let tree = Worktree::add(dir, &start)?;
    if !stage(dir, &tree.path, update)? {
        return Ok(0);
    }
    let names = git(
        &tree.path,
        &["diff", "--cached", "--name-only", "--no-renames"],
    )?;
    Ok(names.lines().count())
}

/// Whether the repository at `dir` has an open update pull request of one of its SDKs.
pub fn splitting(github: &Client, dir: &Path) -> Result<bool> {
    let remote = git(dir, &["remote", "get-url", "origin"])?;
    let Some(repo) = repo_of(&remote) else {
        return Ok(false);
    };
    let pulls = github
        .api()?
        .get(&format!("/repos/{repo}/pulls?state=open&per_page=100"))?;
    Ok(pulls
        .as_array()
        .into_iter()
        .flatten()
        .any(|p| p["head"]["ref"].as_str().is_some_and(is_sdk_branch)))
}

/// What a pull request of [`open`] brings.
pub struct Update<'a> {
    pub branch: &'a str,
    /// Directories whose generated files it brings.
    pub paths: &'a [String],
    /// Other files it brings.
    pub files: &'a [String],
    /// Files it brings along with changes to `paths` or `files`, never alone.
    pub shared: &'a [String],
}

/// Commits the files of `update` of the repository at `dir` on top of its upstream branch to
/// the update branch, and opens (or refreshes) its PR. The commit is made in a temporary
/// worktree: the checkout at `dir` keeps its branch, index and files.
/// An open PR keeps its bump if larger: it releases every spec change since the last merge.
/// Opens or updates the pull request of `dir`, its description from `describe` given the section
/// listing the API changes: those of `changelog` and those it already lists, unless its SDKs are
/// generated for the first time.
#[allow(clippy::too_many_arguments)]
pub fn open(
    github: &Client,
    dir: &Path,
    update: &Update,
    bump: Bump,
    subject: &str,
    changelog: &Changelog,
    describe: impl FnOnce(&str) -> String,
) -> Result<Option<Pull>> {
    let Update {
        branch: target,
        paths,
        ..
    } = *update;
    let (base, start) = upstream(dir)?;
    let tree = Worktree::add(dir, &start)?;
    let work = tree.path.as_path();
    if !stage(dir, work, update)? {
        return Ok(None);
    }
    let remote = git(dir, &["remote", "get-url", "origin"])?;
    let repo = repo_of(&remote)
        .with_context(|| format!("`origin` of {} isn't on GitHub: {remote}", dir.display()))?;
    let owner = repo.split('/').next().unwrap_or_default();
    let api = github.api()?;
    let pulls = api.get(&format!(
        "/repos/{repo}/pulls?head={owner}:{target}&state=open"
    ))?;
    github.check_expiration(api);
    let existing = pulls.get(0).filter(|p| p.is_object());
    let previous = existing
        .and_then(|p| p["title"].as_str())
        .unwrap_or_default();
    let title = bump
        .max(Bump::of_title(previous).unwrap_or(bump))
        .title(subject);
    let mut grep = vec![
        "grep",
        "-q",
        "-i",
        "-I",
        "-e",
        "@generated",
        "-e",
        "Code generated by perseid",
    ];
    grep.extend([start.as_str(), "--"]);
    grep.extend(paths.iter().map(String::as_str));
    let changelog = match git(dir, &grep) {
        Ok(_) => {
            let listed = existing
                .and_then(|p| p["body"].as_str())
                .unwrap_or_default();
            Changelog::of_description(listed).merge(changelog.clone())
        }
        Err(_) => Changelog::default(),
    };
    let nested = changelog.nested();
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
    if !nested.is_empty() {
        commit.extend(["-m", &nested]);
    }
    git(work, &commit)?;
    let head = git(work, &["rev-parse", "HEAD"])?;
    drop(tree);
    let commit_of = |sha: &str| {
        let tree = git(dir, &["rev-parse", &format!("{sha}^{{tree}}")]).ok();
        (tree, git(dir, &["log", "-1", "--format=%B", sha]).ok())
    };
    let unchanged = fetch(dir, target).is_ok_and(|sha| commit_of(&sha) == commit_of(&head));
    if !unchanged {
        let refspec = format!("{head}:refs/heads/{target}");
        remote_git(dir, &["push", "--quiet", "--force", "origin", &refspec])?;
    }
    let body = describe(&changelog.section());
    let mut content = json!({ "title": title, "body": body });
    let pull = match existing {
        Some(pull) => {
            let number = pull["number"].as_u64().unwrap_or_default();
            api.patch(&format!("/repos/{repo}/pulls/{number}"), content)?
        }
        None => {
            content["head"] = target.into();
            content["base"] = base.map_or_else(|| default_branch(dir), Ok)?.into();
            api.post(&format!("/repos/{repo}/pulls"), content)?
        }
    };
    Pull::new(&repo, target, &pull).map(Some)
}

/// Closes the open pull request of [`BRANCH`] in the repository at `dir`, which the update
/// pull requests `by` of each of its SDKs replace, and deletes the branch.
pub fn close_superseded(github: &Client, dir: &Path, by: &[Pull]) -> Result<()> {
    let links: Vec<&str> = by.iter().map(|p| p.url.as_str()).collect();
    let reason = format!(
        "Replaced by one pull request per SDK, as release-please reads at most 3000 files of a \
         commit to tell which SDKs it changed: {}",
        links.join(", ")
    );
    close(github, dir, BRANCH, &reason)
}

/// Closes the open pull request of `branch`, the update branch of an SDK the base branch now
/// holds as generated, and deletes the branch.
pub fn close_stale(github: &Client, dir: &Path, branch: &str) -> Result<()> {
    let reason = "Closed: the base branch already holds this SDK as perseid generates it now.";
    close(github, dir, branch, reason)
}

/// Closes the open pull request of `branch` in the repository at `dir`, with `reason` as a
/// comment, and deletes the branch.
fn close(github: &Client, dir: &Path, branch: &str, reason: &str) -> Result<()> {
    let remote = git(dir, &["remote", "get-url", "origin"])?;
    let Some(repo) = repo_of(&remote) else {
        return Ok(());
    };
    let owner = repo.split('/').next().unwrap_or_default();
    let api = github.api()?;
    let pulls = api.get(&format!(
        "/repos/{repo}/pulls?head={owner}:{branch}&state=open"
    ))?;
    let Some(number) = pulls.get(0).and_then(|p| p["number"].as_u64()) else {
        return Ok(());
    };
    api.post(
        &format!("/repos/{repo}/issues/{number}/comments"),
        json!({ "body": reason }),
    )?;
    api.patch(
        &format!("/repos/{repo}/pulls/{number}"),
        json!({ "state": "closed" }),
    )?;
    let path = format!("/repos/{repo}/git/refs/heads/{branch}");
    let reply = api.send("DELETE", &path, None)?;
    if reply.status != 422 && reply.status != 404 {
        check("DELETE", &path, reply)?;
    }
    Ok(())
}

/// Enables auto-merge (squash) on `pull`, labelled so its release PR follows.
pub fn auto_merge(github: &Client, pull: &Pull) -> Result<()> {
    let api = github.api()?;
    let Pull {
        url,
        repo,
        number,
        node_id,
        ..
    } = pull;
    let labels = format!("/repos/{repo}/labels");
    let label = json!({
        "name": AUTO_RELEASE,
        "color": "6f42c1",
        "description": "Auto-merge the release PR this change leads to",
    });
    let reply = api.send("POST", &labels, Some(&label))?;
    if reply.status != 422 {
        check("POST", &labels, reply)?;
    }
    let add = json!({ "labels": [AUTO_RELEASE] });
    api.post(&format!("/repos/{repo}/issues/{number}/labels"), add)?;
    let query = "mutation($id: ID!) { enablePullRequestAutoMerge(input: \
                 { pullRequestId: $id, mergeMethod: SQUASH }) { clientMutationId } }";
    let reply = api.post(
        "/graphql",
        json!({ "query": query, "variables": { "id": node_id } }),
    )?;
    let Some(error) = reply["errors"][0]["message"].as_str() else {
        return Ok(());
    };
    let lower = error.to_lowercase();
    // GitHub queues no merge for pull requests it could merge now: merge them, as gh does.
    if lower.contains("clean status") || lower.contains("unstable status") {
        let merge = json!({ "merge_method": "squash" });
        api.put(&format!("/repos/{repo}/pulls/{number}/merge"), merge)?;
        return Ok(());
    }
    ensure!(
        lower.contains("auto merge is not allowed") || lower.contains("auto-merge is not allowed"),
        "enabling auto-merge on {url}: {error}"
    );
    warn(&format!(
        "{repo} doesn't allow auto-merge, so {url} waits to be merged: turn on \"Allow \
         auto-merge\" in its settings (General)"
    ));
    Ok(())
}

/// Runs `workflows` on the update branch of `pull`: pushes made with the default `GITHUB_TOKEN`
/// start none.
pub fn dispatch(github: &Client, pull: &Pull, workflows: &[String]) -> Result<()> {
    for workflow in workflows {
        let path = format!(
            "/repos/{}/actions/workflows/{workflow}/dispatches",
            pull.repo
        );
        github.api()?.post(&path, json!({ "ref": pull.branch }))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AUTO_RELEASE, Bump, SOURCE, branch_of, is_update_branch, origin, repo_of, token_config,
    };

    #[test]
    fn each_sdk_of_a_repository_has_its_update_branch() {
        assert_eq!(branch_of("rust"), "perseid/update-rust");
        for branch in ["perseid/update", "perseid/update-rust"] {
            assert!(is_update_branch(branch), "{branch}");
        }
        for branch in ["perseid/updates", "main", "release-please--branches--main"] {
            assert!(!is_update_branch(branch), "{branch}");
        }
    }

    #[test]
    fn the_release_action_reads_the_auto_release_label() {
        let action = include_str!("../release/action.yml");
        assert!(action.contains(&format!(" {AUTO_RELEASE} ")), "{action}");
    }

    #[test]
    fn remotes_name_their_github_repository() {
        for remote in [
            "https://github.com/acme/api-go",
            "https://x-access-token@github.com/acme/api-go.git/",
            "git@github.com:acme/api-go.git",
        ] {
            assert_eq!(repo_of(remote).as_deref(), Some("acme/api-go"), "{remote}");
        }
        assert_eq!(repo_of("origin.git"), None);
    }

    #[test]
    fn pushes_reset_the_extra_headers_before_adding_the_token() {
        let [(reset, empty), (key, header)] = token_config("ghs_1");
        assert_eq!((reset, empty.as_str()), (key, ""));
        assert_eq!(key, "http.https://github.com/.extraheader");
        assert_eq!(header, "AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46Z2hzXzE=");
    }

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
