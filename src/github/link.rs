//! The one-way link from the repository holding the spec to the SDKs repository receiving it: a
//! write deploy key of the SDKs repository only, and a workflow running `perseid push-spec`.

use std::path::Path;

use anyhow::{Result, anyhow};
use crypto_box::aead::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use super::{Ui, api::GitHub, secrets};

pub const SECRET: &str = "PERSEID_SDKS_DEPLOY_KEY";
pub const TOKEN_SECRET: &str = "PERSEID_SDKS_TOKEN";
pub const APP_ID: &str = "PERSEID_PUSH_APP_ID";
pub const APP_KEY: &str = "PERSEID_PUSH_APP_PRIVATE_KEY";
pub const WORKFLOW: &str = ".github/workflows/perseid-push.yml";
pub use crate::pr::SOURCE;

/// The commit the spec was last pushed from, as `.perseid/source.json` holds it.
#[derive(Deserialize, Serialize)]
pub struct Source {
    /// Absent when pushed with `perseid connect --private`.
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// How perseid-push.yml authenticates to the SDKs repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Auth {
    /// A deploy key that can write to the SDKs repository only, which `connect` adds.
    #[default]
    DeployKey,
    /// A personal access token in the PERSEID_SDKS_TOKEN secret, which you add.
    Token,
    /// A GitHub App installed on the SDKs repository only, set as PERSEID_PUSH_APP_ID and
    /// PERSEID_PUSH_APP_PRIVATE_KEY.
    App,
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
    pub auth: Auth,
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
    pub auth: Auth,
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
    let build = step("Write the spec")
        .and_then(|s| s["run"].as_str())
        .map(|run| run.trim_end().to_owned());
    let action = steps.iter().find(|s| {
        s["uses"]
            .as_str()
            .is_some_and(|u| u.starts_with("meteroid-oss/perseid/push@"))
    });
    let with = &action?["with"];
    let auth = match with["token"].as_str() {
        None => Auth::DeployKey,
        Some(token) if token.contains("steps.app") => Auth::App,
        Some(_) => Auth::Token,
    };
    Some(Pushed {
        hub,
        on,
        tags,
        spec: with["spec"].as_str()?.to_owned(),
        build,
        auth,
        private: with["private"] == true,
    })
}

/// The action running `perseid push-spec`.
pub const ACTION: &str = "meteroid-oss/perseid/push@v0";

/// The workflow pushing the spec to the SDKs repository, one run at a time.
pub fn push_workflow(push: &Push) -> String {
    let build = push.build.map_or_else(String::new, |command| {
        let command: Vec<String> = command
            .lines()
            .map(|l| match l {
                "" => String::new(),
                l => format!("          {l}"),
            })
            .collect();
        format!(
            "      # Set up here the toolchain the command needs.\n      - name: Write the spec\n        run: |\n{}\n",
            command.join("\n")
        )
    });
    let (when, trigger) = match push.on {
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
            )
        }
        PushOn::Release => (
            "on each published release",
            "release:\n    types: [published]".to_owned(),
        ),
        PushOn::Tag => (
            "on each matching tag",
            format!("push:\n    tags: [{:?}]", push.tags),
        ),
    };
    let private = match push.private {
        true => "\n          private: true",
        false => "",
    };
    let hub = push.hub;
    let (who, app, credential) = match push.auth {
        Auth::DeployKey => (
            format!("{SECRET} is the private half of a deploy key that can write to {hub} only."),
            String::new(),
            format!("deploy-key: ${{{{ secrets.{SECRET} }}}}"),
        ),
        Auth::Token => (
            format!("{TOKEN_SECRET} is a personal access token that can write to {hub}."),
            String::new(),
            format!("token: ${{{{ secrets.{TOKEN_SECRET} }}}}"),
        ),
        Auth::App => {
            let (owner, name) = hub.split_once('/').unwrap_or((hub, hub));
            (
                format!(
                    "It pushes as the GitHub App set as {APP_ID} and {APP_KEY}, installed on {hub}."
                ),
                format!(
                    "      - id: app\n        uses: actions/create-github-app-token@v2\n        with:\n          app-id: ${{{{ vars.{APP_ID} }}}}\n          private-key: ${{{{ secrets.{APP_KEY} }}}}\n          owner: {owner}\n          repositories: {name}\n          permission-contents: write\n"
                ),
                "token: ${{ steps.app.outputs.token }}".to_owned(),
            )
        }
    };
    format!(
        r#"# Written by `perseid connect {hub}`: pushes the spec to {hub} {when},
# which regenerates the SDKs. Run that command again to change these settings.
# {who}
name: Spec

on:
  {trigger}
  workflow_dispatch:

permissions:
  contents: read

concurrency: perseid-push

jobs:
  push:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
{build}{app}      - uses: {ACTION}
        with:
          spec: {spec:?}
          to: {hub}
          {credential}{private}
"#,
        spec = push.spec,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(on: PushOn, build: Option<&str>, auth: Auth, private: bool) -> Push<'_> {
        Push {
            branch: "main",
            on,
            tags: "api-v*",
            spec: "api/openapi.json",
            build,
            hub: "acme/api-sdks",
            auth,
            private,
        }
    }

    #[test]
    fn generated_specs_are_pushed_on_every_commit() {
        let command = "cargo run --bin openapi > api/openapi.json";
        let yaml = push_workflow(&push(PushOn::Change, Some(command), Auth::DeployKey, false));
        assert!(!yaml.contains("paths:"), "{yaml}");
        assert!(
            yaml.contains("        run: |\n          cargo run --bin openapi > api/openapi.json\n"),
            "{yaml}"
        );
        let parsed: Value = serde_norway::from_str(&yaml).unwrap();
        let steps = &parsed["jobs"]["push"]["steps"];
        assert_eq!(steps[2]["uses"], ACTION);
        assert_eq!(steps[2]["with"]["spec"], "api/openapi.json");
        assert_eq!(steps[2]["with"]["to"], "acme/api-sdks");
        assert_eq!(
            steps[2]["with"]["deploy-key"],
            "${{ secrets.PERSEID_SDKS_DEPLOY_KEY }}"
        );
    }

    #[test]
    fn workflows_read_back_as_their_settings() {
        for (on, build, auth, private) in [
            (PushOn::Change, None, Auth::DeployKey, false),
            (PushOn::Release, Some("make spec"), Auth::Token, true),
            (PushOn::Tag, None, Auth::App, false),
        ] {
            let read = pushed(&push_workflow(&push(on, build, auth, private))).unwrap();
            assert_eq!(
                read,
                Pushed {
                    hub: "acme/api-sdks".into(),
                    on,
                    tags: (on == PushOn::Tag).then(|| "api-v*".into()),
                    spec: "api/openapi.json".into(),
                    build: build.map(str::to_owned),
                    auth,
                    private,
                }
            );
        }
        assert_eq!(pushed("name: Spec\n"), None);
    }

    #[test]
    fn apps_mint_a_token_for_the_sdks_repository_only() {
        let yaml = push_workflow(&push(PushOn::Change, None, Auth::App, false));
        let parsed: Value = serde_norway::from_str(&yaml).unwrap();
        let steps = &parsed["jobs"]["push"]["steps"];
        assert_eq!(steps[1]["uses"], "actions/create-github-app-token@v2");
        assert_eq!(steps[1]["with"]["owner"], "acme");
        assert_eq!(steps[1]["with"]["repositories"], "api-sdks");
        assert_eq!(steps[2]["with"]["token"], "${{ steps.app.outputs.token }}");
        assert!(steps[2]["with"].get("deploy-key").is_none(), "{yaml}");
    }
}
