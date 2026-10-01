//! `perseid status`: what `perseid init` and `perseid app` would change, and how the
//! automation fares. Exits 1 on errors, 2 when something waits on you, 0 otherwise.

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
    auth,
    connect::{self, Settings},
    git, layout,
    link::{self, SOURCE},
    plan::{self, Mark, Plan},
    secrets, toplevel,
};
use crate::config::{Config, Source};

struct Report {
    ui: Ui,
    failed: bool,
    pending: bool,
}

impl Report {
    fn fail(&mut self, message: &str, fix: &str) {
        self.ui.fail(message);
        self.ui.say(fix);
        self.failed = true;
    }

    /// The steps of `plan` left to apply.
    fn pending(&mut self, plan: &Plan, run: &str) {
        match plan.pending() {
            0 => self.ui.ok("In sync"),
            n => {
                self.ui.warn(&format!(
                    "{n} change{} pending:",
                    if n == 1 { "" } else { "s" }
                ));
                for step in plan.steps.iter().filter(|s| s.mark != Mark::Keep) {
                    let mark = if step.mark == Mark::Add { '+' } else { '~' };
                    println!("    {mark} {}", step.text);
                }
                self.ui.say(&format!("run `{run}`"));
                self.pending = true;
            }
        }
        for warning in &plan.warnings {
            self.ui.warn(warning);
        }
        self.pending |= plan.attention;
    }
}

/// A signed-in GitHub client, from stored credentials only.
fn session() -> Option<(GitHub, String, &'static str)> {
    auth::stored_token().and_then(|(token, source)| {
        let api = GitHub::new(Some(token));
        let user = api.get("/user").ok()?;
        Some((api, user["login"].as_str()?.to_owned(), source))
    })
}

pub fn status(config_path: &Path) -> Result<ExitCode> {
    let ui = Ui::new(&Options {
        yes: true,
        dry_run: true,
        browser: false,
    });
    let mut report = Report {
        ui,
        failed: false,
        pending: false,
    };
    if !config_path.exists() {
        let dir = std::path::absolute(config_path)?
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let top = toplevel(&dir).unwrap_or(dir);
        let workflow = std::fs::read_to_string(top.join(link::WORKFLOW)).ok();
        if let Some(pushed) = workflow.as_deref().and_then(link::pushed) {
            pushing(&mut report, &top, pushed)?;
            return Ok(exit(&report));
        }
    }
    let (config, root) = match Config::load(config_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            report.fail(
                &format!("{error:#}"),
                "fix perseid.toml, run `perseid init` in the repository holding the SDKs, or `perseid connect <owner/sdks-repository>` in the one holding the spec",
            );
            return Ok(exit(&report));
        }
    };
    let here = layout::origin_repo(&root);
    let session = session();
    let planned = match (&session, &here) {
        (Some((api, login, _)), Some(_)) => {
            let cx = plan::Session {
                api,
                login,
                app: false,
                collisions: true,
            };
            Some(plan::plan(&cx, config_path))
        }
        _ => None,
    };
    let diagram = match &planned {
        Some(Ok(plan)) => plan.diagram.clone(),
        _ => local_diagram(&config, &root, here.as_deref().unwrap_or("this repository"))?,
    };
    println!("{diagram}\n");
    report.ui.ok("perseid.toml is valid");
    local(&mut report, &config, &root);
    let (Some((api, login, source)), Some(_)) = (&session, &here) else {
        local_spec(&mut report, &root);
        signed_out(&mut report, here.is_some());
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
                "fix what it says, then run `perseid status` again",
            );
            return Ok(exit(&report));
        }
        None => return Ok(exit(&report)),
    };
    report.pending(&plan, "perseid app");
    if let Source::File(_) = config.source() {
        last_spec(&mut report, api, &plan, plan.awaits_spec)?;
    }
    health(&mut report, api, &plan)?;
    Ok(exit(&report))
}

fn exit(report: &Report) -> ExitCode {
    match (report.failed, report.pending) {
        (true, _) => ExitCode::FAILURE,
        (false, true) => ExitCode::from(2),
        (false, false) => ExitCode::SUCCESS,
    }
}

fn signed_out(report: &mut Report, github: bool) {
    match github {
        false => report
            .ui
            .warn("origin isn't a GitHub repository: only local checks ran"),
        true => {
            report
                .ui
                .warn("Not signed in to GitHub: only local checks ran");
            report
                .ui
                .say("set GH_TOKEN, or run `gh auth login`, to check GitHub too");
        }
    }
}

/// `perseid status` in a repository whose perseid-push.yml pushes its spec to `pushed.hub`.
fn pushing(report: &mut Report, top: &Path, pushed: link::Pushed) -> Result<()> {
    let here = layout::origin_repo(top);
    println!(
        "{} ──spec──▶ {}\n",
        here.as_deref().unwrap_or("this repository"),
        pushed.hub
    );
    report.ui.ok(&format!("{} is here", link::WORKFLOW));
    match top.join(&pushed.spec).exists() || pushed.build.is_some() {
        true => report
            .ui
            .ok(&format!("{} pushes {}", link::WORKFLOW, pushed.spec)),
        false => report.fail(
            &format!("the spec {} is missing", pushed.spec),
            &format!("run `perseid connect {} --spec <path>`", pushed.hub),
        ),
    }
    let (Some((api, login, source)), Some(here)) = (session(), here) else {
        signed_out(report, layout::origin_repo(top).is_some());
        return Ok(());
    };
    report
        .ui
        .ok(&format!("Signed in to GitHub as {login} ({source})"));
    let cx = plan::Session {
        api: &api,
        login: &login,
        app: false,
        collisions: false,
    };
    let hub = pushed.hub.clone();
    let settings = Settings {
        here: here.clone(),
        top: top.to_owned(),
        pushed,
    };
    let plan = match connect::plan_connect(&cx, &settings) {
        Ok(plan) => plan,
        Err(error) => {
            report.fail(
                &format!("{error:#}"),
                &format!("fix what it says, then run `perseid connect {hub}`"),
            );
            return Ok(());
        }
    };
    report.pending(&plan, &format!("perseid connect {hub}"));
    last_spec(report, &api, &plan, true)?;
    run(report, &api, &here, "perseid-push.yml")
}

fn local_diagram(config: &Config, root: &Path, here: &str) -> Result<String> {
    let sdks = config.sdks(&[])?;
    let targets = layout::targets(&sdks, here);
    let source = match config.source() {
        Source::Url(url) => Some(url.to_owned()),
        Source::File(_) => link::source(root).and_then(|s| s.repo),
    };
    Ok(layout::diagram(source.as_deref(), here, &targets))
}

/// What this checkout tells without GitHub: the spec, the workflow, the last spec pushed.
fn local(report: &mut Report, config: &Config, root: &Path) {
    let top = toplevel(root).unwrap_or_else(|_| root.to_owned());
    let workflow = layout::SDKS_WORKFLOW;
    match top.join(workflow).exists() {
        true => report.ui.ok(&format!("{workflow} is here")),
        false => {
            report.ui.warn(&format!(
                "{workflow} isn't here: run `perseid init`, then commit and push"
            ));
            report.pending = true;
        }
    }
    if let Source::File(file) = config.source()
        && !root.join(file).exists()
    {
        let hub = layout::origin_repo(root).unwrap_or_else(|| "<owner/this-repository>".into());
        report.ui.warn(&format!(
            "the spec {file} isn't here yet: run `npx perseid connect {hub}` in the repository holding it"
        ));
    }
}

/// The last spec pushed here, as this checkout's `.perseid/source.json` says.
fn local_spec(report: &mut Report, root: &Path) {
    if let Some(source) = link::source(root) {
        let age = git(root, &["log", "-1", "--format=%ct", "--", SOURCE])
            .ok()
            .and_then(|out| String::from_utf8(out).ok()?.trim().parse::<i64>().ok());
        synced(report, &source, age);
    }
}

fn synced(report: &mut Report, source: &link::Source, when: Option<i64>) {
    let Some(sha) = &source.sha else {
        return;
    };
    let age = when.map_or_else(String::new, |t| format!(", {}", ago(t)));
    let short = sha.get(..7).unwrap_or(sha);
    let at = match &source.tag {
        Some(tag) => format!("{tag} ({short})"),
        None => short.to_owned(),
    };
    let from = source
        .repo
        .as_ref()
        .map_or_else(String::new, |r| format!(" from {r}"));
    report.ui.ok(&format!("Last spec{from} at {at}{age}"));
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

/// The last spec pushed to the hub of `plan`, as its `.perseid/source.json` on GitHub says;
/// `expected` warns when there is none.
fn last_spec(report: &mut Report, api: &GitHub, plan: &Plan, expected: bool) -> Result<()> {
    let info = api.find(&format!("/repos/{}", plan.hub))?;
    let branch = info
        .as_ref()
        .map_or_else(|| "main".to_owned(), super::bootstrap::default_branch);
    let file = super::join(&plan.hub_dir, SOURCE);
    let Some(text) = super::bootstrap::read(api, &plan.hub, &branch, &file)? else {
        if expected {
            report
                .ui
                .warn(&format!("{} hasn't received a spec yet", plan.hub));
        }
        return Ok(());
    };
    let Ok(source) = serde_json::from_str::<link::Source>(&text) else {
        return Ok(());
    };
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
    Ok(())
}

fn health(report: &mut Report, api: &GitHub, plan: &Plan) -> Result<()> {
    let config = plan.config();
    let mut repos = BTreeSet::new();
    for sdk in config.sdks(&[])? {
        repos.insert(sdk.remote().unwrap_or(&plan.hub).to_owned());
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
    run(report, api, &plan.hub, "sdks.yml")
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
            "install it from its settings page, or run `perseid app`",
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
