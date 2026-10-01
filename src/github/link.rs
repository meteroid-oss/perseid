//! The one-way link from an API repository to the SDKs repository receiving its spec: a write
//! deploy key of the SDKs repository only, and a workflow pushing the spec over SSH.

use std::path::Path;

use anyhow::{Result, anyhow};
use crypto_box::aead::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use super::{Ui, api::GitHub, secrets};
use crate::config::PushOn;

pub const SECRET: &str = "PERSEID_SDKS_DEPLOY_KEY";
pub const VARIABLE: &str = "PERSEID_SDKS_REPO";
pub use crate::pr::SOURCE;

/// The commit of the API repository a snapshot comes from, as `.perseid/source.json` holds it.
#[derive(Deserialize, Serialize)]
pub struct Source {
    pub repo: String,
    pub path: String,
    pub sha: Option<String>,
    /// The release or tag that pushed it.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Source {
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

pub fn source(root: &Path) -> Option<Source> {
    serde_json::from_str(&std::fs::read_to_string(root.join(SOURCE)).ok()?).ok()
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

/// Registers a new ed25519 write deploy key on `sdks_repo`, replacing `stale`, and stores its
/// private half as a secret of `api_repo`.
pub fn add_deploy_key(
    api: &GitHub,
    api_repo: &str,
    sdks_repo: &str,
    stale: Option<u64>,
    ui: &Ui,
) -> Result<()> {
    if let Some(id) = stale {
        api.delete(&format!("/repos/{sdks_repo}/keys/{id}"))?;
    }
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
    api.post(
        &format!("/repos/{sdks_repo}/keys"),
        json!({ "title": title, "key": public, "read_only": false }),
    )?;
    secrets::set_secret(api, api_repo, SECRET, &private)?;
    ui.ok(&format!(
        "{sdks_repo} has a write deploy key for {api_repo}, whose {SECRET} secret holds its private half"
    ));
    Ok(())
}

/// How perseid-push.yml gets the spec to the SDKs repository.
pub struct Push<'a> {
    pub branch: &'a str,
    pub on: PushOn,
    /// The tags pushing the spec with `PushOn::Tag`.
    pub tags: &'a str,
    /// The spec, relative to the API repository's root.
    pub spec: &'a str,
    /// Run from the repository's root to write the spec, when it isn't committed.
    pub generate: Option<&'a str>,
    pub sdks_repo: &'a str,
    /// Where the SDKs repository keeps the spec, relative to its root.
    pub snapshot: &'a str,
}

/// GitHub's SSH host keys, from https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints
const KNOWN_HOSTS: &str = "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=
github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=";

/// The shell pushing the spec, run from the API repository's checkout at `$GITHUB_SHA`.
const PUSH_SCRIPT: &str = r#"set -euo pipefail
(umask 077 && printf '%s\n' "$DEPLOY_KEY" > "$RUNNER_TEMP/deploy_key")
printf '%s\n' "$KNOWN_HOSTS" > "$RUNNER_TEMP/known_hosts"
export GIT_SSH_COMMAND="ssh -i $RUNNER_TEMP/deploy_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$RUNNER_TEMP/known_hosts"
sdks="$RUNNER_TEMP/sdks"
git clone --quiet --depth 1 "git@github.com:$SDKS_REPO.git" "$sdks"
source="$sdks/$(dirname "$SNAPSHOT")/.perseid/source.json"
if [ ! -f "$source" ]; then
  echo "::notice::$SDKS_REPO isn't set up yet: merge its perseid/setup pull request, then run this workflow again"
  exit 0
fi
synced=$(sed -n 's/.*"sha": *"\([0-9a-f]*\)".*/\1/p' "$source")
if [ -n "$synced" ] && ! git merge-base --is-ancestor "$synced" "$GITHUB_SHA" 2>/dev/null; then
  echo "::notice::$SDKS_REPO has the spec of $synced, which $GITHUB_SHA doesn't descend from: skipped"
  exit 0
fi
if cmp -s "$SPEC" "$sdks/$SNAPSHOT"; then
  echo "$SDKS_REPO already has this spec"
  exit 0
fi
cp "$SPEC" "$sdks/$SNAPSHOT"
at="${GITHUB_SHA::7}"
ref=""
if [ -n "${REF:-}" ]; then
  at="$REF ($at)"
  ref=$(printf ',\n  "ref": "%s"' "${REF//\"/}")
fi
printf '{\n  "repo": "%s",\n  "path": "%s",\n  "sha": "%s"%s\n}\n' "$GITHUB_REPOSITORY" "$SPEC" "$GITHUB_SHA" "$ref" > "$source"
git -C "$sdks" add "$SNAPSHOT" "$source"
git -C "$sdks" -c user.name='github-actions[bot]' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
  commit --quiet -m "spec: $GITHUB_REPOSITORY@$at"
git -C "$sdks" push --quiet origin HEAD
echo "Pushed the spec of $GITHUB_REPOSITORY@$at to $SDKS_REPO"
"#;

/// The workflow of the API repository pushing its spec to the SDKs repository, one run at a time
/// and never older commits over newer ones.
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
    let generate = push.generate.map_or_else(String::new, |command| {
        format!(
            "      # Set up here the toolchain the command needs.\n      - name: Write the spec\n        run: |\n{}\n",
            indent(command, 10)
        )
    });
    let quoted = format!("'{}'", push.branch.replace('\'', "''"));
    let tagged = format!("startsWith(github.ref, 'refs/tags/') || github.ref_name == {quoted}");
    let (when, trigger, guard) = match push.on {
        PushOn::Change => {
            let paths = match push.generate {
                Some(_) => String::new(),
                None => format!(
                    "\n    paths: {}",
                    serde_json::to_string(&[push.spec, ".github/workflows/perseid-push.yml"])
                        .unwrap_or_default()
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
    format!(
        r#"# Written by `perseid setup`: pushes the spec to {sdks} {when},
# which regenerates the SDKs. The PERSEID_SDKS_DEPLOY_KEY deploy key can write to that
# repository only.
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
{generate}      - name: Push the spec to the SDKs repository
        env:
          SDKS_REPO: ${{{{ vars.PERSEID_SDKS_REPO }}}}
          DEPLOY_KEY: ${{{{ secrets.PERSEID_SDKS_DEPLOY_KEY }}}}
          SPEC: {spec}
          SNAPSHOT: {snapshot}
          REF: ${{{{ startsWith(github.ref, 'refs/tags/') && github.ref_name || '' }}}}
          KNOWN_HOSTS: |
{known_hosts}
        run: |
{script}
"#,
        sdks = push.sdks_repo,
        spec = push.spec,
        snapshot = push.snapshot,
        known_hosts = indent(KNOWN_HOSTS, 12),
        script = indent(PUSH_SCRIPT, 10),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_specs_are_pushed_on_every_commit() {
        let yaml = push_workflow(&Push {
            branch: "main",
            on: PushOn::Change,
            tags: "v*",
            spec: "api/openapi.json",
            generate: Some("cargo run --bin openapi > api/openapi.json"),
            sdks_repo: "acme/api-sdks",
            snapshot: "openapi.json",
        });
        assert!(!yaml.contains("paths:"), "{yaml}");
        assert!(
            yaml.contains("        run: |\n          cargo run --bin openapi > api/openapi.json\n"),
            "{yaml}"
        );
        let parsed: serde_json::Value = serde_norway::from_str(&yaml).unwrap();
        let steps = &parsed["jobs"]["push"]["steps"];
        assert_eq!(steps[2]["env"]["SPEC"], "api/openapi.json");
        assert!(
            steps[2]["env"]["KNOWN_HOSTS"]
                .as_str()
                .unwrap()
                .ends_with("wsjk=\n")
        );
    }
}
