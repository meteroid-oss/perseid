//! `perseid push-spec`, which perseid-push.yml runs: commits the spec to the SDKs repository
//! with a deploy key or a token, never an older commit's spec over a newer one's.

use std::{
    path::Path,
    process::{Command, ExitCode, Stdio},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use super::{join, link};
use crate::config::{self, Config, Source};

/// GitHub's SSH host keys, from https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints
const KNOWN_HOSTS: &str = "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=
github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=
";

const BOT: [&str; 4] = [
    "-c",
    "user.name=github-actions[bot]",
    "-c",
    "user.email=41898282+github-actions[bot]@users.noreply.github.com",
];

pub struct PushSpec {
    /// The spec, relative to `cwd`.
    pub spec: String,
    /// The SDKs repository, `owner/name`.
    pub to: String,
    /// Leaves this repository's name out of `.perseid/source.json` and commit messages.
    pub private: bool,
}

pub fn push_spec(cwd: &Path, push: PushSpec) -> Result<ExitCode> {
    let PushSpec { spec, to, private } = push;
    if let Some(skipped) = off_default_branch() {
        notice(&skipped);
        return Ok(ExitCode::SUCCESS);
    }
    let content = std::fs::read(cwd.join(&spec)).with_context(|| format!("reading {spec}"))?;
    let sha = match env("GITHUB_SHA") {
        Some(sha) => sha,
        None => git(cwd, &["rev-parse", "HEAD"])?,
    };
    let temp = tempfile::tempdir()?;
    let (url, auth) = remote(temp.path(), &to)?;
    let sdks = temp.path().join("sdks");
    let target = sdks.to_string_lossy();
    run(git_remote(temp.path(), &auth).args(["clone", "--quiet", "--depth", "1", &url, &*target]))?;

    let Some(config_path) = find_config(&sdks)? else {
        notice(&format!(
            "{to} has no {} yet: run `perseid init` there and commit it, then run this again",
            config::FILE
        ));
        return Ok(ExitCode::SUCCESS);
    };
    let dir = config_path
        .strip_suffix(config::FILE)
        .unwrap_or_default()
        .trim_end_matches('/');
    let text = std::fs::read_to_string(sdks.join(&config_path))?;
    let config = Config::parse(&text, &format!("{to}/{config_path}"))?;
    let Source::File(file) = config.source() else {
        bail!(
            "{to}/{config_path} reads its spec from {}: set its `spec` to a file",
            config.spec
        );
    };
    let destination = join(dir, file);
    let source = join(dir, link::SOURCE);

    let here = env("GITHUB_REPOSITORY");
    let synced = link::source(&sdks.join(dir))
        .filter(|s| match (&s.repo, &here) {
            (Some(recorded), Some(here)) => recorded.eq_ignore_ascii_case(here),
            _ => true,
        })
        .and_then(|s| s.sha);
    if let Some(synced) = synced {
        match lineage(cwd, &synced, &sha)? {
            Lineage::Descends => {}
            Lineage::Older => {
                notice(&format!(
                    "{to} has the spec of {}, newer than {}: skipped",
                    short(&synced),
                    short(&sha)
                ));
                return Ok(ExitCode::SUCCESS);
            }
            Lineage::Diverged => bail!(
                "{to} has the spec of {}, which {} doesn't descend from (a release cut off the default branch, or rewritten history): to push this spec anyway, remove `sha` from {source} in {to}",
                short(&synced),
                short(&sha)
            ),
        }
    }
    if std::fs::read(sdks.join(&destination)).ok().as_deref() == Some(content.as_slice()) {
        println!("{to} already has this spec");
        return Ok(ExitCode::SUCCESS);
    }

    let tag = env("GITHUB_REF").and_then(|r| r.strip_prefix("refs/tags/").map(str::to_owned));
    let repo = env("GITHUB_REPOSITORY").filter(|_| !private);
    let mut at = short(&sha).to_owned();
    if let Some(tag) = &tag {
        at = format!("{tag} ({at})");
    }
    if let Some(repo) = &repo {
        at = format!("{repo}@{at}");
    }
    let record = link::Source {
        repo,
        sha: Some(sha),
        tag,
    };
    crate::fsx::write(&sdks.join(&destination), &content)?;
    crate::fsx::write(
        &sdks.join(&source),
        (serde_json::to_string_pretty(&record)? + "\n").as_bytes(),
    )?;
    git(&sdks, &["add", &destination, &source])?;
    let message = format!("spec: {at}");
    run(Command::new("git")
        .args(BOT)
        .args(["commit", "--quiet", "-m", &message])
        .current_dir(&sdks))?;
    run(git_remote(&sdks, &auth).args(["push", "--quiet", "origin", "HEAD"]))?;
    println!("Pushed the spec of {at} to {to}");
    Ok(ExitCode::SUCCESS)
}

/// Why a run on a branch other than the default one skips, as from a manual dispatch.
fn off_default_branch() -> Option<String> {
    let branch = env("GITHUB_REF")?.strip_prefix("refs/heads/")?.to_owned();
    let event: Value =
        serde_json::from_str(&std::fs::read_to_string(env("GITHUB_EVENT_PATH")?).ok()?).ok()?;
    let default = event["repository"]["default_branch"].as_str()?;
    (branch != default).then(|| format!("`{branch}` isn't the default branch `{default}`: skipped"))
}

/// The URL of `to` and the environment git reaches it with: over HTTPS with the token in
/// `PERSEID_SDKS_TOKEN`, or over SSH with the deploy key in `PERSEID_SDKS_DEPLOY_KEY`, trusting
/// GitHub's host keys only. Without either, git's own credentials.
fn remote(temp: &Path, to: &str) -> Result<(String, Vec<(String, String)>)> {
    match (env("PERSEID_SDKS_TOKEN"), env("PERSEID_SDKS_DEPLOY_KEY")) {
        (Some(_), Some(_)) => {
            bail!("set PERSEID_SDKS_TOKEN or PERSEID_SDKS_DEPLOY_KEY, not both")
        }
        (Some(token), None) => {
            let basic = BASE64.encode(format!("x-access-token:{token}"));
            let count: usize = env("GIT_CONFIG_COUNT")
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let auth = vec![
                (
                    format!("GIT_CONFIG_KEY_{count}"),
                    "http.https://github.com/.extraheader".to_owned(),
                ),
                (
                    format!("GIT_CONFIG_VALUE_{count}"),
                    format!("AUTHORIZATION: basic {basic}"),
                ),
                ("GIT_CONFIG_COUNT".to_owned(), (count + 1).to_string()),
            ];
            Ok((format!("https://github.com/{to}.git"), auth))
        }
        (None, key) => {
            let url = format!("git@github.com:{to}.git");
            let Some(key) = key else {
                if env("GITHUB_ACTIONS").as_deref() == Some("true") {
                    bail!(
                        "no credential for {to}: pass the action `deploy-key` or `token` (is the secret set in this repository?)"
                    );
                }
                return Ok((url, vec![]));
            };
            let (key_file, hosts) = (temp.join("deploy_key"), temp.join("known_hosts"));
            std::fs::write(&key_file, format!("{}\n", key.trim_end()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::write(&hosts, KNOWN_HOSTS)?;
            let ssh = format!(
                "ssh -i '{}' -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile='{}'",
                key_file.display(),
                hosts.display()
            );
            Ok((url, vec![("GIT_SSH_COMMAND".to_owned(), ssh)]))
        }
    }
}

fn git_remote(dir: &Path, auth: &[(String, String)]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .envs(auth.iter().map(|(k, v)| (k, v)));
    command
}

/// perseid.toml at the root of `sdks`, or the only one.
fn find_config(sdks: &Path) -> Result<Option<String>> {
    if sdks.join(config::FILE).exists() {
        return Ok(Some(config::FILE.to_owned()));
    }
    let suffix = format!("/{}", config::FILE);
    let files = git(sdks, &["ls-files"])?;
    let found: Vec<&str> = files.lines().filter(|p| p.ends_with(&suffix)).collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).to_owned())),
        many => bail!("several {}: {}", config::FILE, many.join(", ")),
    }
}

enum Lineage {
    Descends,
    Older,
    Diverged,
}

/// How `sha` relates to `synced`, fetching the history a shallow checkout lacks.
fn lineage(cwd: &Path, synced: &str, sha: &str) -> Result<Lineage> {
    if git(cwd, &["rev-parse", "--is-shallow-repository"])? == "true" {
        git(cwd, &["fetch", "--quiet", "--unshallow"])?;
    }
    let ancestor = |a: &str, b: &str| {
        Command::new("git")
            .args(["merge-base", "--is-ancestor", a, b])
            .current_dir(cwd)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    Ok(match (ancestor(synced, sha), ancestor(sha, synced)) {
        (true, _) => Lineage::Descends,
        (false, true) => Lineage::Older,
        (false, false) => Lineage::Diverged,
    })
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = run(Command::new("git").args(args).current_dir(dir))?;
    Ok(out.trim().to_owned())
}

fn run(command: &mut Command) -> Result<String> {
    let output = command.output().context("running git")?;
    if !output.status.success() {
        bail!(
            "`git {}` failed: {}",
            command
                .get_args()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn notice(message: &str) {
    match env("GITHUB_ACTIONS").as_deref() {
        Some("true") => println!("::notice::{message}"),
        _ => println!("{message}"),
    }
}
