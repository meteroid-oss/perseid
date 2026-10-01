//! What `perseid setup-github` changes on GitHub for perseid.toml to hold, computed without
//! writing, then applied.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use heck::ToKebabCase;
use serde_json::{Value, json};

use super::{
    SETUP_BRANCH, Ui,
    api::GitHub,
    app,
    bootstrap::{self, File, Outcome},
    files, git, join,
    layout::{self, SDKS_WORKFLOW},
    link, relative, secrets, toplevel,
};
use crate::{
    config::{Config, Source},
    scaffold::RELEASE_WORKFLOW,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Add,
    Change,
    Keep,
}

pub struct Step {
    pub mark: Mark,
    pub text: String,
    pub(super) action: Option<Action>,
}

pub(super) enum Action {
    CreateRepo {
        repo: String,
        organization: bool,
        visibility: String,
        description: String,
    },
    Commit(Commit),
    AllowPullRequests(String),
    App(Box<AppPlan>),
    /// Writes `file` in the checkout at `top`, leaving the commit to the user.
    Write {
        top: PathBuf,
        file: File,
    },
    DeployKey {
        api_repo: String,
        sdks_repo: String,
        /// Whether `perseid connect` can register it on `sdks_repo`, and the key it replaces.
        register: (bool, Option<u64>),
    },
}

pub(super) struct Commit {
    pub repo: String,
    pub base: String,
    pub files: Vec<File>,
    pub message: &'static str,
    /// Opens a pull request saying this, instead of committing to `base`.
    pub pull_request: Option<String>,
}

pub(super) struct AppPlan {
    hub: String,
    owner: app::Owner,
    name: String,
    existing: Option<(String, Option<String>)>,
    repos: Vec<String>,
    /// Repositories new to the App, whose installation must grow.
    joining: Vec<String>,
}

/// A repository running sdks.yml: where perseid runs and opens SDK pull requests from.
struct Hub {
    repo: String,
    info: Option<Value>,
    config: Config,
    /// perseid.toml's directory, relative to the repository root.
    dir: String,
    /// The checkout's top and perseid.toml's directory.
    local: (PathBuf, PathBuf),
}

pub struct Plan {
    pub diagram: String,
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    pub hub: String,
    /// perseid.toml's directory in the hub.
    pub hub_dir: String,
    /// Repositories receiving SDK pull requests.
    pub targets: Vec<String>,
    pub app: bool,
    /// The spec isn't in the hub yet: `perseid connect` pushes it there.
    pub awaits_spec: bool,
    /// Files perseid keeps here are out of date or not pushed yet.
    pub unpushed: bool,
    hub_config: Option<Config>,
}

impl Plan {
    pub fn pending(&self) -> usize {
        self.steps.iter().filter(|s| s.mark != Mark::Keep).count()
    }

    pub fn print(&self) {
        for step in &self.steps {
            let mark = match step.mark {
                Mark::Add => '+',
                Mark::Change => '~',
                Mark::Keep => '=',
            };
            println!("  {mark} {}", step.text);
        }
        for warning in &self.warnings {
            println!("! {warning}");
        }
    }

    /// How to make the pending changes by hand.
    pub fn manual(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter(|s| s.mark != Mark::Keep)
            .filter_map(|s| match s.action.as_ref()? {
                Action::CreateRepo {
                    repo, visibility, ..
                } => Some(format!("gh repo create {repo} --{visibility}")),
                Action::Commit(commit) => {
                    let paths: Vec<String> = commit.files.iter().map(|f| f.path.clone()).collect();
                    Some(format!("{}: commit {}", commit.repo, names(&paths)))
                }
                Action::AllowPullRequests(repo) => Some(format!(
                    "{repo}: Settings, Actions, General, check \"Allow GitHub Actions to create and approve pull requests\""
                )),
                Action::App(app) => Some(format!(
                    "create a GitHub App with Contents and Pull requests read and write, install it on {}, and set its ID as the SDK_APP_ID variable and a private key as the SDK_APP_PRIVATE_KEY secret of each (https://github.com/meteroid-oss/perseid/blob/main/docs/ci.md#tokens)",
                    app.repos.join(", ")
                )),
                Action::Write { file, .. } => Some(format!("write {}", file.path)),
                Action::DeployKey { .. } => None,
            })
            .collect()
    }

    pub fn config(&self) -> &Config {
        self.hub_config.as_ref().expect("a planned hub")
    }

    pub(super) fn new() -> Self {
        Self {
            diagram: String::new(),
            steps: Vec::new(),
            warnings: Vec::new(),
            hub: String::new(),
            hub_dir: String::new(),
            targets: Vec::new(),
            app: false,
            awaits_spec: false,
            unpushed: false,
            hub_config: None,
        }
    }

    pub(super) fn add(&mut self, mark: Mark, text: String, action: Option<Action>) {
        self.steps.push(Step { mark, text, action });
    }
}

pub struct Session<'a> {
    pub api: &'a GitHub,
    pub login: &'a str,
    /// Plans a GitHub App even when every SDK lives here, so CI runs on SDK pull requests.
    pub app: bool,
    /// Renders the SDKs to warn about files perseid would overwrite: slow, so `setup` only.
    pub collisions: bool,
}

fn default_branch(info: &Option<Value>) -> String {
    info.as_ref()
        .map_or_else(|| "main".to_owned(), bootstrap::default_branch)
}

/// Compares perseid.toml at `config_path` with GitHub.
pub fn plan(cx: &Session, config_path: &Path) -> Result<Plan> {
    let api = cx.api;
    let (config, root) = Config::load(config_path)?;
    let here = layout::required_origin_repo(&root, "the repository holding perseid.toml")?;
    let top = toplevel(&root)?;
    let dir = relative(&root, &top)?;
    let here_info = api.get(&format!("/repos/{here}"))?;
    let mut plan = Plan::new();
    unused_folders(&mut plan, &config, &root)?;
    let source = match config.source() {
        Source::Url(url) => Some(url.to_owned()),
        Source::File(file) => {
            let spec = join(&dir, file);
            let tracked = git(&top, &["ls-files", "--error-unmatch", "--", &spec]).is_ok();
            plan.awaits_spec = !tracked && !root.join(file).exists();
            link::source(&root).and_then(|s| s.repo)
        }
    };
    let hub = Hub {
        repo: here.clone(),
        info: Some(here_info.clone()),
        config,
        dir,
        local: (top, root),
    };
    plan_hub(cx, &mut plan, hub, &here_info)?;
    plan.diagram = layout::diagram(source.as_deref(), &here, &plan.targets);
    Ok(plan)
}

/// Warns about the SDK folders `perseid init` wrote here that perseid.toml now sends elsewhere.
fn unused_folders(plan: &mut Plan, config: &Config, root: &Path) -> Result<()> {
    let unused: Vec<String> = config
        .sdks(&[])?
        .iter()
        .filter(|s| !s.local)
        .filter(|s| root.join(s.language).is_dir())
        .map(|s| format!("{}/", s.language))
        .collect();
    match unused.as_slice() {
        [] => {}
        [one] => plan
            .warnings
            .push(format!("{one} isn't generated here anymore: delete it")),
        many => plan.warnings.push(format!(
            "{} aren't generated here anymore: delete them",
            many.join(", ")
        )),
    }
    Ok(())
}

fn visibility(info: &Value) -> String {
    info["visibility"]
        .as_str()
        .unwrap_or(if info["private"] == true {
            "private"
        } else {
            "public"
        })
        .to_owned()
}

fn names(paths: &[String]) -> String {
    let short: Vec<&str> = paths.iter().take(4).map(String::as_str).collect();
    match paths.len() {
        n if n > 4 => format!("{} and {} more files", short.join(", "), n - 4),
        _ => short.join(", "),
    }
}

/// Repository names without their owner.
fn short(repos: &[String]) -> String {
    let names: Vec<&str> = repos
        .iter()
        .map(|r| r.rsplit('/').next().unwrap_or(r))
        .collect();
    names.join(", ")
}

fn owner_of(repo: &str) -> &str {
    repo.split('/').next().unwrap_or_default()
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn plan_hub(cx: &Session, plan: &mut Plan, hub: Hub, here_info: &Value) -> Result<()> {
    let api = cx.api;
    let sdks = hub.config.sdks(&[])?;
    let remote: BTreeSet<String> = sdks
        .iter()
        .filter_map(|s| s.remote().map(str::to_owned))
        .collect();
    let local_sdks = sdks.iter().any(|s| s.local);
    plan.targets = layout::targets(&sdks, &hub.repo);
    let hub_owner = owner_of(&hub.repo).to_owned();
    let owner = remote
        .first()
        .map_or(hub_owner.clone(), |r| owner_of(r).to_owned());
    if let Some(other) = remote.iter().find(|r| !same(owner_of(r), &owner)) {
        bail!(
            "{other} isn't owned by {owner}: the App's token covers the repositories of one account"
        );
    }
    if !same(&owner, &hub_owner) && local_sdks {
        bail!(
            "SDKs kept in {} need the App on {hub_owner}, not {owner}: set `repo` for each of them",
            hub.repo
        );
    }
    let visibility = visibility(here_info);
    let description = format!("{} API SDK, generated by perseid", hub.config.name);
    let mut created = BTreeSet::new();
    let mut accounts = std::collections::BTreeMap::new();
    let mut account = |login: &str| -> Result<app::Owner> {
        if let Some((l, o)) = accounts.get(login) {
            return Ok(app::Owner {
                login: String::clone(l),
                organization: *o,
            });
        }
        let owner = app::owner(api, login)?;
        if !owner.organization && !same(&owner.login, cx.login) {
            bail!(
                "{} is another user's account: GitHub only lets {} create repositories and Apps for itself or its organizations",
                owner.login,
                cx.login
            );
        }
        accounts.insert(login.to_owned(), (owner.login.clone(), owner.organization));
        Ok(owner)
    };
    let mut existing = Vec::new();
    for (repo, info) in std::iter::once((hub.repo.clone(), hub.info.clone())).chain(
        remote
            .iter()
            .map(|r| Ok((r.clone(), api.find(&format!("/repos/{r}"))?)))
            .collect::<Result<Vec<_>>>()?,
    ) {
        match info {
            Some(_) if repo != hub.repo => existing.push(repo),
            Some(_) => {}
            None => {
                let who = account(owner_of(&repo))?;
                plan.add(
                    Mark::Add,
                    format!("create {repo} ({visibility})"),
                    Some(Action::CreateRepo {
                        repo: repo.clone(),
                        organization: who.organization,
                        visibility: visibility.clone(),
                        description: description.clone(),
                    }),
                );
                created.insert(repo);
            }
        }
    }
    if !existing.is_empty() {
        plan.add(
            Mark::Keep,
            format!(
                "{}: existing, files added only where missing",
                existing.join(", ")
            ),
            None,
        );
    }
    let release = hub.config.release != Some(false);
    for repo in remote.iter().filter(|_| release) {
        let held: Vec<&crate::config::Sdk> =
            sdks.iter().filter(|s| s.remote() == Some(repo)).collect();
        let base = match created.contains(repo) {
            true => "main".to_owned(),
            false => default_branch(&api.find(&format!("/repos/{repo}"))?),
        };
        let mut files = Vec::new();
        let mut updated = Vec::new();
        for file in bootstrap::release_files(api, &hub.config, repo, &base, &held)? {
            let remote = match created.contains(repo) {
                true => None,
                false => api.raw(repo, &base, &file.path)?,
            };
            if remote.as_deref() == Some(file.content.as_slice()) {
                continue;
            }
            if file.path == RELEASE_WORKFLOW && !files::owned(remote.as_deref()) {
                plan.warnings
                    .push(format!("{repo}: {}", files::kept(RELEASE_WORKFLOW, &base)));
                continue;
            }
            if remote.is_some() {
                updated.push(file.path.clone());
            }
            files.push(file);
        }
        if files.is_empty() {
            plan.add(Mark::Keep, format!("{repo}: release files in place"), None);
            continue;
        }
        let commit = Commit {
            repo: repo.clone(),
            base,
            files,
            message: "ci: release the SDK with release-please",
            pull_request: (!created.contains(repo)).then(|| {
                format!("`{RELEASE_WORKFLOW}` releases the {} SDK with release-please when its release pull request is merged.", hub.config.name)
            }),
        };
        plan_commit(cx, plan, commit, &updated)?;
    }

    let hub_base = default_branch(&hub.info);
    let allowed = match (&hub.info, remote.is_empty()) {
        (Some(_), true) => api
            .find(&format!("/repos/{}/actions/permissions/workflow", hub.repo))?
            .is_some_and(|p| p["can_approve_pull_request_reviews"] == true),
        _ => false,
    };
    let app_set = hub.info.is_some()
        && secrets::variable(api, &hub.repo, "SDK_APP_ID")
            .ok()
            .flatten()
            .is_some();
    let default_token = remote.is_empty() && !app_set && (!cx.app || allowed);
    if default_token {
        plan.warnings.push(format!(
            "SDK pull requests opened with the default token run no CI: `perseid setup-github` without --no-app sets up a GitHub App for {}",
            hub.repo
        ));
        match allowed {
            true => plan.add(
                Mark::Keep,
                format!("{}: GitHub Actions may open pull requests", hub.repo),
                None,
            ),
            false => plan.add(
                Mark::Change,
                format!("{}: let GitHub Actions open pull requests", hub.repo),
                Some(Action::AllowPullRequests(hub.repo.clone())),
            ),
        }
    } else {
        plan.app = true;
        let mut configured = vec![hub.repo.clone()];
        configured.extend(remote.iter().cloned());
        let installed: Vec<String> = configured
            .iter()
            .filter(|r| same(owner_of(r), &owner))
            .cloned()
            .collect();
        let who = account(&owner)?;
        let name = hub.config.name.to_kebab_case();
        plan_app(cx, plan, &hub.repo, configured, installed, who, name)?;
    }
    let expected = super::files::expected(&hub.config, &hub.local.1)?;
    if expected.branch != hub_base {
        plan.warnings.push(format!(
            "the workflows here run on `{}`, but the default branch of {} is `{hub_base}`: run `perseid init` once `origin/HEAD` points to it (`git remote set-head origin --auto`)",
            expected.branch, hub.repo
        ));
    }
    let (mut stale, mut unpushed) = (Vec::new(), Vec::new());
    for (path, content) in &expected.files {
        let here = std::fs::read(expected.top.join(path)).ok();
        let workflow = path == SDKS_WORKFLOW || path == RELEASE_WORKFLOW;
        if workflow && !files::owned(here.as_deref()) {
            plan.warnings.push(files::kept(path, &hub_base));
            continue;
        }
        if here.as_deref() != Some(content.as_slice()) {
            stale.push(path.clone());
        } else if hub.info.is_some()
            && api.raw(&hub.repo, &hub_base, path)?.as_deref() != here.as_deref()
        {
            unpushed.push(path.clone());
        }
    }
    if !stale.is_empty() {
        plan.warnings.push(format!(
            "{} {} out of date here: run `perseid init`, then commit and push",
            names(&stale),
            if stale.len() == 1 { "is" } else { "are" }
        ));
    }
    if !unpushed.is_empty() {
        plan.warnings.push(format!(
            "{} {} not on `{hub_base}` of {} yet: commit and push",
            names(&unpushed),
            if unpushed.len() == 1 { "is" } else { "are" },
            hub.repo
        ));
    }
    plan.unpushed = !stale.is_empty() || !unpushed.is_empty();
    if cx.collisions {
        collisions(cx, plan, &hub.config, &hub.repo, &hub.local.1)?;
    }
    plan.hub_dir = hub.dir.clone();
    plan.hub = hub.repo;
    plan.hub_config = Some(hub.config);
    Ok(())
}

/// Adds the step committing `commit.files`, unless its pull request already holds them.
pub(super) fn plan_commit(
    cx: &Session,
    plan: &mut Plan,
    commit: Commit,
    updated: &[String],
) -> Result<()> {
    let repo = commit.repo.clone();
    if commit.files.is_empty() {
        plan.add(
            Mark::Keep,
            format!("{repo}: workflow and files in place"),
            None,
        );
        return Ok(());
    }
    let paths: Vec<String> = commit.files.iter().map(|f| f.path.clone()).collect();
    let mark = match updated.len() == paths.len() {
        true => Mark::Change,
        false => Mark::Add,
    };
    let what = match updated.is_empty() {
        true => names(&paths),
        false => format!("{} (updating {})", names(&paths), names(updated)),
    };
    if commit.pull_request.is_none() {
        plan.add(
            mark,
            format!("{repo}: commit {what}"),
            Some(Action::Commit(commit)),
        );
        return Ok(());
    }
    if let Some(url) = bootstrap::open_pull(cx.api, &repo, SETUP_BRANCH)? {
        let mut held = true;
        for file in &commit.files {
            held &= cx.api.raw(&repo, SETUP_BRANCH, &file.path)?.as_deref()
                == Some(file.content.as_slice());
        }
        if held {
            plan.add(Mark::Keep, format!("{repo}: {url} holds {what}"), None);
            return Ok(());
        }
        plan.add(
            Mark::Change,
            format!("{repo}: update {url} with {what}"),
            Some(Action::Commit(commit)),
        );
        return Ok(());
    }
    plan.add(
        mark,
        format!("{repo}: pull request with {what}"),
        Some(Action::Commit(commit)),
    );
    Ok(())
}

fn plan_app(
    cx: &Session,
    plan: &mut Plan,
    hub: &str,
    configured: Vec<String>,
    installed: Vec<String>,
    owner: app::Owner,
    name: String,
) -> Result<()> {
    let api = cx.api;
    let id = secrets::variable(api, hub, "SDK_APP_ID")?;
    let slug = secrets::variable(api, hub, "SDK_APP_SLUG")?;
    let mut joining = Vec::new();
    let mut keyless = Vec::new();
    for repo in &configured {
        if id.is_none() || secrets::variable(api, repo, "SDK_APP_ID")? != id {
            joining.push(repo.clone());
        }
        if !secrets::has_secret(api, repo, "SDK_APP_PRIVATE_KEY")? {
            keyless.push(repo.clone());
        }
    }
    let list = configured.join(", ");
    let (mark, text) = match (&id, joining.is_empty()) {
        (None, _) => (
            Mark::Add,
            format!(
                "GitHub App {name}-sdk-bot on {}, created in your browser and installed on {}, with SDK_APP_ID and SDK_APP_PRIVATE_KEY on each",
                owner.login,
                short(&installed)
            ),
        ),
        (Some(id), true) => (
            Mark::Keep,
            format!(
                "GitHub App {}: SDK_APP_ID and SDK_APP_PRIVATE_KEY on {list}",
                slug.as_deref().unwrap_or(id)
            ),
        ),
        (Some(id), false) => (
            Mark::Change,
            format!(
                "GitHub App {}: SDK_APP_ID on {}, installed there too",
                slug.as_deref().unwrap_or(id),
                joining.join(", ")
            ),
        ),
    };
    if id.is_some() && !keyless.is_empty() {
        plan.warnings.push(format!(
            "Add a private key of the App (generated on its settings page) as the SDK_APP_PRIVATE_KEY secret of {}",
            keyless.join(", ")
        ));
    }
    let action = (mark != Mark::Keep).then(|| {
        Action::App(Box::new(AppPlan {
            hub: hub.to_owned(),
            owner,
            name,
            existing: id.map(|id| (id, slug)),
            repos: configured,
            joining: joining
                .into_iter()
                .filter(|r| installed.contains(r))
                .collect(),
        }))
    });
    plan.add(mark, text, action);
    Ok(())
}

/// Warns about generated paths taken by files perseid didn't generate.
fn collisions(
    cx: &Session,
    plan: &mut Plan,
    config: &Config,
    hub: &str,
    root: &Path,
) -> Result<()> {
    let Ok(spec) = crate::generate::load_spec(config, root, None) else {
        return Ok(());
    };
    for sdk in config.sdks(&[])? {
        let planned = crate::generate::planned(config, root, &sdk, &spec)?;
        let repo = sdk.remote().unwrap_or(hub).to_owned();
        let mut taken = Vec::new();
        match sdk.remote() {
            None => {
                for path in &planned {
                    let file = root.join(&sdk.path).join(path);
                    if file.exists()
                        && !std::fs::read_to_string(&file)
                            .is_ok_and(|t| crate::generate::marked(&file, &t))
                    {
                        taken.push(
                            file.strip_prefix(root)
                                .unwrap_or(&file)
                                .display()
                                .to_string(),
                        );
                    }
                }
            }
            _ => {
                let Some(info) = cx.api.find(&format!("/repos/{repo}"))? else {
                    continue;
                };
                let branch = bootstrap::default_branch(&info);
                let existing = bootstrap::paths(cx.api, &repo, &branch)?;
                for path in &planned {
                    let path = join(
                        if sdk.path == "." { "" } else { &sdk.path },
                        &path.to_string_lossy(),
                    );
                    if !existing.contains(&path) || taken.len() >= 10 {
                        continue;
                    }
                    let text = cx.api.raw(&repo, &branch, &path)?.unwrap_or_default();
                    if !crate::generate::marked(Path::new(&path), &String::from_utf8_lossy(&text)) {
                        taken.push(path);
                    }
                }
            }
        }
        if !taken.is_empty() {
            plan.warnings.push(format!(
                "{repo}: perseid would overwrite files it didn't generate, and stops instead: {} (delete or move them, or set `path` of [{}])",
                taken.join(", "),
                sdk.language
            ));
        }
    }
    Ok(())
}

/// Applies the plan, returning the pull requests left to merge, the SDKs repository's first.
pub fn apply(api: &GitHub, plan: &mut Plan, ui: &Ui) -> Result<Vec<String>> {
    let mut pulls = Vec::new();
    for step in std::mem::take(&mut plan.steps) {
        let Some(action) = step.action else {
            continue;
        };
        match action {
            Action::CreateRepo {
                repo,
                organization,
                visibility,
                description,
            } => {
                let (owner, name) = repo.split_once('/').unwrap_or_default();
                let mut body = json!({
                    "name": name,
                    "description": description,
                    "private": visibility != "public",
                    "has_wiki": false,
                });
                let path = match organization {
                    true => {
                        body["visibility"] = visibility.into();
                        format!("/orgs/{owner}/repos")
                    }
                    false => "/user/repos".to_owned(),
                };
                api.post(&path, body)?;
                ui.ok(&format!("Created {repo}"));
            }
            Action::Commit(commit) => {
                if let Some(url) = apply_commit(api, commit, ui)? {
                    pulls.push(url);
                }
            }
            Action::AllowPullRequests(repo) => {
                let path = format!("/repos/{repo}/actions/permissions/workflow");
                let current = api.find(&path)?.unwrap_or_default();
                let body = json!({
                    "default_workflow_permissions": current["default_workflow_permissions"].as_str().unwrap_or("read"),
                    "can_approve_pull_request_reviews": true,
                });
                match api.send("PUT", &path, Some(&body))?.status {
                    200..=299 => ui.ok(&format!("GitHub Actions may open pull requests in {repo}")),
                    _ => ui.warn(&format!(
                        "Allow GitHub Actions to create pull requests in {repo} (Settings, Actions, General): its organization may forbid it"
                    )),
                }
            }
            Action::App(app) => apply_app(api, *app, ui)?,
            Action::Write { top, file } => {
                crate::fsx::write(&top.join(&file.path), &file.content)?;
                ui.ok(&format!("Wrote {}", file.path));
            }
            Action::DeployKey {
                api_repo,
                sdks_repo,
                register,
            } => link::add_deploy_key(api, &api_repo, &sdks_repo, register, ui)?,
        }
    }
    Ok(pulls)
}

fn apply_commit(api: &GitHub, commit: Commit, ui: &Ui) -> Result<Option<String>> {
    let Commit {
        repo,
        base,
        files,
        message,
        pull_request,
    } = commit;
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    let branch = match pull_request {
        Some(_) => SETUP_BRANCH,
        None => base.as_str(),
    };
    let outcome = bootstrap::commit(api, &repo, &base, branch, &files, message)?;
    let url = match (outcome, pull_request) {
        (Outcome::Unchanged, _) => None,
        (Outcome::Committed, None) => {
            ui.ok(&format!("{repo}: committed {}", names(&paths)));
            None
        }
        (Outcome::Committed, Some(summary)) => {
            let list: Vec<String> = paths.iter().map(|p| format!("- `{p}`")).collect();
            let body = format!("Set up by perseid: {summary}\n\n{}", list.join("\n"));
            let (url, opened) = bootstrap::pull_request(api, &repo, &base, branch, message, &body)?;
            match opened {
                true => ui.ok(&format!("Opened {url} with {}", names(&paths))),
                false => ui.ok(&format!("Updated {url} with {}", names(&paths))),
            }
            Some(url)
        }
    };
    Ok(url)
}

fn apply_app(api: &GitHub, plan: AppPlan, ui: &Ui) -> Result<()> {
    let app = match plan.existing {
        Some((id, slug)) => app::App {
            id,
            slug,
            pem: None,
        },
        None => app::create(&plan.owner, &plan.hub, &plan.name, ui)?,
    };
    super::credentials(api, &app, &plan.hub, &plan.repos, ui)?;
    let installed: Vec<String> = plan
        .repos
        .iter()
        .filter(|r| same(owner_of(r), &plan.owner.login))
        .cloned()
        .collect();
    match app.pem {
        Some(_) => app::install(&app, &plan.owner, &installed, ui),
        None if !plan.joining.is_empty() => app::install(&app, &plan.owner, &plan.joining, ui),
        None => Ok(()),
    }
}
