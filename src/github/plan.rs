//! What `perseid setup` changes on GitHub for perseid.toml to hold, computed without writing,
//! then applied.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail, ensure};
use heck::ToKebabCase;
use serde_json::{Value, json};

use super::{
    AppToken, SETUP_BRANCH, Ui, Workflow,
    api::GitHub,
    app,
    bootstrap::{self, File, Outcome},
    git, join,
    layout::{self, SDKS_WORKFLOW, set_top},
    link::{self, Push},
    relative, secrets, toplevel, workflow,
};
use crate::config::{self, Config, Source};

pub const PUSH_WORKFLOW: &str = ".github/workflows/perseid-push.yml";
const OWNED: &str = "# Written by `perseid";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Add,
    Change,
    Keep,
}

pub struct Step {
    pub mark: Mark,
    pub text: String,
    action: Option<Action>,
}

enum Action {
    CreateRepo {
        repo: String,
        organization: bool,
        visibility: String,
        description: String,
    },
    Commit(Commit),
    AllowPullRequests(String),
    App(Box<AppPlan>),
    DeployKey {
        api_repo: String,
        sdks_repo: String,
        stale: Option<u64>,
    },
    Variable {
        repo: String,
        name: &'static str,
        value: String,
    },
}

struct Commit {
    repo: String,
    base: String,
    files: Vec<File>,
    message: &'static str,
    /// Opens a pull request saying this, instead of committing to `base`.
    pull_request: Option<String>,
    /// The checkout of `repo` here: files it lacks are written, and all staged once committed.
    local: Option<PathBuf>,
}

struct AppPlan {
    hub: String,
    owner: app::Owner,
    name: String,
    existing: Option<(String, Option<String>)>,
    repos: Vec<String>,
    /// Repositories new to the App, whose installation must grow.
    joining: Vec<String>,
}

/// How the SDKs repository learns that the spec changed.
enum Trigger {
    /// A file of this repository, and perseid.toml.
    Paths(Vec<String>),
    /// A URL, polled daily.
    Schedule,
    /// Pushes from another repository, of the snapshot and its source.
    Received(String),
}

/// A repository running sdks.yml: where perseid runs and opens SDK pull requests from.
struct Hub {
    repo: String,
    info: Option<Value>,
    config: Config,
    toml: String,
    /// perseid.toml's directory, relative to the repository root.
    dir: String,
    /// The local checkout, when the hub is the repository `perseid setup` runs in.
    local: Option<(PathBuf, PathBuf)>,
    trigger: Trigger,
    /// Files seeding a snapshot, written only when missing.
    seeds: Vec<File>,
}

pub struct Link {
    pub api_repo: String,
    pub sdks_repo: String,
}

pub struct Plan {
    pub diagram: String,
    pub steps: Vec<Step>,
    pub warnings: Vec<String>,
    pub hub: String,
    /// perseid.toml's directory in the hub.
    pub hub_dir: String,
    pub link: Option<Link>,
    /// Repositories receiving SDK pull requests.
    pub targets: Vec<String>,
    pub app: bool,
    hub_config: Config,
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

    pub fn config(&self) -> &Config {
        &self.hub_config
    }

    fn add(&mut self, mark: Mark, text: String, action: Option<Action>) {
        self.steps.push(Step { mark, text, action });
    }
}

pub struct Session<'a> {
    pub api: &'a GitHub,
    pub login: &'a str,
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
    let text = std::fs::read_to_string(config_path)?;
    let here = layout::required_spec_repo(&root)?;
    let top = toplevel(&root)?;
    let dir = relative(&root, &top)?;
    let here_info = api.get(&format!("/repos/{here}"))?;
    let here_base = bootstrap::default_branch(&here_info);
    let local = Some((top.clone(), root.clone()));
    let config_file = join(&dir, config::FILE);
    let mut plan = Plan {
        diagram: String::new(),
        steps: Vec::new(),
        warnings: Vec::new(),
        hub: here.clone(),
        hub_dir: String::new(),
        link: None,
        targets: Vec::new(),
        app: false,
        hub_config: toml::from_str(&text)?,
    };
    let spec = config.spec.clone();
    let hub = match (Source::parse(&spec)?, config.push_spec.clone()) {
        (Source::File(file), Some(sdks_repo)) => {
            let spec_path = join(&dir, file);
            let tracked = git(&top, &["ls-files", "--error-unmatch", "--", &spec_path]).is_ok();
            ensure!(
                tracked || config.generate.is_some(),
                "{spec_path} isn't committed: set `generate` in perseid.toml to the command writing it in CI"
            );
            admin(&here_info, &here, &sdks_repo, cx.login)?;
            let info = api.find(&format!("/repos/{sdks_repo}"))?;
            if let Some(info) = &info
                && info["permissions"]["admin"] == false
            {
                bail!(
                    "{} isn't an admin of {sdks_repo}, which must get a deploy key for {here}: ask an admin of {sdks_repo} to run `perseid setup` here",
                    cx.login
                );
            }
            let base = default_branch(&info);
            let expected = format!("github:{here}/{spec_path}");
            let existing = match &info {
                Some(_) => bootstrap::read(api, &sdks_repo, &base, config::FILE)?,
                None => None,
            };
            let toml = match existing {
                Some(toml) => {
                    let table: toml::Table = toml.parse()?;
                    match table.get("spec").and_then(|s| s.as_str()) == Some(expected.as_str()) {
                        true => toml,
                        false => set_top(&toml, "spec", &expected),
                    }
                }
                None => seed(&text, &expected, &sdks_repo),
            };
            let snapshot = config::snapshot(&spec_path);
            let received = match &info {
                Some(_) => bootstrap::read(api, &sdks_repo, &base, link::SOURCE)?.is_some(),
                None => false,
            };
            let seeds = match received {
                true => vec![],
                false => {
                    let content = match tracked {
                        true => api.raw(&here, &here_base, &spec_path)?,
                        false => std::fs::read(root.join(file)).ok(),
                    };
                    let sha = bootstrap::head(api, &here, &here_base)?;
                    seeds(&here, &spec_path, sha, content, "", &snapshot)
                }
            };
            unused_folders(&mut plan, &config, &root)?;
            let api_files = vec![File {
                content: text.clone().into_bytes(),
                path: config_file,
                executable: false,
            }];
            plan.link = Some(Link {
                api_repo: here.clone(),
                sdks_repo: sdks_repo.clone(),
            });
            let hub = Hub {
                repo: sdks_repo.clone(),
                info,
                config: toml::from_str(&toml).context("the SDKs repository's perseid.toml")?,
                toml,
                dir: String::new(),
                local: None,
                trigger: Trigger::Received(snapshot.clone()),
                seeds,
            };
            plan_hub(cx, &mut plan, hub, &here_info)?;
            let push = Push {
                branch: &here_base,
                spec: &spec_path,
                generate: config.generate.as_deref(),
                sdks_repo: &sdks_repo,
                snapshot: &snapshot,
            };
            plan_link(
                cx,
                &mut plan,
                &here,
                &here_base,
                &push,
                api_files,
                Some(top.clone()),
            )?;
            plan.diagram = layout::diagram(Some(&here), &sdks_repo, &plan.targets);
            return Ok(plan);
        }
        (
            Source::GitHub {
                repo: api_repo,
                path,
            },
            _,
        ) => {
            let api_info = api.find(&format!("/repos/{api_repo}"))?.with_context(|| {
                format!("{api_repo} doesn't exist or {} can't see it", cx.login)
            })?;
            admin(&api_info, &api_repo, &here, cx.login)?;
            let api_base = bootstrap::default_branch(&api_info);
            let content = api.raw(&api_repo, &api_base, path)?;
            ensure!(
                content.is_some() || config.generate.is_some(),
                "{api_repo} doesn't commit {path}: set `generate` in perseid.toml to the command writing it in its CI"
            );
            let snapshot = join(&dir, &config::snapshot(path));
            let seeds = match link::source(&root) {
                Some(source) if !source.repo.eq_ignore_ascii_case(&api_repo) => bail!(
                    "{} says the spec comes from {}, not {api_repo}: delete it to switch",
                    link::SOURCE,
                    source.repo
                ),
                Some(_) => vec![],
                None => {
                    let sha = bootstrap::head(api, &api_repo, &api_base)?;
                    seeds(&api_repo, path, sha, content, &dir, &snapshot)
                }
            };
            plan.link = Some(Link {
                api_repo: api_repo.clone(),
                sdks_repo: here.clone(),
            });
            let hub = Hub {
                repo: here.clone(),
                info: Some(here_info.clone()),
                config,
                toml: text,
                dir,
                local,
                trigger: Trigger::Received(snapshot.clone()),
                seeds,
            };
            let generate = hub.config.generate.clone();
            plan_hub(cx, &mut plan, hub, &here_info)?;
            let push = Push {
                branch: &api_base,
                spec: path,
                generate: generate.as_deref(),
                sdks_repo: &here,
                snapshot: &snapshot,
            };
            plan_link(cx, &mut plan, &api_repo, &api_base, &push, vec![], None)?;
            plan.diagram = layout::diagram(Some(&api_repo), &here, &plan.targets);
            return Ok(plan);
        }
        (Source::Url(url), _) => {
            let url = url.to_owned();
            unused_folders(&mut plan, &config, &root)?;
            let hub = hub_here(
                &here,
                &here_info,
                config,
                text,
                dir,
                local,
                Trigger::Schedule,
            );
            plan_hub(cx, &mut plan, hub, &here_info)?;
            plan.diagram = layout::diagram(Some(&url), &here, &plan.targets);
            return Ok(plan);
        }
        (Source::File(file), None) => {
            unused_folders(&mut plan, &config, &root)?;
            let paths = vec![join(&dir, file), config_file];
            hub_here(
                &here,
                &here_info,
                config,
                text,
                dir,
                local,
                Trigger::Paths(paths),
            )
        }
    };
    plan_hub(cx, &mut plan, hub, &here_info)?;
    plan.diagram = layout::diagram(None, &here, &plan.targets);
    Ok(plan)
}

/// Warns about the SDK folders `perseid init` wrote here that perseid.toml now sends elsewhere.
fn unused_folders(plan: &mut Plan, config: &Config, root: &Path) -> Result<()> {
    let unused: Vec<String> = config
        .sdks(&[])?
        .iter()
        .filter(|s| config.push_spec.is_some() || s.repo.is_some())
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

fn hub_here(
    here: &str,
    info: &Value,
    config: Config,
    toml: String,
    dir: String,
    local: Option<(PathBuf, PathBuf)>,
    trigger: Trigger,
) -> Hub {
    Hub {
        repo: here.to_owned(),
        info: Some(info.clone()),
        config,
        toml,
        dir,
        local,
        trigger,
        seeds: vec![],
    }
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

/// Setting up the API side takes admin rights on it: a secret and a variable.
fn admin(info: &Value, repo: &str, sdks: &str, login: &str) -> Result<()> {
    if info["permissions"]["admin"] == false {
        bail!(
            "{login} isn't an admin of {repo}, which must get a secret, a variable and a workflow pushing its spec to {sdks}: ask an admin of {repo} to run `perseid setup` here, or in {repo} with `push_spec = \"{sdks}\"` in its perseid.toml"
        );
    }
    Ok(())
}

/// perseid.toml of the SDKs repository, from the API repository's: the same SDKs, receiving `spec`.
fn seed(api_toml: &str, spec: &str, sdks_repo: &str) -> String {
    let mut out = String::new();
    let mut top = true;
    for line in api_toml.lines() {
        top &= !line.trim_start().starts_with('[');
        let key = line.split('=').next().unwrap_or_default().trim();
        let example = [
            "# Where the SDKs live",
            "# See https://github.com/meteroid-oss",
            "# repo =",
            "# push_spec =",
            "# generate =",
        ]
        .iter()
        .any(|e| line.starts_with(e));
        let blank = line.trim().is_empty() && (out.is_empty() || out.ends_with("\n\n"));
        if blank || top && (example || ["push_spec", "repository"].contains(&key)) {
            continue;
        }
        out += line;
        out.push('\n');
    }
    let out = set_top(
        &out,
        "repository",
        &format!("https://github.com/{sdks_repo}"),
    );
    set_top(&out, "spec", spec)
}

/// The snapshot of the spec and its source, as a repository receiving it starts with.
fn seeds(
    repo: &str,
    path: &str,
    sha: Option<String>,
    content: Option<Vec<u8>>,
    dir: &str,
    snapshot: &str,
) -> Vec<File> {
    let source = link::Source {
        repo: repo.to_owned(),
        path: path.to_owned(),
        sha: sha.filter(|_| content.is_some()),
    };
    let mut files = vec![File {
        path: join(dir, link::SOURCE),
        content: source.json().into_bytes(),
        executable: false,
    }];
    if let Some(content) = content {
        files.insert(
            0,
            File {
                path: snapshot.to_owned(),
                content,
                executable: false,
            },
        );
    }
    files
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
        .filter_map(|s| s.repo.clone())
        .filter(|r| !same(r, &hub.repo))
        .collect();
    let local_sdks = sdks
        .iter()
        .any(|s| s.repo.as_deref().is_none_or(|r| same(r, &hub.repo)));
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
            Some(_) if repo != hub.repo || hub.local.is_none() => existing.push(repo),
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
    for repo in &remote {
        let held: Vec<&crate::config::Sdk> = sdks
            .iter()
            .filter(|s| s.repo.as_deref() == Some(repo))
            .collect();
        let base = match created.contains(repo) {
            true => "main".to_owned(),
            false => default_branch(&api.find(&format!("/repos/{repo}"))?),
        };
        let files = bootstrap::release_files(api, &hub.config, repo, &base, &held)?;
        if files.is_empty() {
            plan.add(Mark::Keep, format!("{repo}: release files in place"), None);
            continue;
        }
        let mut updated = Vec::new();
        for file in &files {
            if !created.contains(repo) && api.raw(repo, &base, &file.path)?.is_some() {
                updated.push(file.path.clone());
            }
        }
        let commit = Commit {
            repo: repo.clone(),
            base,
            files,
            message: "ci: release the SDK with release-please",
            pull_request: None,
            local: None,
        };
        plan_commit(cx, plan, commit, &updated)?;
    }

    let received = matches!(hub.trigger, Trigger::Received(_));
    let default_token = received && remote.is_empty();
    let hub_base = default_branch(&hub.info);
    let mut token = None;
    let installed: Vec<String>;
    if default_token {
        installed = vec![];
        let allowed = match hub.info {
            Some(_) => api
                .find(&format!("/repos/{}/actions/permissions/workflow", hub.repo))?
                .is_some_and(|p| p["can_approve_pull_request_reviews"] == true),
            None => false,
        };
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
        installed = configured
            .iter()
            .filter(|r| same(owner_of(r), &owner))
            .cloned()
            .collect();
        let who = account(&owner)?;
        let name = hub.config.name.to_kebab_case();
        plan_app(
            cx,
            plan,
            &hub.repo,
            configured,
            installed.clone(),
            who,
            name,
        )?;
    }
    let names_installed: Vec<String> = installed
        .iter()
        .map(|r| r.rsplit('/').next().unwrap_or(r).to_owned())
        .collect();
    if !default_token {
        token = Some(names_installed);
    }

    let dir = hub.dir.as_str();
    let triggers = match &hub.trigger {
        Trigger::Paths(paths) => paths.clone(),
        Trigger::Schedule => vec![join(dir, config::FILE)],
        Trigger::Received(snapshot) => vec![
            snapshot.clone(),
            join(dir, link::SOURCE),
            SDKS_WORKFLOW.to_owned(),
        ],
    };
    let yaml = workflow(&Workflow {
        branch: &hub_base,
        paths: &triggers,
        app: token.as_ref().map(|repositories| AppToken {
            owner: (!same(&owner, &hub_owner)).then_some(owner.as_str()),
            repositories,
        }),
        dir,
        daily: matches!(hub.trigger, Trigger::Schedule).then_some(hub.repo.as_str()),
        requires: match &hub.trigger {
            Trigger::Received(snapshot) => Some(snapshot.as_str()),
            _ => None,
        },
    });
    let read_remote = |path: &str| -> Result<Option<Vec<u8>>> {
        match hub.info {
            Some(_) => api.raw(&hub.repo, &hub_base, path),
            None => Ok(None),
        }
    };
    let mut candidates = vec![File {
        path: SDKS_WORKFLOW.into(),
        content: yaml.into_bytes(),
        executable: false,
    }];
    candidates.push(File {
        path: join(dir, config::FILE),
        content: hub.toml.clone().into_bytes(),
        executable: false,
    });
    let seeded = match &hub.trigger {
        Trigger::Received(snapshot) => hub
            .seeds
            .iter()
            .find(|f| &f.path == snapshot)
            .map(|f| (snapshot.clone(), f.content.clone())),
        _ => None,
    };
    candidates.extend(hub.seeds);
    match &hub.local {
        Some((top, root)) => {
            let read = |path: &str| Ok(std::fs::read_to_string(root.join(path)).ok());
            let release = root.join("release-please-config.json").exists();
            for (path, content) in crate::init::hub_files(&hub.config, Some(root), release, read)? {
                let path = join(dir, &path);
                candidates.push(File {
                    executable: path.ends_with("gradlew"),
                    path,
                    content,
                });
            }
            for path in super::local_files(top, dir, &hub.config, &sdks)? {
                if candidates.iter().any(|c| c.path == path) {
                    continue;
                }
                let absolute = top.join(&path);
                candidates.push(File {
                    content: std::fs::read(&absolute).with_context(|| format!("reading {path}"))?,
                    executable: super::executable(&absolute),
                    path,
                });
            }
        }
        None => {
            let read = |path: &str| {
                read_remote(path).map(|c| c.map(|c| String::from_utf8_lossy(&c).into_owned()))
            };
            for (path, content) in crate::init::hub_files(&hub.config, None, true, read)? {
                candidates.push(File {
                    executable: path.ends_with("gradlew"),
                    path,
                    content,
                });
            }
        }
    }
    let mut pending = Vec::new();
    let mut updated = Vec::new();
    for file in candidates {
        let remote = read_remote(&file.path)?;
        if remote.as_deref() == Some(file.content.as_slice()) {
            continue;
        }
        if file.path == SDKS_WORKFLOW
            && let Some(remote) = &remote
            && !remote.starts_with(OWNED.as_bytes())
        {
            plan.warnings.push(format!(
                "{} keeps its own {SDKS_WORKFLOW}: check it runs meteroid-oss/perseid@v0 on {}",
                hub.repo,
                triggers.join(", ")
            ));
            continue;
        }
        if remote.is_some() {
            updated.push(file.path.clone());
        }
        pending.push(file);
    }
    let direct = match &hub.info {
        None => true,
        Some(_) => bootstrap::head(api, &hub.repo, &hub_base)?.is_none(),
    };
    let summary = match &hub.trigger {
        Trigger::Paths(_) => format!(
            "when the spec changes on `{hub_base}`, `{SDKS_WORKFLOW}` regenerates the SDKs and opens their pull requests."
        ),
        Trigger::Schedule => format!(
            "every day, `{SDKS_WORKFLOW}` fetches the spec, regenerates the SDKs and opens pull requests when it changed."
        ),
        Trigger::Received(_) => format!(
            "when the spec is pushed here, `{SDKS_WORKFLOW}` regenerates the SDKs and opens their pull requests."
        ),
    };
    let local = hub.local.as_ref().map(|(top, _)| top.clone());
    plan_commit(
        cx,
        plan,
        Commit {
            repo: hub.repo.clone(),
            base: hub_base,
            files: pending,
            message: "ci: generate the SDKs with perseid",
            pull_request: (!direct).then_some(summary),
            local,
        },
        &updated,
    )?;
    if cx.collisions {
        let root = hub.local.as_ref().map(|l| l.1.as_path());
        collisions(cx, plan, &hub.config, &hub.repo, root, seeded)?;
    }
    plan.hub_dir = hub.dir.clone();
    plan.hub = hub.repo;
    plan.hub_config = hub.config;
    Ok(())
}

/// Adds the step committing `commit.files`, unless its pull request already holds them.
fn plan_commit(cx: &Session, plan: &mut Plan, commit: Commit, updated: &[String]) -> Result<()> {
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

fn plan_link(
    cx: &Session,
    plan: &mut Plan,
    api_repo: &str,
    api_base: &str,
    push: &Push,
    mut files: Vec<File>,
    local: Option<PathBuf>,
) -> Result<()> {
    let api = cx.api;
    let sdks_repo = push.sdks_repo;
    let key = link::deploy_key(api, sdks_repo, api_repo)?;
    let secret = secrets::has_secret(api, api_repo, link::SECRET)?;
    let text = format!(
        "{sdks_repo}: write deploy key for {api_repo}, its private half the {} secret of {api_repo}",
        link::SECRET
    );
    match (key, secret) {
        (Some((_, true)), true) => plan.add(Mark::Keep, text, None),
        (stale, _) => plan.add(
            match stale {
                Some(_) => Mark::Change,
                None => Mark::Add,
            },
            text,
            Some(Action::DeployKey {
                api_repo: api_repo.to_owned(),
                sdks_repo: sdks_repo.to_owned(),
                stale: stale.map(|(id, _)| id),
            }),
        ),
    }
    let variable = format!("{api_repo}: variable {} = {sdks_repo}", link::VARIABLE);
    match secrets::variable(api, api_repo, link::VARIABLE)? {
        Some(value) if same(&value, sdks_repo) => plan.add(Mark::Keep, variable, None),
        Some(other) => bail!(
            "{api_repo} already pushes its spec to {other}: `perseid setup` doesn't move it to {sdks_repo}, see https://github.com/meteroid-oss/perseid/blob/main/docs/ci.md#repository-layouts"
        ),
        None => plan.add(
            Mark::Add,
            variable,
            Some(Action::Variable {
                repo: api_repo.to_owned(),
                name: link::VARIABLE,
                value: sdks_repo.to_owned(),
            }),
        ),
    }
    files.insert(
        0,
        File {
            path: PUSH_WORKFLOW.into(),
            content: link::push_workflow(push).into_bytes(),
            executable: false,
        },
    );
    let mut pending = Vec::new();
    let mut updated = Vec::new();
    for file in files {
        let remote = api.raw(api_repo, api_base, &file.path)?;
        if remote.as_deref() == Some(file.content.as_slice()) {
            continue;
        }
        if remote.is_some() {
            updated.push(file.path.clone());
        }
        pending.push(file);
    }
    let summary = format!(
        "when the spec changes on `{api_base}`, `{PUSH_WORKFLOW}` pushes it to {sdks_repo} with the {} deploy key, which can write to that repository only.",
        link::SECRET
    );
    plan_commit(
        cx,
        plan,
        Commit {
            repo: api_repo.to_owned(),
            base: api_base.to_owned(),
            files: pending,
            message: "ci: push the spec to the SDKs repository",
            pull_request: Some(summary),
            local,
        },
        &updated,
    )
}

/// Warns about generated paths taken by files perseid didn't generate.
fn collisions(
    cx: &Session,
    plan: &mut Plan,
    config: &Config,
    hub: &str,
    root: Option<&Path>,
    seeded: Option<(String, Vec<u8>)>,
) -> Result<()> {
    let spec = match (config.source(), root) {
        (Source::GitHub { .. }, _) if seeded.is_some() => {
            let (path, content) = seeded.unwrap_or_default();
            let name = Path::new(&path).file_name().unwrap_or_default().to_owned();
            let dir = tempfile::tempdir()?;
            std::fs::write(dir.path().join(&name), content)?;
            crate::spec::read(&name.to_string_lossy(), dir.path()).ok()
        }
        (Source::GitHub { .. }, None) => return Ok(()),
        (_, Some(root)) => crate::generate::load_spec(config, root).ok(),
        (_, None) => crate::generate::load_spec(config, Path::new(".")).ok(),
    };
    let Some(spec) = spec else {
        return Ok(());
    };
    let render_root = root.map_or_else(|| PathBuf::from("/nonexistent"), Path::to_path_buf);
    for sdk in config.sdks(&[])? {
        let planned = crate::generate::planned(config, &render_root, &sdk, &spec)?;
        let repo = sdk.repo.clone().unwrap_or_else(|| hub.to_owned());
        let mut taken = Vec::new();
        match (&sdk.repo, root) {
            (None, Some(root)) => {
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
            Action::DeployKey {
                api_repo,
                sdks_repo,
                stale,
            } => link::add_deploy_key(api, &api_repo, &sdks_repo, stale, ui)?,
            Action::Variable { repo, name, value } => {
                secrets::set_variable(api, &repo, name, &value)?;
                ui.ok(&format!("{repo}: variable {name} = {value}"));
            }
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
        local,
    } = commit;
    if let Some(top) = &local {
        for file in &files {
            let target = top.join(&file.path);
            if std::fs::read(&target).ok().as_deref() != Some(file.content.as_slice()) {
                crate::fsx::write(&target, &file.content)?;
            }
        }
    }
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
            let body = format!(
                "Set up by `perseid setup`: {summary}\n\n{}",
                list.join("\n")
            );
            let (url, opened) = bootstrap::pull_request(api, &repo, &base, branch, message, &body)?;
            match opened {
                true => ui.ok(&format!("Opened {url} with {}", names(&paths))),
                false => ui.ok(&format!("Updated {url} with {}", names(&paths))),
            }
            Some(url)
        }
    };
    if let Some(top) = &local
        && url.is_some()
    {
        super::stage(top, &paths)?;
    }
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
