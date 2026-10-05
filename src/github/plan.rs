//! What `perseid app` and `perseid connect` change on GitHub for perseid.toml to hold, computed
//! without writing, then applied.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use heck::ToKebabCase;
use serde_json::Value;

use super::{
    Ui,
    api::GitHub,
    app,
    bootstrap::{self, File},
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
    App(Box<AppPlan>),
    AppKey(Box<app::NewKey>),
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
    /// Something waits on the user outside the plan: files to refresh or push, a repository or a
    /// secret to add.
    pub attention: bool,
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
                Action::App(app) => Some(format!(
                    "create a GitHub App with Contents, Pull requests and Workflows read and write, install it on {}, and set its ID as the SDK_APP_ID variable and a private key as the SDK_APP_PRIVATE_KEY secret of each (https://github.com/meteroid-oss/perseid/blob/main/docs/ci.md#tokens)",
                    app.repos.join(", ")
                )),
                Action::AppKey(new) => Some(format!(
                    "generate a private key at {}, then `gh secret set SDK_APP_PRIVATE_KEY -R <repo> < <downloaded>.private-key.pem` for {}",
                    app::settings_url(new.slug.as_deref(), &new.owner),
                    new.repos.join(", ")
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
            attention: false,
            hub_config: None,
        }
    }

    pub(super) fn add(&mut self, mark: Mark, text: String, action: Option<Action>) {
        self.steps.push(Step { mark, text, action });
    }
}

/// The secret holding the token sdks.yml and sdk-release.yml open pull requests with.
pub const TOKEN: &str = "PERSEID_TOKEN";

pub struct Session<'a> {
    pub api: &'a GitHub,
    pub login: &'a str,
    /// Plans a GitHub App, instead of checking the `PERSEID_TOKEN` secret.
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
            "{other} isn't owned by {owner}: one token covers the repositories of a single account"
        );
    }
    if !same(&owner, &hub_owner) && local_sdks {
        bail!(
            "SDKs kept in {} need the App on {hub_owner}, not {owner}: set `repo` for each of them",
            hub.repo
        );
    }
    let mut absent = Vec::new();
    let mut repos = vec![hub.repo.clone()];
    for repo in &remote {
        match api.find(&format!("/repos/{repo}"))? {
            Some(_) => repos.push(repo.clone()),
            None => absent.push(format!(
                "`gh repo create {repo} --{}`",
                visibility(here_info)
            )),
        }
    }
    if !absent.is_empty() {
        plan.warnings.push(format!(
            "create the SDK repositories perseid.toml names: {}. sdks.yml then opens their first pull request, with the SDK and its release workflow",
            absent.join(", ")
        ));
        plan.attention = true;
    }
    let hub_base = default_branch(&hub.info);
    let app_set = hub.info.is_some()
        && secrets::variable(api, &hub.repo, "SDK_APP_ID")
            .ok()
            .flatten()
            .is_some();
    if cx.app || app_set {
        plan.app = true;
        let installed: Vec<String> = repos
            .iter()
            .filter(|r| same(owner_of(r), &owner))
            .cloned()
            .collect();
        let who = app::owner(api, &owner)?;
        if !who.organization && !same(&who.login, cx.login) {
            bail!(
                "{} is another user's account: GitHub only lets {} create Apps for itself or its organizations",
                who.login,
                cx.login
            );
        }
        let name = hub.config.name.to_kebab_case();
        plan_app(cx, plan, &hub.repo, repos, installed, who, name)?;
    } else {
        let missing: Vec<&String> = repos
            .iter()
            .filter(|r| !secrets::has_secret(api, r, TOKEN).unwrap_or(false))
            .collect();
        match missing.as_slice() {
            [] => plan.add(
                Mark::Keep,
                format!("{}: {TOKEN} set", repos.join(", ")),
                None,
            ),
            missing => {
                let list: Vec<&str> = missing.iter().map(|r| r.as_str()).collect();
                plan.warnings.push(format!(
                    "add the {TOKEN} secret to {}: a fine-grained token at https://github.com/settings/personal-access-tokens/new for {} with Contents, Pull requests and Workflows read and write. It expires: `perseid app` sets up a GitHub App instead",
                    list.join(", "),
                    repos.join(", ")
                ));
                plan.attention = true;
            }
        }
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
    plan.attention |= !stale.is_empty() || !unpushed.is_empty();
    if cx.collisions {
        collisions(cx, plan, &hub.config, &hub.repo, &hub.local.1)?;
    }
    plan.hub_dir = hub.dir.clone();
    plan.hub = hub.repo;
    plan.hub_config = Some(hub.config);
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
                "GitHub App {}: SDK_APP_ID{} on {list}",
                slug.as_deref().unwrap_or(id),
                if keyless.is_empty() {
                    " and SDK_APP_PRIVATE_KEY"
                } else {
                    ""
                }
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
    let new_key = match (&id, keyless.is_empty()) {
        (Some(id), false) => Some(app::NewKey {
            id: id.clone(),
            slug: slug.clone(),
            owner: owner.clone(),
            repos: keyless,
        }),
        _ => None,
    };
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
    if let Some(new) = new_key {
        plan.add(
            Mark::Add,
            format!(
                "SDK_APP_PRIVATE_KEY on {}: a new key of the App, generated in your browser",
                new.repos.join(", ")
            ),
            Some(Action::AppKey(Box::new(new))),
        );
    }
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
            plan.attention = true;
        }
    }
    Ok(())
}

/// Applies the plan's actions.
pub fn apply(api: &GitHub, plan: &mut Plan, ui: &Ui) -> Result<()> {
    for step in std::mem::take(&mut plan.steps) {
        let Some(action) = step.action else {
            continue;
        };
        match action {
            Action::App(app) => apply_app(api, *app, ui)?,
            Action::AppKey(new) => app::add_key(api, &new, ui)?,
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
    Ok(())
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
