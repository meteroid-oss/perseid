//! The one-way link from the repository holding the spec to the SDKs repository receiving it: a
//! workflow running `perseid push-spec`, as the hosted perseid App, the GitHub App of the SDKs or
//! with a token.

use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Ui,
    api::GitHub,
    app::{APP_ID, APP_KEY},
    plan::TOKEN,
    secrets,
};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Auth {
    /// The hosted perseid App, installed on the SDKs repository, whose perseid.toml names this
    /// repository as its `source`. Nothing to store here.
    Perseid,
    /// The GitHub App `perseid app` set up for the SDKs, with a new key of it.
    App,
    /// A fine-grained token with Contents read and write on the SDKs repository.
    Token,
}

/// Asks for a fine-grained token reaching `hub` (SDK_GITHUB_TOKEN, when set, answers), and stores
/// it as the SDK_GITHUB_TOKEN secret of `repo`.
pub fn add_token(api: &GitHub, repo: &str, hub: &str, ui: &Ui) -> Result<()> {
    let owner = hub.split('/').next().unwrap_or(hub);
    let url = "https://github.com/settings/personal-access-tokens/new";
    let token = match std::env::var(TOKEN).ok().filter(|t| !t.trim().is_empty()) {
        Some(token) => token,
        None if ui.yes => bail!("--yes takes the token from SDK_GITHUB_TOKEN, which isn't set"),
        None => {
            ui.say(&format!(
                "Create a fine-grained token: resource owner {owner}, repository {hub} only, Contents read and write: {url}"
            ));
            ui.open(url);
            ui.secret("Paste the token")?
        }
    };
    let token = token.trim();
    if GitHub::new(Some(token.to_owned()))
        .find(&format!("/repos/{hub}"))?
        .is_none()
    {
        bail!("this token can't see {hub}: give it access to {hub}, then run the command again");
    }
    secrets::set_secret(api, repo, TOKEN, token)?;
    ui.ok(&format!("{TOKEN} is set on {repo}"));
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
        None => Auth::Perseid,
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
        true => "          private: true\n",
        false => "",
    };
    let hub = push.hub;
    let (who, app, credential) = match push.auth {
        Auth::Perseid => (
            format!(
                "It pushes as the perseid App, installed on {hub}, whose perseid.toml names this repository as its source."
            ),
            String::new(),
            String::new(),
        ),
        Auth::Token => (
            format!("{TOKEN} is a fine-grained token that can write to {hub}."),
            String::new(),
            format!("          token: ${{{{ secrets.{TOKEN} }}}}\n"),
        ),
        Auth::App => {
            let (owner, name) = hub.split_once('/').unwrap_or((hub, hub));
            (
                format!(
                    "It pushes as the GitHub App of {APP_ID} and {APP_KEY}, with a token for {hub} only."
                ),
                format!(
                    "      - id: app\n        uses: actions/create-github-app-token@v2\n        with:\n          app-id: ${{{{ vars.{APP_ID} }}}}\n          private-key: ${{{{ secrets.{APP_KEY} }}}}\n          owner: {owner}\n          repositories: {name}\n          permission-contents: write\n"
                ),
                "          token: ${{ steps.app.outputs.token }}\n".to_owned(),
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
  contents: read{id_token}

concurrency: perseid-push

jobs:
  push:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
{build}{app}      - uses: {action}
        with:
          spec: {spec:?}
          to: {hub}
{credential}{private}"#,
        id_token = match push.auth {
            Auth::Perseid => "\n  id-token: write",
            _ => "",
        },
        spec = push.spec,
        action = super::uses("push"),
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
        let yaml = push_workflow(&push(PushOn::Change, Some(command), Auth::Token, false));
        assert!(!yaml.contains("paths:"), "{yaml}");
        assert!(
            yaml.contains("        run: |\n          cargo run --bin openapi > api/openapi.json\n"),
            "{yaml}"
        );
        let parsed: Value = serde_norway::from_str(&yaml).unwrap();
        let steps = &parsed["jobs"]["push"]["steps"];
        assert_eq!(steps[2]["uses"], super::super::uses("push"));
        assert_eq!(steps[2]["with"]["spec"], "api/openapi.json");
        assert_eq!(steps[2]["with"]["to"], "acme/api-sdks");
        assert_eq!(steps[2]["with"]["token"], "${{ secrets.SDK_GITHUB_TOKEN }}");
    }

    #[test]
    fn workflows_read_back_as_their_settings() {
        for (on, build, auth, private) in [
            (PushOn::Change, None, Auth::Token, false),
            (PushOn::Release, Some("make spec"), Auth::Token, true),
            (PushOn::Tag, None, Auth::App, false),
            (PushOn::Change, None, Auth::Perseid, true),
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
    }

    #[test]
    fn the_perseid_app_needs_no_secret_but_an_oidc_token() {
        let yaml = push_workflow(&push(PushOn::Change, None, Auth::Perseid, false));
        let parsed: Value = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(parsed["permissions"]["id-token"], "write");
        let with = &parsed["jobs"]["push"]["steps"][1]["with"];
        assert_eq!(with["to"], "acme/api-sdks");
        assert!(with.get("token").is_none(), "{yaml}");
        assert!(!yaml.contains("secrets."), "{yaml}");
        assert!(yaml.ends_with("          to: acme/api-sdks\n"), "{yaml}");
    }
}
