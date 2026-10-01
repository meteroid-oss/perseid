//! `perseid connect`: in the repository holding the spec, the workflow pushing it to the SDKs
//! repository, with the deploy key that lets it.

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
    auth,
    bootstrap::{self, File},
    git, join, layout,
    link::{self, Push, PushOn, Pushed},
    plan::{self, Action, Commit, Mark, Plan, Session},
    secrets, toplevel,
};
use crate::config::{self, Config, Source};

pub struct Connect {
    pub hub: String,
    pub spec: Option<String>,
    pub build: Option<String>,
    pub on: Option<PushOn>,
    pub tags: Option<String>,
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
        "{here} holds perseid.toml: `perseid setup` there regenerates the SDKs when the spec changes, nothing to connect"
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
    let login = auth::whoami(&api, source)?;
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
            private: connect.private || existing.as_ref().is_some_and(|p| p.private),
        },
    };
    let cx = Session {
        api: &api,
        login: &login,
        collisions: false,
    };
    let mut plan = plan_connect(&cx, &settings)?;
    println!("\n{}\n", plan.diagram);
    plan.print();
    let pending = plan.pending();
    if pending == 0 {
        println!();
        ui.ok("In sync: nothing to change");
        return Ok(ExitCode::SUCCESS);
    }
    if options.dry_run {
        return Ok(ExitCode::from(2));
    }
    let question = match pending {
        1 => "Apply this change?".to_owned(),
        n => format!("Apply these {n} changes?"),
    };
    if !ui.confirm(&question, true)? {
        bail!("nothing changed");
    }
    println!();
    let pulls = plan::apply(&api, &mut plan, &ui)?;
    println!("\n✓ {}", plan.diagram);
    println!("\nNext steps");
    let mut step = 0;
    let mut item = |text: String| {
        step += 1;
        ui.info(&format!("{step}. {text}"));
    };
    for pull in &pulls {
        item(format!(
            "Merge {pull}, then `git pull`: perseid staged {}",
            link::WORKFLOW
        ));
    }
    let Pushed { hub, on, .. } = &settings.pushed;
    item(format!(
        "The spec reaches {hub} {}: run the Spec workflow (`gh workflow run perseid-push.yml`) to push it now",
        when(*on)
    ));
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

/// The perseid.toml of `hub`, and its path: at the root, or the only one.
fn hub_config(api: &GitHub, hub: &str, branch: &str) -> Result<(Config, String)> {
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
                [] => match bootstrap::open_pull(api, hub, super::SETUP_BRANCH)? {
                    Some(url) => bail!(
                        "{hub}'s {} waits in {url}: merge it, then run this again",
                        config::FILE
                    ),
                    None => bail!(
                        "{hub} has no {} on `{branch}`: run `npx perseid init` there and push it, then run this again",
                        config::FILE
                    ),
                },
                many => bail!("{hub} holds several {}: {}", config::FILE, many.join(", ")),
            }
        }
    };
    let text = bootstrap::read(api, hub, branch, &path)?.unwrap_or_default();
    Ok((Config::parse(&text, &format!("{hub}/{path}"))?, path))
}

fn admin(info: &Value) -> bool {
    info["permissions"]["admin"] != false
}

/// What GitHub lacks for `settings` to push the spec: the deploy key and its secret, the
/// variable naming the SDKs repository, and the workflow.
pub(super) fn plan_connect(cx: &Session, settings: &Settings) -> Result<Plan> {
    let api = cx.api;
    let Settings { here, top, pushed } = settings;
    let hub = pushed.hub.as_str();
    let here_info = api.get(&format!("/repos/{here}"))?;
    ensure!(
        admin(&here_info),
        "{} isn't an admin of {here}, which gets the {} secret and the {} variable: ask an admin of {here} to run `npx perseid connect {hub}` there",
        cx.login,
        link::SECRET,
        link::VARIABLE
    );
    let hub_info = api.find(&format!("/repos/{hub}"))?.with_context(|| {
        format!(
            "{hub} doesn't exist or {} can't see it: create it, run `npx perseid init` there and push perseid.toml",
            cx.login
        )
    })?;
    let hub_base = bootstrap::default_branch(&hub_info);
    let (config, config_path) = hub_config(api, hub, &hub_base)?;
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
    let destination = join(dir, file);
    let mut plan = Plan::new();
    plan.hub = hub.to_owned();
    plan.hub_dir = dir.to_owned();
    plan.diagram = format!("{here} ──spec──▶ {hub} ({destination})");

    let secret = secrets::has_secret(api, here, link::SECRET)?;
    let text = format!(
        "{hub}: write deploy key for {here}, its private half the {} secret of {here}",
        link::SECRET
    );
    let action = |stale, register| {
        Some(Action::DeployKey {
            api_repo: here.clone(),
            sdks_repo: hub.to_owned(),
            register: (register, stale),
        })
    };
    match admin(&hub_info) {
        true => match (link::deploy_key(api, hub, here)?, secret) {
            (Some((_, true)), true) => plan.add(Mark::Keep, text, None),
            (Some((id, _)), _) => plan.add(Mark::Change, text, action(Some(id), true)),
            (None, _) => plan.add(Mark::Add, text, action(None, true)),
        },
        false if secret => {
            plan.add(
                Mark::Keep,
                format!("{here}: {} secret set", link::SECRET),
                None,
            );
            plan.warnings.push(format!(
                "{} can't see the deploy keys of {hub}: check with one of its admins that the key titled \"{}\" can write",
                cx.login,
                link::key_title(here)
            ));
        }
        false => plan.add(
            Mark::Add,
            format!(
                "{here}: deploy key in the {} secret, whose public half an admin of {hub} adds",
                link::SECRET
            ),
            action(None, false),
        ),
    }

    let variable = format!("{here}: variable {} = {hub}", link::VARIABLE);
    match secrets::variable(api, here, link::VARIABLE)? {
        Some(value) if value.eq_ignore_ascii_case(hub) => plan.add(Mark::Keep, variable, None),
        current => plan.add(
            match current {
                Some(_) => Mark::Change,
                None => Mark::Add,
            },
            variable,
            Some(Action::Variable {
                repo: here.clone(),
                name: link::VARIABLE,
                value: hub.to_owned(),
            }),
        ),
    }

    let here_base = bootstrap::default_branch(&here_info);
    let yaml = link::push_workflow(&Push {
        branch: &here_base,
        on: pushed.on,
        tags: pushed.tags.as_deref().unwrap_or("v*"),
        spec: &pushed.spec,
        build: pushed.build.as_deref(),
        hub,
        destination: &destination,
        config: &config_path,
        private: pushed.private,
    });
    let remote = api.raw(here, &here_base, link::WORKFLOW)?;
    let files = match remote.as_deref() == Some(yaml.as_bytes()) {
        true => vec![],
        false => vec![File {
            path: link::WORKFLOW.into(),
            content: yaml.into_bytes(),
            executable: false,
        }],
    };
    let updated = match remote {
        Some(_) => vec![link::WORKFLOW.to_owned()],
        None => vec![],
    };
    let summary = format!(
        "{}, `{}` pushes {} to {hub} with the {} deploy key, which can write to that repository only.",
        when(pushed.on),
        link::WORKFLOW,
        pushed.spec,
        link::SECRET
    );
    plan::plan_commit(
        cx,
        &mut plan,
        Commit {
            repo: here.clone(),
            base: here_base,
            files,
            message: "ci: push the spec to the SDKs repository",
            pull_request: Some(summary),
            local: Some(top.clone()),
        },
        &updated,
    )?;
    Ok(plan)
}
