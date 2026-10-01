//! `perseid status`: the plan `perseid setup` would apply, and how the automation fares.

use std::{
    collections::BTreeSet,
    path::Path,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde_json::Value;

use super::{
    Options, Ui,
    api::GitHub,
    auth, git, layout,
    link::{self, SOURCE},
    plan::{self, Mark, PUSH_WORKFLOW},
    secrets, toplevel,
};
use crate::config::{self, Config, PushOn, Source};

struct Report {
    ui: Ui,
    failed: bool,
}

impl Report {
    fn fail(&mut self, message: &str, fix: &str) {
        self.ui.fail(message);
        self.ui.say(fix);
        self.failed = true;
    }
}

pub fn status(config_path: &Path) -> Result<ExitCode> {
    let ui = Ui::new(&Options {
        yes: true,
        dry_run: true,
        browser: false,
    });
    let mut report = Report { ui, failed: false };
    let (config, root) = match Config::load(config_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            report.fail(
                &format!("{error:#}"),
                "fix perseid.toml, or run `perseid init`",
            );
            return Ok(ExitCode::FAILURE);
        }
    };
    let here = layout::spec_repo(&root);
    let session = auth::stored_token().and_then(|(token, source)| {
        let api = GitHub::new(Some(token));
        let user = api.get("/user").ok()?;
        Some((api, user["login"].as_str()?.to_owned(), source))
    });
    let planned = match (&session, &here) {
        (Some((api, login, _)), Some(_)) => {
            let cx = plan::Session {
                api,
                login,
                collisions: false,
            };
            Some(plan::plan(&cx, config_path))
        }
        _ => None,
    };
    let diagram = match &planned {
        Some(Ok(plan)) => plan.diagram.clone(),
        _ => local_diagram(&config, here.as_deref().unwrap_or("this repository"))?,
    };
    println!("{diagram}\n");
    report.ui.ok("perseid.toml is valid");
    local(&mut report, &config, &root);
    let (Some((api, login, source)), Some(_)) = (&session, &here) else {
        match here {
            None => report
                .ui
                .warn("origin isn't a GitHub repository: only local checks ran"),
            Some(_) => {
                report
                    .ui
                    .warn("Not signed in to GitHub: only local checks ran");
                report
                    .ui
                    .say("set GH_TOKEN, or run `gh auth login`, to check GitHub too");
            }
        }
        return Ok(exit(&report));
    };
    report
        .ui
        .ok(&format!("Signed in to GitHub as {login} ({source})"));
    let plan = match planned {
        Some(Ok(plan)) => plan,
        Some(Err(error)) => {
            report.fail(
                &format!("{error:#}"),
                "fix what it says, then run `perseid setup`",
            );
            return Ok(exit(&report));
        }
        None => return Ok(exit(&report)),
    };
    match plan.pending() {
        0 => report.ui.ok("In sync with perseid.toml"),
        n => {
            report.ui.fail(&format!(
                "{n} change{} pending:",
                if n == 1 { "" } else { "s" }
            ));
            for step in plan.steps.iter().filter(|s| s.mark != Mark::Keep) {
                let mark = if step.mark == Mark::Add { '+' } else { '~' };
                println!("    {mark} {}", step.text);
            }
            report.ui.say("run `perseid setup`");
            report.failed = true;
        }
    }
    for warning in &plan.warnings {
        report.ui.warn(warning);
    }
    health(&mut report, api, &plan)?;
    Ok(exit(&report))
}

fn exit(report: &Report) -> ExitCode {
    match report.failed {
        true => ExitCode::FAILURE,
        false => ExitCode::SUCCESS,
    }
}

fn local_diagram(config: &Config, here: &str) -> Result<String> {
    let sdks = config.sdks(&[])?;
    let targets = layout::targets(&sdks, here);
    Ok(match (config.source(), &config.sdks_repo) {
        (_, Some(sdks_repo)) => layout::diagram(Some(here), sdks_repo, &[]),
        (Source::GitHub { repo, .. }, _) => layout::diagram(Some(&repo), here, &targets),
        (Source::Url(url), _) => layout::diagram(Some(url), here, &targets),
        (Source::File(_), None) => layout::diagram(None, here, &targets),
    })
}

/// What this checkout tells without GitHub: the spec, the workflow, the last sync.
fn local(report: &mut Report, config: &Config, root: &Path) {
    let top = toplevel(root).unwrap_or_else(|_| root.to_owned());
    let workflow = match config.sdks_repo {
        Some(_) => PUSH_WORKFLOW,
        None => layout::SDKS_WORKFLOW,
    };
    match top.join(workflow).exists() {
        true => report.ui.ok(&format!("{workflow} is here")),
        false => report.fail(
            &format!("{workflow} is missing"),
            "run `perseid setup`, or merge and pull its perseid/setup pull request",
        ),
    }
    match config.source() {
        Source::File(file) if !root.join(file).exists() && config.generate.is_none() => report
            .fail(
                &format!("the spec {file} is missing"),
                "point `spec` of perseid.toml to it",
            ),
        Source::GitHub { repo, path } => {
            let snapshot = config::snapshot(path);
            if !root.join(&snapshot).exists() {
                report.ui.warn(&format!(
                    "{snapshot} is missing: {repo} hasn't pushed its spec here yet"
                ));
            }
            if let Some(source) = link::source(root) {
                let age = git(root, &["log", "-1", "--format=%ct", "--", SOURCE])
                    .ok()
                    .and_then(|out| String::from_utf8(out).ok()?.trim().parse::<i64>().ok());
                synced(report, &source, age);
            }
        }
        _ => {}
    }
}

fn synced(report: &mut Report, source: &link::Source, when: Option<i64>) {
    let Some(sha) = &source.sha else {
        report
            .ui
            .warn(&format!("{} hasn't pushed its spec here yet", source.repo));
        return;
    };
    let age = when.map_or_else(String::new, |t| format!(", {}", ago(t)));
    let short = sha.get(..7).unwrap_or(sha);
    let at = match &source.tag {
        Some(tag) => format!("{tag} ({short})"),
        None => short.to_owned(),
    };
    report
        .ui
        .ok(&format!("Last synced {}@{at}{age}", source.repo));
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn ago(time: i64) -> String {
    let seconds = (now() - time).max(0);
    let (value, unit) = match seconds {
        s if s < 120 => return "just now".to_owned(),
        s if s < 7200 => (s / 60, "minute"),
        s if s < 172_800 => (s / 3600, "hour"),
        s => (s / 86_400, "day"),
    };
    format!("{value} {unit}s ago")
}

/// Seconds since the epoch of a GitHub timestamp, `2026-09-30T12:00:00Z`.
fn timestamp(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hh, mm, ss) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let year = y - era * 400;
    let day = (153 * m + 2) / 5 + d - 1;
    let days = era * 146_097 + year * 365 + year / 4 - year / 100 + day - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn health(report: &mut Report, api: &GitHub, plan: &plan::Plan) -> Result<()> {
    let config = plan.config();
    if plan.link.is_some() {
        let info = api.find(&format!("/repos/{}", plan.hub))?;
        let branch = info
            .as_ref()
            .map_or_else(|| "main".to_owned(), super::bootstrap::default_branch);
        let file = super::join(&plan.hub_dir, SOURCE);
        if let Some(text) = super::bootstrap::read(api, &plan.hub, &branch, &file)?
            && let Ok(source) = serde_json::from_str::<link::Source>(&text)
        {
            let commits = api
                .find(&format!(
                    "/repos/{}/commits?path={file}&per_page=1",
                    plan.hub
                ))?
                .unwrap_or_default();
            let when = commits[0]["commit"]["committer"]["date"]
                .as_str()
                .and_then(timestamp);
            synced(report, &source, when);
            let on = plan.link.as_ref().map(|l| l.on).unwrap_or_default();
            behind(report, api, &source, on)?;
        }
    }
    let mut repos = BTreeSet::new();
    for sdk in config.sdks(&[])? {
        repos.insert(sdk.repo.unwrap_or_else(|| plan.hub.clone()));
    }
    for repo in &repos {
        let owner = repo.split('/').next().unwrap_or_default();
        let open = api
            .find(&format!(
                "/repos/{repo}/pulls?head={owner}:{}&state=open",
                crate::pr::BRANCH
            ))?
            .unwrap_or_default();
        match open[0]["html_url"].as_str() {
            Some(url) => report.ui.warn(&format!(
                "{repo}: SDK pull request waiting for review, {url}"
            )),
            None => report
                .ui
                .ok(&format!("{repo}: no SDK pull request waiting")),
        }
    }
    if plan.app {
        installation(report, api, &plan.hub)?;
    }
    run(report, api, &plan.hub, "sdks.yml")?;
    if let Some(link) = &plan.link {
        run(report, api, &link.api_repo, "perseid-push.yml")?;
    }
    Ok(())
}

/// Warns when the API repository changed its spec after the commit last synced.
fn behind(report: &mut Report, api: &GitHub, source: &link::Source, on: PushOn) -> Result<()> {
    let Some(sha) = &source.sha else {
        return Ok(());
    };
    let Some(commits) = api.find(&format!(
        "/repos/{}/commits?path={}&per_page=1",
        source.repo, source.path
    ))?
    else {
        return Ok(());
    };
    let Some(latest) = commits[0]["sha"].as_str() else {
        return Ok(());
    };
    if latest == sha {
        return Ok(());
    }
    let compare = api.find(&format!("/repos/{}/compare/{sha}...{latest}", source.repo))?;
    if compare.is_some_and(|c| c["status"] == "ahead") {
        let changed = format!(
            "{}@{} changed the spec after the last sync",
            source.repo,
            latest.get(..7).unwrap_or(latest)
        );
        match on {
            PushOn::Change => report.fail(
                &changed,
                &format!("check the perseid-push.yml runs of {}", source.repo),
            ),
            PushOn::Release => report
                .ui
                .info(&format!("{changed}: the next release pushes it")),
            PushOn::Tag => report
                .ui
                .info(&format!("{changed}: the next tag pushes it")),
        }
    }
    Ok(())
}

fn installation(report: &mut Report, api: &GitHub, hub: &str) -> Result<()> {
    let Some(id) = secrets::variable(api, hub, "SDK_APP_ID")? else {
        return Ok(());
    };
    let owner = hub.split('/').next().unwrap_or_default();
    let reply = api.send(
        "GET",
        &format!("/orgs/{owner}/installations?per_page=100"),
        None,
    )?;
    if reply.status != 200 {
        return Ok(());
    }
    let found = reply.body["installations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i: &&Value| {
            i["app_id"].as_u64().map(|n| n.to_string()).as_deref() == Some(id.as_str())
        });
    match found {
        Some(installation) => report.ui.ok(&format!(
            "The App is installed on {owner} ({} repositories)",
            installation["repository_selection"]
                .as_str()
                .unwrap_or("selected")
        )),
        None => report.fail(
            &format!("The App {id} isn't installed on {owner}"),
            "install it from its settings page, or run `perseid setup`",
        ),
    }
    Ok(())
}

fn run(report: &mut Report, api: &GitHub, repo: &str, workflow: &str) -> Result<()> {
    let Some(runs) = api.find(&format!(
        "/repos/{repo}/actions/workflows/{workflow}/runs?per_page=1"
    ))?
    else {
        return Ok(());
    };
    let Some(last) = runs["workflow_runs"].get(0) else {
        report
            .ui
            .warn(&format!("{repo}: {workflow} hasn't run yet"));
        return Ok(());
    };
    let when = last["created_at"]
        .as_str()
        .and_then(timestamp)
        .map_or_else(String::new, |t| format!(", {}", ago(t)));
    let url = last["html_url"].as_str().unwrap_or_default();
    match (last["status"].as_str(), last["conclusion"].as_str()) {
        (Some("completed"), Some("success")) => report
            .ui
            .ok(&format!("{repo}: last {workflow} run succeeded{when}")),
        (Some("completed"), conclusion) => report.fail(
            &format!(
                "{repo}: last {workflow} run {}{when}",
                conclusion.unwrap_or("failed")
            ),
            &format!("see {url}"),
        ),
        _ => report
            .ui
            .warn(&format!("{repo}: {workflow} is running{when}, {url}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_timestamps_are_read_as_unix_seconds() {
        assert_eq!(timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(timestamp("2026-09-30T12:34:56Z"), Some(1_790_771_696));
        assert_eq!(timestamp("soon"), None);
    }
}
