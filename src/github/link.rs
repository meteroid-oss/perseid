//! The one-way link from the repository holding the spec to the SDKs repository receiving it: a
//! write deploy key of the SDKs repository only, and a workflow pushing the spec over SSH.

use std::path::Path;

use anyhow::{Result, anyhow};
use crypto_box::aead::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use super::{Ui, api::GitHub, secrets};

pub const SECRET: &str = "PERSEID_SDKS_DEPLOY_KEY";
pub const VARIABLE: &str = "PERSEID_SDKS_REPO";
pub const WORKFLOW: &str = ".github/workflows/perseid-push.yml";
pub use crate::pr::SOURCE;

/// The commit the spec was last pushed from, as `.perseid/source.json` holds it.
#[derive(Deserialize, Serialize)]
pub struct Source {
    /// Absent when pushed with `perseid connect --private`.
    pub repo: Option<String>,
    pub sha: Option<String>,
    /// The release or tag that pushed it.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

pub fn source(root: &Path) -> Option<Source> {
    serde_json::from_str(&std::fs::read_to_string(root.join(SOURCE)).ok()?).ok()
}

/// When the repository holding the spec pushes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum PushOn {
    /// Every push to the default branch changing the spec.
    #[default]
    Change,
    /// Every published GitHub release, with the spec of its tag.
    Release,
    /// Every tag matching `--tags`.
    Tag,
}

impl PushOn {
    pub fn name(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::Release => "release",
            Self::Tag => "tag",
        }
    }
}

/// The title naming the deploy key an API repository pushes with.
pub fn key_title(api_repo: &str) -> String {
    format!("perseid: spec pushes from {api_repo}")
}

/// The deploy key of `sdks_repo` for `api_repo`: its id and whether it can write.
pub fn deploy_key(api: &GitHub, sdks_repo: &str, api_repo: &str) -> Result<Option<(u64, bool)>> {
    let Some(keys) = api.find(&format!("/repos/{sdks_repo}/keys?per_page=100"))? else {
        return Ok(None);
    };
    let title = key_title(api_repo);
    Ok(keys.as_array().into_iter().flatten().find_map(|k: &Value| {
        (k["title"] == title.as_str()).then(|| {
            (
                k["id"].as_u64().unwrap_or_default(),
                k["read_only"] == false,
            )
        })
    }))
}

/// Generates an ed25519 deploy key and stores its private half as a secret of `api_repo`. With
/// `register`, adds it to `sdks_repo` with write access, replacing `stale`; otherwise, says how
/// an admin of `sdks_repo` adds it.
pub fn add_deploy_key(
    api: &GitHub,
    api_repo: &str,
    sdks_repo: &str,
    (register, stale): (bool, Option<u64>),
    ui: &Ui,
) -> Result<()> {
    let title = key_title(api_repo);
    let mut key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|e| anyhow!("generating a deploy key: {e}"))?;
    key.set_comment(&title);
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|e| anyhow!("encoding the deploy key: {e}"))?;
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| anyhow!("encoding the deploy key: {e}"))?;
    if register {
        if let Some(id) = stale {
            api.delete(&format!("/repos/{sdks_repo}/keys/{id}"))?;
        }
        api.post(
            &format!("/repos/{sdks_repo}/keys"),
            json!({ "title": title, "key": public, "read_only": false }),
        )?;
    }
    secrets::set_secret(api, api_repo, SECRET, &private)?;
    match register {
        true => ui.ok(&format!(
            "{sdks_repo} has a write deploy key for {api_repo}, whose {SECRET} secret holds its private half"
        )),
        false => {
            ui.ok(&format!("{api_repo}: the {SECRET} secret holds a new deploy key"));
            ui.say(&format!(
                "An admin of {sdks_repo} must add its public half at {}/{sdks_repo}/settings/keys/new, checking \"Allow write access\":",
                super::api::web_base()
            ));
            ui.info(&format!("Title: {title}"));
            ui.info(&format!("Key:   {public}"));
        }
    }
    Ok(())
}

/// How perseid-push.yml gets the spec to the SDKs repository.
pub struct Push<'a> {
    pub branch: &'a str,
    pub on: PushOn,
    /// The tags pushing the spec with `PushOn::Tag`.
    pub tags: &'a str,
    /// The spec, relative to the root of the repository holding it.
    pub spec: &'a str,
    /// Run from the repository's root to write the spec, when it isn't committed.
    pub build: Option<&'a str>,
    pub hub: &'a str,
    /// Where the SDKs repository reads the spec, relative to its root: `spec` of its perseid.toml.
    pub destination: &'a str,
    /// The SDKs repository's perseid.toml, relative to its root.
    pub config: &'a str,
    /// Leaves this repository's name out of `.perseid/source.json` and commit messages.
    pub private: bool,
}

/// What perseid-push.yml says, read back to update it or check it.
#[derive(Debug, PartialEq, Eq)]
pub struct Pushed {
    pub hub: String,
    pub on: PushOn,
    pub tags: Option<String>,
    pub spec: String,
    pub build: Option<String>,
    pub private: bool,
}

/// The settings of a perseid-push.yml that `perseid connect` wrote.
pub fn pushed(yaml: &str) -> Option<Pushed> {
    let header = yaml.lines().next()?;
    let hub = header
        .strip_prefix("# Written by `perseid connect ")?
        .split('`')
        .next()?
        .to_owned();
    let workflow: Value = serde_norway::from_str(yaml).ok()?;
    let on = &workflow["on"];
    let tags = on["push"]["tags"][0].as_str().map(str::to_owned);
    let on = match (on.get("release"), &tags) {
        (Some(_), _) => PushOn::Release,
        (None, Some(_)) => PushOn::Tag,
        (None, None) => PushOn::Change,
    };
    let steps = workflow["jobs"]["push"]["steps"].as_array()?;
    let step = |name: &str| steps.iter().find(|s| s["name"] == name);
    let env = &step("Push the spec to the SDKs repository")?["env"];
    let build = step("Write the spec")
        .and_then(|s| s["run"].as_str())
        .map(|run| run.trim_end().to_owned());
    Some(Pushed {
        hub,
        on,
        tags,
        spec: env["SPEC"].as_str()?.to_owned(),
        build,
        private: env["SOURCE_REPOSITORY"].as_str() == Some(""),
    })
}

/// GitHub's SSH host keys, from https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints
const KNOWN_HOSTS: &str = "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=
github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=";

/// The shell pushing the spec, run from the checkout of the repository holding it at `$GITHUB_SHA`.
const PUSH_SCRIPT: &str = r#"set -euo pipefail
(umask 077 && printf '%s\n' "$DEPLOY_KEY" > "$RUNNER_TEMP/deploy_key")
printf '%s\n' "$KNOWN_HOSTS" > "$RUNNER_TEMP/known_hosts"
export GIT_SSH_COMMAND="ssh -i $RUNNER_TEMP/deploy_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$RUNNER_TEMP/known_hosts"
sdks="$RUNNER_TEMP/sdks"
git clone --quiet --depth 1 "git@github.com:$SDKS_REPO.git" "$sdks"
if [ ! -f "$sdks/$SDKS_CONFIG" ]; then
  echo "::notice::$SDKS_REPO has no $SDKS_CONFIG yet: run \`perseid init\` there and commit it, then run this workflow again"
  exit 0
fi
source="$(dirname "$SDKS_CONFIG")/.perseid/source.json"
synced=""
if [ -f "$sdks/$source" ]; then
  synced=$(sed -n 's/.*"sha": *"\([0-9a-f]*\)".*/\1/p' "$sdks/$source")
fi
if [ -n "$synced" ] && ! git merge-base --is-ancestor "$synced" "$GITHUB_SHA" 2>/dev/null; then
  echo "::notice::$SDKS_REPO has the spec of $synced, which $GITHUB_SHA doesn't descend from: skipped"
  exit 0
fi
if cmp -s "$SPEC" "$sdks/$SDKS_SPEC"; then
  echo "$SDKS_REPO already has this spec"
  exit 0
fi
mkdir -p "$(dirname "$sdks/$SDKS_SPEC")" "$(dirname "$sdks/$source")"
cp "$SPEC" "$sdks/$SDKS_SPEC"
at="${GITHUB_SHA::7}"
ref=""
if [ -n "${REF:-}" ]; then
  at="$REF ($at)"
  ref=$(printf ',\n  "ref": "%s"' "${REF//\"/}")
fi
repo=""
if [ -n "${SOURCE_REPOSITORY:-}" ]; then
  repo=$(printf '\n  "repo": "%s",' "$SOURCE_REPOSITORY")
  at="$SOURCE_REPOSITORY@$at"
fi
printf '{%s\n  "sha": "%s"%s\n}\n' "$repo" "$GITHUB_SHA" "$ref" > "$sdks/$source"
git -C "$sdks" add "$SDKS_SPEC" "$source"
git -C "$sdks" -c user.name='github-actions[bot]' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
  commit --quiet -m "spec: $at"
git -C "$sdks" push --quiet origin HEAD
echo "Pushed the spec of $at to $SDKS_REPO"
"#;

/// The workflow pushing the spec to the SDKs repository, one run at a time and never older
/// commits over newer ones.
pub fn push_workflow(push: &Push) -> String {
    let indent = |text: &str, spaces: usize| {
        let pad = " ".repeat(spaces);
        text.lines()
            .map(|l| match l {
                "" => String::new(),
                l => format!("{pad}{l}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let build = push.build.map_or_else(String::new, |command| {
        format!(
            "      # Set up here the toolchain the command needs.\n      - name: Write the spec\n        run: |\n{}\n",
            indent(command, 10)
        )
    });
    let quoted = format!("'{}'", push.branch.replace('\'', "''"));
    let tagged = format!("startsWith(github.ref, 'refs/tags/') || github.ref_name == {quoted}");
    let (when, trigger, guard) = match push.on {
        PushOn::Change => {
            let paths = match push.build {
                Some(_) => String::new(),
                None => format!(
                    "\n    paths: {}",
                    serde_json::to_string(&[push.spec, WORKFLOW]).unwrap_or_default()
                ),
            };
            (
                "on each change",
                format!("push:\n    branches: [{:?}]{paths}", push.branch),
                format!("github.ref_name == {quoted}"),
            )
        }
        PushOn::Release => (
            "on each published release",
            "release:\n    types: [published]".to_owned(),
            tagged,
        ),
        PushOn::Tag => (
            "on each matching tag",
            format!("push:\n    tags: [{:?}]", push.tags),
            tagged,
        ),
    };
    let source = match push.private {
        true => "''",
        false => "${{ github.repository }}",
    };
    format!(
        r#"# Written by `perseid connect {hub}`: pushes the spec to {hub} {when},
# which regenerates the SDKs. Run that command again to change these settings. The
# PERSEID_SDKS_DEPLOY_KEY deploy key can write to {hub} only.
name: Spec

on:
  {trigger}
  workflow_dispatch:

permissions:
  contents: read

# One push at a time, in commit order: runs queue instead of cancelling each other.
concurrency:
  group: perseid-push
  cancel-in-progress: false

jobs:
  push:
    if: {guard}
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0
{build}      - name: Push the spec to the SDKs repository
        env:
          SPEC: {spec:?}
          SDKS_REPO: ${{{{ vars.PERSEID_SDKS_REPO }}}}
          SDKS_SPEC: {destination:?}
          SDKS_CONFIG: {config:?}
          SOURCE_REPOSITORY: {source}
          DEPLOY_KEY: ${{{{ secrets.PERSEID_SDKS_DEPLOY_KEY }}}}
          REF: ${{{{ startsWith(github.ref, 'refs/tags/') && github.ref_name || '' }}}}
          KNOWN_HOSTS: |
{known_hosts}
        run: |
{script}
"#,
        hub = push.hub,
        spec = push.spec,
        destination = push.destination,
        config = push.config,
        known_hosts = indent(KNOWN_HOSTS, 12),
        script = indent(PUSH_SCRIPT, 10),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(on: PushOn, build: Option<&str>, private: bool) -> Push<'_> {
        Push {
            branch: "main",
            on,
            tags: "api-v*",
            spec: "api/openapi.json",
            build,
            hub: "acme/api-sdks",
            destination: "openapi.json",
            config: "perseid.toml",
            private,
        }
    }

    #[test]
    fn generated_specs_are_pushed_on_every_commit() {
        let command = "cargo run --bin openapi > api/openapi.json";
        let yaml = push_workflow(&push(PushOn::Change, Some(command), false));
        assert!(!yaml.contains("paths:"), "{yaml}");
        assert!(
            yaml.contains("        run: |\n          cargo run --bin openapi > api/openapi.json\n"),
            "{yaml}"
        );
        let parsed: Value = serde_norway::from_str(&yaml).unwrap();
        let steps = &parsed["jobs"]["push"]["steps"];
        assert_eq!(steps[2]["env"]["SPEC"], "api/openapi.json");
        assert_eq!(steps[2]["env"]["SDKS_SPEC"], "openapi.json");
        assert!(
            steps[2]["env"]["KNOWN_HOSTS"]
                .as_str()
                .unwrap()
                .ends_with("wsjk=\n")
        );
    }

    #[test]
    fn workflows_read_back_as_their_settings() {
        for (on, build, private) in [
            (PushOn::Change, None, false),
            (PushOn::Release, Some("make spec"), true),
            (PushOn::Tag, None, false),
        ] {
            let read = pushed(&push_workflow(&push(on, build, private))).unwrap();
            assert_eq!(
                read,
                Pushed {
                    hub: "acme/api-sdks".into(),
                    on,
                    tags: (on == PushOn::Tag).then(|| "api-v*".into()),
                    spec: "api/openapi.json".into(),
                    build: build.map(str::to_owned),
                    private,
                }
            );
        }
        assert_eq!(pushed("name: Spec\n"), None);
    }
}
