//! `perseid connect`: in the repository holding the spec, the workflow pushing it to the SDKs
//! repository, and what it pushes with: the hosted perseid App, the SDKs' GitHub App, or a token.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::{
    Options, Ui,
    api::GitHub,
    app::{self, APP_ID, APP_KEY},
    auth,
    bootstrap::{self, File},
    git, hosted, join, layout,
    link::{self, Auth, Push, PushOn, Pushed},
    plan::{self, Action, Mark, Plan, Session, TOKEN},
    secrets, toplevel,
};
use crate::config::{self, Config, Source};

pub struct Connect {
    pub hub: String,
    pub spec: Option<String>,
    pub build: Option<String>,
    pub on: Option<PushOn>,
    pub tags: Option<String>,
    pub auth: Option<Auth>,
    pub private: bool,
}

/// The repository holding the spec, and how its workflow pushes it.
pub(super) struct Settings {
    pub here: String,
    pub top: PathBuf,
    pub pushed: Pushed,
}

pub fn connect(cwd: &Path, connect: Connect, options: &Options) -> Result<ExitCode> {
    let ui = Ui::new(options);
    let here = layout::required_origin_repo(cwd, "the repository holding the spec")?;
    let top = toplevel(cwd)?;
    let hub = connect.hub.trim_end_matches(".git").to_owned();
    ensure!(
        hub.split('/').count() == 2 && !hub.starts_with('/') && !hub.ends_with('/'),
        "`{hub}` must read owner/name: the repository holding perseid.toml"
    );
    ensure!(
        !hub.eq_ignore_ascii_case(&here),
        "{here} holds perseid.toml: its sdks.yml regenerates the SDKs when the spec changes, nothing to connect"
    );
    let existing = std::fs::read_to_string(top.join(link::WORKFLOW))
        .ok()
        .and_then(|yaml| link::pushed(&yaml))
        .filter(|p| p.hub.eq_ignore_ascii_case(&hub));
    let interactive = std::io::stdin().is_terminal() && !options.yes;
    let spec = match (&connect.spec, &existing) {
        (Some(spec), _) => in_repository(cwd, &top, spec)?,
        (None, Some(pushed)) => pushed.spec.clone(),
        (None, None) => crate::init::pick_spec(&top, interactive)?.with_context(|| {
            format!(
                "no OpenAPI spec found in {here}: pass --spec <path>, and --build '<command>' when CI writes it"
            )
        })?,
    };
    let build = connect
        .build
        .clone()
        .or_else(|| existing.as_ref().and_then(|p| p.build.clone()));
    ensure!(
        top.join(&spec).exists() || build.is_some(),
        "{spec} doesn't exist in {here}: pass --spec <path>, and --build '<command>' when CI writes it"
    );
    let tracked = git(&top, &["ls-files", "--error-unmatch", "--", &spec]).is_ok();
    ensure!(
        tracked || build.is_some(),
        "{spec} isn't committed: pass --build '<command writing it>', which the workflow runs before pushing it"
    );

    let (token, source) = auth::token(&ui)?;
    let api = GitHub::new(Some(token));
    let login = auth::whoami(&api)?;
    ui.ok(&format!("Signed in to GitHub as {login} ({source})"));
    let on = match (connect.on, &existing) {
        (Some(on), _) => on,
        (None, Some(pushed)) => pushed.on,
        (None, None) => {
            let proposed = proposed(&api, &here)?;
            match interactive {
                true => ask_on(proposed)?,
                false => proposed,
            }
        }
    };
    let auth = match (connect.auth, &existing) {
        (Some(auth), _) => auth,
        (None, Some(pushed)) => pushed.auth,
        (None, None) => match secrets::variable(&api, &hub, APP_ID).ok().flatten() {
            Some(_) => Auth::App,
            None => Auth::Perseid,
        },
    };
    let tags = connect
        .tags
        .clone()
        .or_else(|| existing.as_ref().and_then(|p| p.tags.clone()))
        .filter(|_| on == PushOn::Tag);
    let settings = Settings {
        here,
        top,
        pushed: Pushed {
            hub,
            on,
            tags,
            spec,
            build,
            auth,
            private: connect.private || existing.as_ref().is_some_and(|p| p.private),
        },
    };
    let cx = Session {
        api: &api,
        login: &login,
        app: true,
        collisions: false,
    };
    let mut plan = plan_connect(&cx, &settings)?;
    let Pushed { hub, on, .. } = &settings.pushed;
    let here = &settings.here;
    println!("\n{}\n", plan.diagram);
    ui.say(&match settings.pushed.auth {
        Auth::Perseid => format!(
            "perseid-push.yml pushes as the perseid App, installed on {hub}, whose perseid.toml names {here} as its source.\nNothing is stored in {here}, and nothing gets access to it. `--auth app` or `--auth token` push otherwise."
        ),
        Auth::App => format!(
            "perseid-push.yml pushes as the GitHub App of {hub}, with a token for {hub} only minted on each run.\nThe App's key, stored in {here}, reaches every repository the App is installed on. `--auth token` uses a token instead."
        ),
        Auth::Token => format!(
            "perseid-push.yml pushes with a fine-grained token, the {TOKEN} secret of {here}.\n`perseid app` in {hub}, then `--auth app` here, pushes as its GitHub App instead: nothing expires."
        ),
    });
    println!();
    plan.print();
    if plan.pending() == 0 {
        println!();
        ui.ok("In sync: nothing to change");
        return Ok(ExitCode::SUCCESS);
    }
    if options.dry_run {
        return Ok(ExitCode::from(2));
    }
    let on_github = |s: &plan::Step| {
        s.action
            .as_ref()
            .is_some_and(|a| !matches!(a, Action::Write { .. }))
    };
    let remote = plan.steps.iter().any(on_github);
    let question = match settings.pushed.auth {
        Auth::Perseid => format!("Open the pull request on {hub}?"),
        _ => format!("Store the credentials on {here}?"),
    };
    let consent = !remote || ui.confirm(&question, true)?;
    let manual = plan.manual();
    if !consent {
        plan.steps.retain(|s| !on_github(s));
    }
    println!();
    plan::apply(&api, &mut plan, &ui)?;
    if !consent {
        ui.say("Nothing changed on GitHub. To do it yourself:");
        for step in manual.iter().filter(|m| !m.starts_with("write ")) {
            ui.info(step);
        }
    }
    println!("\nNext steps");
    let mut steps = vec![format!(
        "Review {}, then commit and push it to the default branch",
        link::WORKFLOW
    )];
    if settings.pushed.auth == Auth::Perseid {
        steps.push(format!(
            "Merge the pull request adding `source` to the perseid.toml of {hub}"
        ));
    }
    steps.push(format!(
        "The spec then reaches {hub} {}: run the Spec workflow (`gh workflow run perseid-push.yml`) to push it now",
        when(*on)
    ));
    for (n, step) in steps.iter().enumerate() {
        ui.info(&format!("{}. {step}", n + 1));
    }
    Ok(ExitCode::SUCCESS)
}

fn when(on: PushOn) -> &'static str {
    match on {
        PushOn::Change => "on each change",
        PushOn::Release => "on each published release",
        PushOn::Tag => "on each matching tag",
    }
}

/// `spec`, given relative to `cwd`, relative to the repository's `top`.
fn in_repository(cwd: &Path, top: &Path, spec: &str) -> Result<String> {
    let absolute = std::path::absolute(cwd.join(spec))?;
    let top = top.canonicalize()?;
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let relative = absolute
        .strip_prefix(&top)
        .with_context(|| format!("{spec} is outside the repository"))?;
    Ok(relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// `release` for a repository publishing GitHub releases, `change` otherwise.
fn proposed(api: &GitHub, repo: &str) -> Result<PushOn> {
    let releases = api.find(&format!("/repos/{repo}/releases?per_page=1"))?;
    Ok(
        match releases.and_then(|r| r.as_array().map(|r| !r.is_empty())) {
            Some(true) => PushOn::Release,
            _ => PushOn::Change,
        },
    )
}

fn ask_on(proposed: PushOn) -> Result<PushOn> {
    let options = [PushOn::Release, PushOn::Change, PushOn::Tag];
    let labels: Vec<String> = options
        .iter()
        .map(|o| match o {
            PushOn::Release => "on each GitHub release".to_owned(),
            PushOn::Change => "on each change to the default branch".to_owned(),
            PushOn::Tag => "on each tag".to_owned(),
        })
        .collect();
    let default = options.iter().position(|o| *o == proposed).unwrap_or(0);
    let picked = crate::prompt::pick_one("Push the spec", &labels, default)?;
    Ok(options[picked])
}

/// The perseid.toml of `hub`, its path and text: at the root, or the only one.
fn hub_config(api: &GitHub, hub: &str, branch: &str) -> Result<Option<(Config, String, String)>> {
    let path = match bootstrap::read(api, hub, branch, config::FILE)? {
        Some(_) => config::FILE.to_owned(),
        None => {
            let suffix = format!("/{}", config::FILE);
            let found: Vec<String> = bootstrap::paths(api, hub, branch)?
                .into_iter()
                .filter(|p| p.ends_with(&suffix))
                .collect();
            match found.as_slice() {
                [one] => one.clone(),
                [] => return Ok(None),
                many => bail!("{hub} holds several {}: {}", config::FILE, many.join(", ")),
            }
        }
    };
    let text = bootstrap::read(api, hub, branch, &path)?.unwrap_or_default();
    let mut config = Config::parse(&text, &format!("{hub}/{path}"))?;
    config.home = config::Home::github(hub, &path);
    Ok(Some((config, path, text)))
}

fn admin(info: &Value) -> bool {
    info["permissions"]["admin"] != false
}

/// What `settings` lacks to push the spec: the credential stored on GitHub, and the
/// workflow here, which the user commits.
pub(super) fn plan_connect(cx: &Session, settings: &Settings) -> Result<Plan> {
    let api = cx.api;
    let Settings { here, top, pushed } = settings;
    let hub = pushed.hub.as_str();
    let here_info = api.get(&format!("/repos/{here}"))?;
    ensure!(
        pushed.auth == Auth::Perseid || admin(&here_info),
        "{} isn't an admin of {here}, which stores the credentials: ask an admin of {here} to run `npx perseid connect {hub}` there",
        cx.login
    );
    let hub_info = api.find(&format!("/repos/{hub}"))?.with_context(|| {
        format!(
            "{hub} doesn't exist or {} can't see it: create it, run `npx perseid init` there and push perseid.toml",
            cx.login
        )
    })?;
    let hub_base = bootstrap::default_branch(&hub_info);
    let mut plan = Plan::new();
    plan.hub = hub.to_owned();
    let found = hub_config(api, hub, &hub_base)?;
    match &found {
        Some((config, config_path, _)) => {
            let dir = config_path
                .strip_suffix(config::FILE)
                .unwrap_or_default()
                .trim_end_matches('/');
            let Source::File(file) = config.source() else {
                bail!(
                    "{hub}/{config_path} reads its spec from {}: set its `spec` to a file, which this repository then pushes",
                    config.spec
                );
            };
            plan.hub_dir = dir.to_owned();
            plan.diagram = format!("{here} ──spec──▶ {hub} ({})", join(dir, file));
        }
        None => {
            plan.diagram = format!("{here} ──spec──▶ {hub}");
            plan.warnings.push(format!(
                "{hub} has no {} on `{hub_base}` yet: pushes skip until it does. Run `npx perseid init` there, then commit and push",
                config::FILE
            ));
        }
    }
    if bootstrap::read(api, hub, &hub_base, layout::SDKS_WORKFLOW)?.is_none() {
        plan.warnings.push(format!(
            "{hub} doesn't regenerate its SDKs yet: its sdks.yml, which `npx perseid init` writes there, isn't on `{hub_base}`"
        ));
    }

    match pushed.auth {
        Auth::Perseid => plan_source(api, &mut plan, &here_info, &hub_info, found, pushed.private)?,
        Auth::App => plan_app(api, &mut plan, here, hub)?,
        Auth::Token => plan_token(api, &mut plan, here, hub)?,
    }

    let here_base = bootstrap::default_branch(&here_info);
    let yaml = link::push_workflow(&Push {
        branch: &here_base,
        on: pushed.on,
        tags: pushed.tags.as_deref().unwrap_or("v*"),
        spec: &pushed.spec,
        build: pushed.build.as_deref(),
        hub,
        auth: pushed.auth,
        private: pushed.private,
    });
    let local = std::fs::read(top.join(link::WORKFLOW)).ok();
    match local.as_deref() == Some(yaml.as_bytes()) {
        true => plan.add(
            Mark::Keep,
            format!("{} is up to date here", link::WORKFLOW),
            None,
        ),
        false => plan.add(
            match local {
                Some(_) => Mark::Change,
                None => Mark::Add,
            },
            format!("{}, written here for you to commit", link::WORKFLOW),
            Some(Action::Write {
                top: top.clone(),
                file: File {
                    path: link::WORKFLOW.into(),
                    content: yaml.clone().into_bytes(),
                },
            }),
        ),
    }
    if api.raw(here, &here_base, link::WORKFLOW)?.as_deref() != Some(yaml.as_bytes()) {
        plan.warnings.push(format!(
            "`{here_base}` of {here} doesn't hold this {} yet: commit and push it",
            link::WORKFLOW
        ));
    }
    Ok(plan)
}

/// The `source` of the hub's perseid.toml naming `here`, added through a pull request: the
/// hosted perseid App then lets `here` push there, and nothing else.
fn plan_source(
    api: &GitHub,
    plan: &mut Plan,
    here_info: &Value,
    hub_info: &Value,
    found: Option<(Config, String, String)>,
    private: bool,
) -> Result<()> {
    let here = here_info["full_name"].as_str().unwrap_or_default();
    let hub = hub_info["full_name"].as_str().unwrap_or_default();
    let (Some(id), Some(target_id)) = (here_info["id"].as_u64(), hub_info["id"].as_u64()) else {
        bail!("GitHub sent no id for {here} or {hub}");
    };
    if hosted::missing(api, &[hub.to_owned()])?.is_some_and(|m| !m.is_empty()) {
        plan.warnings.push(format!(
            "the perseid App isn't installed on {hub}: run `npx perseid sync` in a clone of it"
        ));
        plan.attention = true;
    }
    let Some((config, path, text)) = found else {
        plan.warnings.push(format!(
            "run `perseid connect {hub}` again once perseid.toml is there: it adds its `source`"
        ));
        return Ok(());
    };
    let line = hosted::source_line(here, id, target_id, private);
    let named = config
        .pushed_from
        .as_ref()
        .is_some_and(|s| s.id == id && s.target_id == target_id);
    if named {
        plan.add(
            Mark::Keep,
            format!("{hub}/{path} names {here} as its source"),
            None,
        );
        return Ok(());
    }
    if hub_info["permissions"]["push"] == false {
        plan.warnings.push(format!(
            "ask a maintainer of {hub} to add to its {path}, at the top level: {line}"
        ));
        plan.attention = true;
        return Ok(());
    }
    plan.add(
        match config.pushed_from {
            Some(_) => Mark::Change,
            None => Mark::Add,
        },
        format!("{hub}/{path}: `{line}`, through a pull request"),
        Some(Action::Commit(Box::new(hosted::Commit {
            repo: hub.to_owned(),
            base: bootstrap::default_branch(hub_info),
            path: path.clone(),
            content: hosted::with_source(&text, &line).into_bytes(),
            message: match private {
                true => "ci: let the API repository push the spec".to_owned(),
                false => format!("ci: let {here} push the spec"),
            },
            branch: "perseid/source".to_owned(),
            body: format!(
                "Written by `perseid connect`: the perseid App lets the repository of id {id} push the spec here, with Contents write on this repository only. A copy of this file elsewhere grants nothing: `target_id` is this repository's id."
            ),
            direct: false,
        }))),
    );
    Ok(())
}

/// The ID and a key of the GitHub App of `hub` in `here`: the ID copied, the key new.
fn plan_app(api: &GitHub, plan: &mut Plan, here: &str, hub: &str) -> Result<()> {
    let Some(id) = secrets::variable(api, hub, APP_ID)
        .with_context(|| format!("reading the {APP_ID} variable of {hub}"))?
    else {
        bail!(
            "{hub} has no GitHub App yet: run `perseid app` in a clone of {hub} first, or pass --auth token"
        );
    };
    let slug = secrets::variable(api, hub, "SDK_APP_SLUG")?;
    let name = slug.clone().unwrap_or_else(|| id.clone());
    let current = secrets::variable(api, here, APP_ID)?;
    let same = current.as_deref() == Some(id.as_str());
    let variable = format!("{here}: {APP_ID} variable, the ID of the GitHub App {name}");
    match (same, current) {
        (true, _) => plan.add(Mark::Keep, variable, None),
        (false, current) => plan.add(
            match current {
                Some(_) => Mark::Change,
                None => Mark::Add,
            },
            variable,
            Some(Action::Variable {
                repo: here.to_owned(),
                name: APP_ID,
                value: id.clone(),
            }),
        ),
    }
    let key = format!("{here}: {APP_KEY} secret");
    match same && secrets::has_secret(api, here, APP_KEY)? {
        true => plan.add(Mark::Keep, key, None),
        false => {
            let owner = app::owner(api, hub.split('/').next().unwrap_or(hub))?;
            plan.add(
                Mark::Add,
                format!("{key}, a new key of the App, generated in your browser"),
                Some(Action::AppKey(Box::new(app::NewKey {
                    id,
                    slug,
                    owner,
                    repos: vec![here.to_owned()],
                }))),
            );
        }
    }
    Ok(())
}

/// The fine-grained token reaching `hub`, as the SDK_GITHUB_TOKEN secret of `here`.
fn plan_token(api: &GitHub, plan: &mut Plan, here: &str, hub: &str) -> Result<()> {
    match secrets::has_secret(api, here, TOKEN)? {
        true => plan.add(Mark::Keep, format!("{here}: {TOKEN} secret"), None),
        false => plan.add(
            Mark::Add,
            format!(
                "{here}: {TOKEN} secret, a fine-grained token you paste, with Contents read and write on {hub}"
            ),
            Some(Action::Token {
                repo: here.to_owned(),
                hub: hub.to_owned(),
            }),
        ),
    }
    Ok(())
}
