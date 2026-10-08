//! `[targets]`: what perseid writes besides the SDKs, each into the one folder of a repository it
//! owns there, through pull requests. A `docs` target writes the spec and the docs data, once
//! the SDKs they describe are released, or with the SDK pull requests. A pack target renders a
//! program wrapping one SDK, such as a CLI, once that SDK is released.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    changelog::Changelog,
    config::{After, Config, Sdk, TargetKind, TargetTable, same_repo},
    generate::{self, Change, GENERATION, Options},
    pack,
    pr::{self, Bump},
    scaffold::package_path,
    sizing,
};

/// A `[targets.<name>]` table, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub name: String,
    pub kind: Kind,
    pub repo: String,
    /// The folder of `repo` perseid owns, without a trailing `/`: `.` for the whole repository.
    pub path: String,
    pub after: After,
}

/// What a target writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Docs,
    /// The pack at `dir`, relative to perseid.toml, wrapping the SDK of `wraps`.
    Pack {
        dir: String,
        wraps: &'static str,
    },
}

impl Target {
    pub fn of(name: &str, table: &TargetTable) -> Result<Self> {
        let at = format!("[targets.{name}]");
        ensure!(
            name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{at}: name targets in lowercase letters, digits and dashes"
        );
        let kinds = TargetKind::ALL.map(TargetKind::name).join(", ");
        let kind = match (&table.pack, table.kind) {
            (Some(_), Some(kind)) => bail!(
                "{at}: `pack` makes it a pack target, delete `kind = \"{}\"`",
                kind.name()
            ),
            (Some(dir), None) => {
                ensure!(
                    !dir.is_empty()
                        && !["gh:", "github:", "git@", "http://", "https://"]
                            .iter()
                            .any(|p| dir.starts_with(p)),
                    "{at}: `pack = {dir:?}`: perseid reads packs from a folder for now, set its path relative to perseid.toml, such as \"../packs/cli\""
                );
                let wraps = table.wraps.with_context(|| {
                    format!("{at}: set `wraps`, the language of the SDK the pack wraps")
                })?;
                Kind::Pack {
                    dir: dir.clone(),
                    wraps: wraps.name(),
                }
            }
            (None, _) if table.wraps.is_some() => {
                bail!("{at}: `wraps` names the SDK a pack wraps, set `pack` too")
            }
            (None, Some(TargetKind::Docs)) => Kind::Docs,
            (None, None) => match TargetKind::ALL.into_iter().find(|k| k.name() == name) {
                Some(TargetKind::Docs) => Kind::Docs,
                None => bail!(
                    "{at}: no target kind is named `{name}`. Name the table after its kind, or set `kind`, among {kinds}, or set `pack`"
                ),
            },
        };
        let path = match (&kind, table.path.as_deref()) {
            (Kind::Pack { .. }, None | Some(".")) => ".",
            (Kind::Pack { .. }, Some(path)) => path.trim_end_matches('/'),
            (Kind::Docs, path) => path.unwrap_or("api").trim_end_matches('/'),
        };
        let parts: Vec<&str> = path.split('/').collect();
        let what = match kind {
            Kind::Docs => "such as \"api\" or \"docs/api\": perseid replaces it whole",
            Kind::Pack { .. } => "such as \"cli\", or \".\" for the whole repository",
        };
        ensure!(
            path == "." && matches!(kind, Kind::Pack { .. })
                || !path.is_empty()
                    && !path.starts_with('/')
                    && parts.iter().all(|p| !p.is_empty()
                        && *p != "."
                        && *p != ".."
                        && !p.starts_with(".git")),
            "{at}: `path = {path:?}` must be a folder of {}, {what}",
            table.repo
        );
        ensure!(!table.repo.is_empty(), "{at}: set `repo`, `owner/name`");
        let after = match (&kind, table.after) {
            (Kind::Pack { wraps, .. }, Some(After::Sdk(language))) if language.name() != *wraps => {
                bail!(
                    "{at}: `after = \"{}\"` waits for an SDK the pack doesn't wrap: set \"{wraps}\", \"sdks\" or \"generate\"",
                    language.name()
                )
            }
            (_, Some(after)) => after,
            (Kind::Pack { .. }, None) => table.wraps.map_or(After::Sdks, After::Sdk),
            (Kind::Docs, None) => After::Sdks,
        };
        Ok(Target {
            name: name.to_owned(),
            kind,
            repo: table.repo.clone(),
            path: path.to_owned(),
            after,
        })
    }

    /// The branch of its pull request.
    pub fn branch(&self) -> String {
        format!("perseid/targets/{}", self.name)
    }
}

/// Fails when a pack target writes in the folder of an SDK or of another target, or in one
/// holding it: perseid would replace or delete the files of one generating the other.
pub(crate) fn overlaps(config: &Config) -> Result<()> {
    let targets = config.targets(&[])?;
    let sdks = config.sdks(&[]).unwrap_or_default();
    let nested = |a: &str, b: &str| {
        let (a, b) = (a.trim_end_matches('/'), b.trim_end_matches('/'));
        a == "."
            || b == "."
            || a == b
            || b.starts_with(&format!("{a}/"))
            || a.starts_with(&format!("{b}/"))
    };
    let place = |repo: &str, path: &str| match path {
        "." => repo.to_owned(),
        path => format!("{repo} ({path}/)"),
    };
    for (i, target) in targets.iter().enumerate() {
        let at = format!("[targets.{}]", target.name);
        let pack = matches!(target.kind, Kind::Pack { .. });
        for other in &targets[i + 1..] {
            ensure!(
                !(pack || matches!(other.kind, Kind::Pack { .. }))
                    || !same_repo(&target.repo, &other.repo)
                    || !nested(&target.path, &other.path),
                "{at} writes in {}, and [targets.{}] in {}: set `path` to folders apart",
                place(&target.repo, &target.path),
                other.name,
                place(&other.repo, &other.path)
            );
        }
        let packages = sdks
            .iter()
            .filter(|_| pack)
            .filter_map(|s| Package::of(config, s));
        for package in packages {
            ensure!(
                !same_repo(&target.repo, &package.repo) || !nested(&target.path, &package.path),
                "{at} writes in {}, and the {} SDK in {}: set `path` to folders apart",
                place(&target.repo, &target.path),
                package.language,
                place(&package.repo, &package.path)
            );
        }
    }
    Ok(())
}

/// The files of a docs target, relative to its folder: the spec as perseid read it, the docs
/// data, and the attributes collapsing their diffs on GitHub.
pub fn docs_files(spec: &str, data: &Value) -> Result<Vec<(String, Vec<u8>)>> {
    let spec: Value = serde_json::from_str(spec)?;
    let pretty = |value: &Value| -> Result<Vec<u8>> {
        Ok((serde_json::to_string_pretty(value)? + "\n").into_bytes())
    };
    Ok(vec![
        (
            ".gitattributes".to_owned(),
            b"* linguist-generated=true\n".to_vec(),
        ),
        ("docs-data.json".to_owned(), pretty(data)?),
        ("openapi.json".to_owned(), pretty(&spec)?),
    ])
}

/// Replaces the folder `dir` with `files`: the changes, by path relative to `dir`. With `check`,
/// only reports them.
pub fn write_folder(dir: &Path, files: &[(String, Vec<u8>)], check: bool) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    let existing = match dir.is_dir() {
        true => crate::assets::walk(dir)?,
        false => vec![],
    };
    for path in existing {
        let relative = path.strip_prefix(dir)?.to_path_buf();
        if !files.iter().any(|(f, _)| Path::new(f) == relative) {
            if !check {
                std::fs::remove_file(&path)?;
            }
            changes.push(Change::Removed(relative));
        }
    }
    for (path, content) in files {
        let target = dir.join(path);
        match std::fs::read(&target) {
            Ok(old) if &old == content => continue,
            Ok(_) => changes.push(Change::Modified(path.into())),
            Err(_) => changes.push(Change::Added(path.into())),
        }
        if !check {
            crate::fsx::write(&target, content)?;
        }
    }
    if dir.is_dir() {
        remove_empty(dir)?;
    }
    changes.sort_by_key(|c| match c {
        Change::Added(p) | Change::Modified(p) | Change::Removed(p) => p.clone(),
    });
    Ok(changes)
}

fn remove_empty(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_empty(&path)?;
            let _ = std::fs::remove_dir(&path);
        }
    }
    Ok(())
}

/// What the release check reads of GitHub.
pub trait Remote {
    /// The head branches of the open pull requests of `repo`.
    fn open_branches(&self, repo: &str) -> Result<Vec<String>>;
    /// The text of `path` in `repo`, on its default branch or at the ref `at`.
    fn file(&self, repo: &str, at: Option<&str>, path: &str) -> Result<Option<String>>;
}

impl Remote for pr::Client {
    fn open_branches(&self, repo: &str) -> Result<Vec<String>> {
        let pulls = (self.api()?).get(&format!("/repos/{repo}/pulls?state=open&per_page=100"))?;
        Ok((pulls.as_array().into_iter().flatten())
            .filter_map(|p| p["head"]["ref"].as_str().map(str::to_owned))
            .collect())
    }

    fn file(&self, repo: &str, at: Option<&str>, path: &str) -> Result<Option<String>> {
        let api = self.api()?;
        let at = match at {
            Some(at) => at.to_owned(),
            None => (api.get(&format!("/repos/{repo}"))?["default_branch"].as_str())
                .context("GitHub answered a repository without a default branch")?
                .to_owned(),
        };
        let text = api.raw(repo, &at, path)?;
        Ok(text.map(|t| String::from_utf8_lossy(&t).into_owned()))
    }
}

/// An SDK as released: its repository, and the path release-please knows its package by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub language: String,
    pub repo: String,
    pub path: String,
}

impl Package {
    /// Where `sdk` is released, unless its repository isn't on GitHub.
    pub fn of(config: &Config, sdk: &Sdk) -> Option<Self> {
        let (repo, path) = match sdk.remote() {
            Some(repo) => (repo.to_owned(), package_path("", &sdk.path)),
            None => (
                config.home.repo()?.to_owned(),
                package_path(&config.home.dir, &sdk.path),
            ),
        };
        Some(Package {
            language: sdk.language.to_owned(),
            repo,
            path,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Release {
    /// Its latest release holds what perseid generates now, at this version.
    Released(String),
    /// Why its latest release doesn't.
    Pending(String),
}

/// Whether the latest release of `package` holds what perseid generates from the current spec,
/// once `generate --pr` opened the pull requests it calls for: none of its update pull requests
/// is open, and its [`GENERATION`] at the default branch is the one at its latest release tag.
pub fn release(remote: &impl Remote, package: &Package) -> Result<Release> {
    let Package {
        language,
        repo,
        path,
    } = package;
    let open = remote.open_branches(repo)?;
    if let Some(branch) = open
        .iter()
        .find(|b| *b == pr::BRANCH || **b == pr::branch_of(language))
    {
        return Ok(Release::Pending(format!(
            "its pull request from {branch} isn't merged"
        )));
    }
    let json = |at: Option<&str>, file: &str| -> Result<Value> {
        let text = remote.file(repo, at, file)?.unwrap_or_default();
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    };
    let Some(version) = json(None, ".release-please-manifest.json")?[path.as_str()]
        .as_str()
        .map(str::to_owned)
    else {
        return Ok(Release::Pending("it has no release yet".into()));
    };
    let config = json(None, "release-please-config.json")?;
    let Some(tag) = tag(&config, path, &version) else {
        return Ok(Release::Pending(format!(
            "release-please-config.json has no package {path}"
        )));
    };
    let marker = package_path(path, GENERATION);
    let generated = remote.file(repo, None, &marker)?;
    if remote.file(repo, Some(&tag), &marker)? != generated {
        return Ok(Release::Pending(format!(
            "{tag} predates what perseid generated last"
        )));
    }
    Ok(Release::Released(version))
}

/// The release tag of `version` of the package at `path`, as release-please names it.
fn tag(config: &Value, path: &str, version: &str) -> Option<String> {
    let package = config["packages"].get(path)?;
    let setting = |key: &str| package.get(key).or_else(|| config.get(key));
    let v = match setting("include-v-in-tag").and_then(Value::as_bool) {
        Some(false) => "",
        _ => "v",
    };
    let component = setting("component").and_then(Value::as_str);
    Some(
        match (
            setting("include-component-in-tag").and_then(Value::as_bool),
            component,
        ) {
            (Some(false), _) | (_, None) => format!("{v}{version}"),
            (_, Some(component)) => {
                let separator = setting("tag-separator").and_then(Value::as_str);
                format!("{component}{}{v}{version}", separator.unwrap_or("-"))
            }
        },
    )
}

/// The release of each SDK of `config`, by language, a failure to tell counted as pending.
pub fn releases(remote: &impl Remote, config: &Config) -> Result<BTreeMap<String, Release>> {
    let mut releases = BTreeMap::new();
    for sdk in config.sdks(&[])? {
        let release = match Package::of(config, &sdk) {
            Some(package) => release(remote, &package)
                .unwrap_or_else(|error| Release::Pending(format!("{error:#}"))),
            None => Release::Pending("the repository holding it isn't on GitHub".into()),
        };
        releases.insert(sdk.language.to_owned(), release);
    }
    Ok(releases)
}

/// The released versions among `releases`.
pub fn versions(releases: &BTreeMap<String, Release>) -> BTreeMap<String, String> {
    (releases.iter())
        .filter_map(|(language, release)| match release {
            Release::Released(version) => Some((language.clone(), version.clone())),
            Release::Pending(_) => None,
        })
        .collect()
}

/// The files a docs target writes into its folder.
fn docs(
    config: &Config,
    root: &Path,
    spec: &str,
    versions: &BTreeMap<String, String>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let sdks = config.sdks(&[])?;
    let data = crate::docs_data::run(config, root, &sdks, spec, versions)?;
    docs_files(spec, &data)
}

/// Writes `target` into `dir`, its folder, depending on the SDKs at `versions`, the released
/// ones: the changes, and the files besides the generated ones to commit, relative to `dir`.
fn write(
    config: &Config,
    root: &Path,
    target: &Target,
    dir: &Path,
    spec: &str,
    versions: &BTreeMap<String, String>,
    options: &Options,
) -> Result<(Vec<Change>, Vec<PathBuf>)> {
    match &target.kind {
        Kind::Docs => {
            let files = docs(config, root, spec, versions)?;
            Ok((write_folder(dir, &files, options.check)?, vec![]))
        }
        Kind::Pack { wraps, .. } => {
            let released = versions.get(*wraps).map(String::as_str);
            let generated = pack::generate(config, root, target, dir, spec, released, options)?;
            let mut files = generated.scaffold;
            files.push(pack::SPEC.into());
            Ok((generated.changes, files))
        }
    }
}

/// Writes each of `targets` into `<out>/<name>`: what changed there. With `check`, only
/// reports it.
pub fn preview(
    config: &Config,
    root: &Path,
    targets: &[Target],
    spec: &str,
    out: &Path,
    options: &Options,
) -> Result<BTreeMap<String, Vec<Change>>> {
    let mut changes = BTreeMap::new();
    for target in targets {
        let dir = out.join(&target.name);
        let (written, _) = write(config, root, target, &dir, spec, &BTreeMap::new(), options)?;
        changes.insert(target.name.clone(), written);
    }
    Ok(changes)
}

/// What `--pr` asks of the pull request of each target.
pub struct Request {
    /// `auto` sizes it against the spec its folder holds.
    pub bump: Bump,
    pub relax_enum_additions: bool,
    pub auto_merge: bool,
    /// Runs the formatters of pack targets.
    pub format: bool,
}

/// The SDKs of `releases` that `after` waits for and are not released, with why.
fn waiting(after: After, releases: &BTreeMap<String, Release>) -> Vec<String> {
    (releases.iter())
        .filter(|(language, _)| match after {
            After::Generate => false,
            After::Sdks => true,
            After::Sdk(awaited) => awaited.name() == language.as_str(),
        })
        .filter_map(|(language, release)| match release {
            Release::Pending(why) => Some(format!("{language} ({why})")),
            Release::Released(_) => None,
        })
        .collect()
}

/// Opens or updates the pull request of each of `targets` whose `after` holds.
pub fn deliver(
    github: &pr::Client,
    config: &Config,
    root: &Path,
    targets: &[Target],
    spec: &str,
    request: &Request,
) -> Result<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let releases = releases(github, config)?;
    let versions = versions(&releases);
    let origin = pr::origin(config, root, spec);
    let options = Options {
        check: false,
        format: request.format,
    };
    for target in targets {
        let at = format!("[targets.{}]", target.name);
        let pending = waiting(target.after, &releases);
        if !pending.is_empty() {
            println!(
                "{at}: waits for the release of {}, then opens its pull request on {}",
                pending.join(", "),
                target.repo
            );
            continue;
        }
        let checkout = pr::checkout(&target.repo, root, true)?;
        let dir = checkout.join(&target.path);
        let (snapshot, subject, described) = match &target.kind {
            Kind::Docs => (
                "openapi.json",
                format!("update the API reference to {}", api_name(spec)),
                releases.clone(),
            ),
            Kind::Pack { wraps, .. } => (
                pack::SPEC,
                format!("update to {}", api_name(spec)),
                (releases.iter())
                    .filter(|(language, _)| language == wraps)
                    .map(|(l, r)| (l.clone(), r.clone()))
                    .collect(),
            ),
        };
        let scratch = tempfile::tempdir()?;
        let previous = scratch.path().join("openapi.json");
        let had_spec = std::fs::copy(dir.join(snapshot), &previous).is_ok();
        let (mut changes, files) = write(config, root, target, &dir, spec, &versions, &options)?;
        pr::record(&checkout)?;
        let within = |p: &Path| generate::clean(&Path::new(&target.path).join(p));
        let (workflows, files): (Vec<_>, Vec<_>) =
            (files.into_iter()).partition(|f| within(f).starts_with(WORKFLOWS));
        changes.retain(|c| !matches!(c, Change::Added(p) if workflows.contains(p)));
        if changes.is_empty() {
            println!("{at}: up to date in {}", target.repo);
            continue;
        }
        let (bump, changelog) = match (request.bump, had_spec) {
            (Bump::Auto, true) => sizing::size(&sizing::Comparison {
                root: &dir,
                spec: snapshot,
                base: Some(&previous),
                relax_enum_additions: request.relax_enum_additions,
            })?,
            (Bump::Auto, false) => (Bump::Minor, Changelog::default()),
            (bump, _) => (bump, Changelog::default()),
        };
        let folder = [target.path.clone()];
        let files: Vec<String> = (files.iter())
            .filter(|f| dir.join(f).is_file())
            .map(|f| within(f).to_string_lossy().into_owned())
            .collect();
        let update = match target.kind {
            Kind::Docs => pr::Update {
                branch: &target.branch(),
                paths: &[],
                files: &[],
                shared: &[],
                owned: &folder,
            },
            Kind::Pack { .. } => pr::Update {
                branch: &target.branch(),
                paths: &folder,
                files: &files,
                shared: &[],
                owned: &[],
            },
        };
        let shown = (changes.into_iter())
            .map(|c| match c {
                Change::Added(p) => Change::Added(within(&p)),
                Change::Modified(p) => Change::Modified(within(&p)),
                Change::Removed(p) => Change::Removed(within(&p)),
            })
            .collect();
        let summary = generate::summary(
            &BTreeMap::from([(target.name.clone(), shown)]),
            &BTreeMap::new(),
        );
        let workflows = workflows_note(
            target,
            &workflows.iter().map(|w| within(w)).collect::<Vec<_>>(),
        );
        let describe = |listed: &str| {
            let listed = [listed, &workflows].into_iter().filter(|s| !s.is_empty());
            describe(
                &summary,
                &listed.collect::<Vec<_>>().join("\n\n"),
                &described,
                origin.as_deref(),
            )
        };
        let changelog = &changelog;
        match pr::open(
            github, &checkout, &update, bump, &subject, changelog, describe,
        )? {
            Some(pull) => {
                println!("{}", pull.url);
                if request.auto_merge {
                    pr::auto_merge(github, &pull)?;
                }
            }
            None => println!("{at}: nothing to update in {}", target.repo),
        }
    }
    Ok(())
}

/// Where GitHub reads workflows, which the tokens of CI runs can't write.
const WORKFLOWS: &str = ".github/workflows";

/// What a pull request says of the `workflows` of a pack's scaffold it leaves out.
fn workflows_note(target: &Target, workflows: &[PathBuf]) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    let listed: Vec<String> = (workflows.iter())
        .map(|w| format!("- `{}`", w.display()))
        .collect();
    format!(
        "The scaffold also writes these workflows, which pull requests from CI can't carry:\n\n{}\n\n\
         Add them by hand: `perseid targets {} --out <dir>`, then commit them from `<dir>/{}`.",
        listed.join("\n"),
        target.name,
        target.name
    )
}

/// The description of a target's pull request.
fn describe(
    summary: &str,
    listed: &str,
    releases: &BTreeMap<String, Release>,
    origin: Option<&str>,
) -> String {
    let versions: Vec<String> = (releases.iter())
        .map(|(language, release)| match release {
            Release::Released(version) => format!("{language} {version}"),
            Release::Pending(_) => format!("{language} (unreleased)"),
        })
        .collect();
    let listed = match listed {
        "" => String::new(),
        listed => format!("{listed}\n\n"),
    };
    let from = origin.map_or_else(String::new, |o| format!(" from {o}"));
    format!(
        "```\n{summary}\n```\n\n{listed}SDK versions: {}.\n\nGenerated by [perseid](https://github.com/meteroid-oss/perseid){from}.",
        versions.join(", ")
    )
}

/// `Acme 1.2.0`: the title and version of the spec.
pub fn api_name(spec: &str) -> String {
    let spec: Value = serde_json::from_str(spec).unwrap_or_default();
    let name = format!(
        "{} {}",
        spec["info"]["title"].as_str().unwrap_or("the API"),
        spec["info"]["version"].as_str().unwrap_or_default()
    );
    name.trim().to_owned()
}

/// The repositories `targets` write to, for the tokens of the runs.
pub fn repos(targets: &[Target]) -> Vec<String> {
    let mut repos: Vec<String> = targets.iter().map(|t| t.repo.clone()).collect();
    repos.sort();
    repos.dedup();
    repos
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;

    use super::*;
    use crate::config::Language;

    fn config(toml: &str) -> Result<Config> {
        let text = format!("name = \"Acme\"\nsdks = [\"typescript\", \"go\"]\n{toml}");
        Config::parse(&text, "perseid.toml")
    }

    fn error(toml: &str) -> String {
        format!("{:#}", config(toml).err().expect("an invalid perseid.toml"))
    }

    #[test]
    fn targets_take_their_kind_from_their_name_unless_set() {
        let config = config(
            "[targets.docs]\nrepo = \"acme/docs\"\n\
             [targets.public]\nkind = \"docs\"\nrepo = \"acme/site\"\npath = \"reference/api/\"\nafter = \"generate\"\n",
        )
        .unwrap();
        assert_eq!(
            config.targets(&[]).unwrap(),
            [
                Target {
                    name: "docs".into(),
                    kind: Kind::Docs,
                    repo: "acme/docs".into(),
                    path: "api".into(),
                    after: After::Sdks,
                },
                Target {
                    name: "public".into(),
                    kind: Kind::Docs,
                    repo: "acme/site".into(),
                    path: "reference/api".into(),
                    after: After::Generate,
                },
            ]
        );
        assert_eq!(
            config.targets(&["public".into()]).unwrap()[0].branch(),
            "perseid/targets/public"
        );
        assert!(config.awaits_releases());
        let missing = format!("{:#}", config.targets(&["cli".into()]).unwrap_err());
        assert!(missing.contains("no [targets.cli]"), "{missing}");
    }

    #[test]
    fn unknown_kinds_and_paths_outside_one_folder_are_refused() {
        let cli = error("[targets.cli]\nrepo = \"acme/cli\"\n");
        assert!(
            cli.contains("[targets.cli]: no target kind is named `cli`")
                && cli.contains("among docs"),
            "{cli}"
        );
        let kind = error("[targets.x]\nkind = \"mcp\"\nrepo = \"acme/x\"\n");
        assert!(kind.contains("unknown variant `mcp`"), "{kind}");
        for path in ["", ".", "/api", "../api", "docs/../..", ".github", "a//b"] {
            let text = error(&format!(
                "[targets.docs]\nrepo = \"acme/docs\"\npath = {path:?}\n"
            ));
            assert!(
                text.contains("must be a folder of acme/docs"),
                "{path}: {text}"
            );
        }
        let name = error("[targets.Docs]\nkind = \"docs\"\nrepo = \"acme/docs\"\n");
        assert!(name.contains("lowercase letters"), "{name}");
        assert!(error("[targets.docs]\npath = \"api\"\n").contains("missing field `repo`"));
        assert!(
            error("[targets.docs]\nrepo = \"a/b\"\nhost = \"x\"\n")
                .contains("unknown field `host`")
        );
        let unreleased = error("release = false\n[targets.docs]\nrepo = \"acme/docs\"\n");
        assert!(
            unreleased.contains("`after = \"generate\"`"),
            "{unreleased}"
        );
        let config =
            config("release = false\n[targets.docs]\nrepo = \"a/b\"\nafter = \"generate\"\n");
        assert!(!config.unwrap().awaits_releases());
    }

    #[test]
    fn pack_targets_wrap_an_sdk_and_may_wait_for_its_release_alone() {
        let config = config(
            "[targets.cli]\npack = \"../ext/packs/cli\"\nwraps = \"typescript\"\nrepo = \"acme/cli\"\nafter = \"typescript\"\n\
             [targets.tools]\npack = \"packs/tools\"\nwraps = \"go\"\nrepo = \"acme/tools\"\npath = \"tools/\"\nafter = \"generate\"\n\
             [targets.tui]\npack = \"packs/tui\"\nwraps = \"go\"\nrepo = \"acme/tools\"\npath = \"tui\"\n",
        )
        .unwrap();
        assert_eq!(
            config.targets(&[]).unwrap(),
            [
                Target {
                    name: "cli".into(),
                    kind: Kind::Pack {
                        dir: "../ext/packs/cli".into(),
                        wraps: "typescript",
                    },
                    repo: "acme/cli".into(),
                    path: ".".into(),
                    after: After::Sdk(Language::Typescript),
                },
                Target {
                    name: "tools".into(),
                    kind: Kind::Pack {
                        dir: "packs/tools".into(),
                        wraps: "go",
                    },
                    repo: "acme/tools".into(),
                    path: "tools".into(),
                    after: After::Generate,
                },
                Target {
                    name: "tui".into(),
                    kind: Kind::Pack {
                        dir: "packs/tui".into(),
                        wraps: "go",
                    },
                    repo: "acme/tools".into(),
                    path: "tui".into(),
                    after: After::Sdk(Language::Go),
                },
            ]
        );
        assert!(config.awaits_releases());

        let releases = BTreeMap::from([
            (
                "go".to_owned(),
                Release::Pending("it has no release yet".into()),
            ),
            ("typescript".to_owned(), Release::Released("1.4.0".into())),
        ]);
        let go = ["go (it has no release yet)".to_owned()];
        assert!(waiting(After::Sdk(Language::Typescript), &releases).is_empty());
        assert_eq!(waiting(After::Sdk(Language::Go), &releases), go);
        assert_eq!(waiting(After::Sdks, &releases), go);
        assert!(waiting(After::Generate, &releases).is_empty());
    }

    #[test]
    fn pack_targets_name_a_folder_and_an_sdk_perseid_toml_lists() {
        let cli = |table: &str| error(&format!("[targets.cli]\nrepo = \"acme/cli\"\n{table}"));
        let pack = "pack = \"packs/cli\"\n";
        for (table, expected) in [
            (
                format!("{pack}wraps = \"rust\"\n"),
                "[targets.cli] `wraps = \"rust\"` names an SDK that `sdks` doesn't list",
            ),
            (
                format!("{pack}wraps = \"go\"\nafter = \"typescript\"\n"),
                "[targets.cli]: `after = \"typescript\"` waits for an SDK the pack doesn't wrap: set \"go\", \"sdks\" or \"generate\"",
            ),
            (
                format!("{pack}wraps = \"go\"\nafter = \"ruby\"\n"),
                "`after = \"ruby\"`: expected \"sdks\", \"generate\" or a language, among rust,",
            ),
            (
                format!("{pack}wraps = \"go\"\nkind = \"docs\"\n"),
                "[targets.cli]: `pack` makes it a pack target, delete `kind = \"docs\"`",
            ),
            (
                "wraps = \"go\"\n".to_owned(),
                "[targets.cli]: `wraps` names the SDK a pack wraps, set `pack` too",
            ),
            (
                pack.to_owned(),
                "[targets.cli]: set `wraps`, the language of the SDK the pack wraps",
            ),
            (
                "pack = \"gh:acme/packs/cli@v0\"\nwraps = \"go\"\n".to_owned(),
                "perseid reads packs from a folder for now",
            ),
            (
                format!("{pack}wraps = \"go\"\npath = \"../cli\"\n"),
                "`path = \"../cli\"` must be a folder of acme/cli, such as \"cli\"",
            ),
        ] {
            let text = cli(&table);
            assert!(text.contains(expected), "{table}: {text}");
        }
        let unreleased = error(
            "release = false\n[targets.cli]\nrepo = \"a/b\"\npack = \"p\"\nwraps = \"go\"\nafter = \"go\"\n",
        );
        assert!(
            unreleased.contains("(`after = \"go\"`), which `release = false` leaves to you"),
            "{unreleased}"
        );
    }

    #[test]
    fn pack_targets_write_apart_from_the_sdks_and_the_other_targets() {
        let toml = "repo = \"acme/acme-{lang}\"\n[go]\nrepo = \"acme/sdks\"\npath = \"go\"\n\
                    [targets.docs]\nrepo = \"acme/site\"\npath = \"reference/api\"\n\
                    [targets.cli]\npack = \"packs/cli\"\nwraps = \"go\"\n";
        let pack = |table: &str| config(&format!("{toml}{table}"));
        for apart in [
            "repo = \"acme/cli\"\n",
            "repo = \"acme/sdks\"\npath = \"cli\"\n",
            "repo = \"acme/sdks\"\npath = \"gopher\"\n",
            "repo = \"acme/site\"\npath = \"reference/cli\"\n",
        ] {
            assert!(pack(apart).is_ok(), "{apart}");
        }
        for (table, expected) in [
            (
                "repo = \"acme/sdks\"\n",
                "[targets.cli] writes in acme/sdks, and the go SDK in acme/sdks (go/)",
            ),
            (
                "repo = \"Acme/SDKs\"\npath = \"go/cli\"\n",
                "[targets.cli] writes in Acme/SDKs (go/cli/), and the go SDK in acme/sdks (go/)",
            ),
            (
                "repo = \"acme/acme-typescript\"\npath = \"cli\"\n",
                "and the typescript SDK in acme/acme-typescript: set `path` to folders apart",
            ),
            (
                "repo = \"acme/site\"\npath = \"reference\"\n",
                "[targets.cli] writes in acme/site (reference/), and [targets.docs] in acme/site (reference/api/)",
            ),
        ] {
            let text = format!("{:#}", pack(table).err().expect(table));
            assert!(text.contains(expected), "{table}: {text}");
        }
        let twice = error(
            "[targets.cli]\npack = \"p\"\nwraps = \"go\"\nrepo = \"acme/cli\"\n\
             [targets.tui]\npack = \"q\"\nwraps = \"go\"\nrepo = \"acme/cli\"\npath = \"tui\"\n",
        );
        assert!(
            twice
                .contains("[targets.cli] writes in acme/cli, and [targets.tui] in acme/cli (tui/)"),
            "{twice}"
        );
    }

    #[test]
    fn a_pack_target_depends_on_the_released_version_of_its_sdk() {
        let root = tempfile::tempdir().unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let toy = fixtures.join("packs/toy");
        let toml = format!(
            "name = \"Acme\"\nsdks = [\"rust\"]\n[targets.cli]\npack = {:?}\nwraps = \"rust\"\nrepo = \"acme/cli\"\nafter = \"rust\"\n",
            toy.display()
        );
        let config = Config::parse(&toml, "perseid.toml").unwrap();
        let target = config.targets(&[]).unwrap().remove(0);
        let spec = fixtures.join("petstore.yaml");
        let spec = crate::spec::read(spec.to_str().unwrap(), root.path()).unwrap();
        let options = Options {
            check: false,
            format: false,
        };
        let render = |dir: &Path, versions: &BTreeMap<String, String>| {
            write(
                &config,
                root.path(),
                &target,
                dir,
                &spec,
                versions,
                &options,
            )
            .unwrap()
        };
        let released = root.path().join("released");
        let versions = BTreeMap::from([("rust".to_owned(), "1.4.0".to_owned())]);
        let (changes, files) = render(&released, &versions);
        assert!(changes.contains(&Change::Added("src/commands/pets.rs".into())));
        assert_eq!(
            files,
            [
                PathBuf::from(".github/workflows/release.yml"),
                "Cargo.toml".into(),
                "src/main.rs".into(),
                pack::SPEC.into()
            ]
        );
        let read = |dir: &Path, file: &str| std::fs::read_to_string(dir.join(file)).unwrap();
        assert!(
            read(&released, "api.md").ends_with("Wraps `acme` 1.4.0, as released."),
            "{}",
            read(&released, "api.md")
        );
        assert!(read(&released, "Cargo.toml").contains("acme = \"1.4.0\"\n"));

        let current = root.path().join("current");
        render(&current, &BTreeMap::new());
        assert!(read(&current, "api.md").ends_with("Wraps `acme` 0.1.0."));
    }

    #[test]
    fn a_docs_target_owns_its_folder_and_nothing_else() {
        let repo = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            crate::fsx::write(&repo.path().join(path), text.as_bytes()).unwrap()
        };
        write("README.md", "# Docs\n");
        write("api.md", "kept\n");
        write("api/guide/old.md", "stale\n");
        write("api/openapi.json", "{}\n");
        let spec = r#"{"openapi":"3.1.0","info":{"title":"Acme","version":"1.2.0"},"paths":{}}"#;
        let data = json!({ "generated": "this file is @generated by perseid", "languages": {} });
        let files = docs_files(spec, &data).unwrap();
        let names: Vec<&str> = files.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(names, [".gitattributes", "docs-data.json", "openapi.json"]);
        assert_eq!(files[0].1, b"* linguist-generated=true\n");
        assert!(
            String::from_utf8_lossy(&files[2].1)
                .starts_with("{\n  \"openapi\": \"3.1.0\",\n  \"info\"")
        );

        let dir = repo.path().join("api");
        let changes = write_folder(&dir, &files, false).unwrap();
        let shown: Vec<String> = changes.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "+ .gitattributes",
                "+ docs-data.json",
                "- guide/old.md",
                "~ openapi.json"
            ]
        );
        assert!(!dir.join("guide").exists());
        let read = |path: &str| std::fs::read_to_string(repo.path().join(path)).unwrap();
        assert_eq!(
            (read("README.md"), read("api.md")),
            ("# Docs\n".into(), "kept\n".into())
        );
        assert!(write_folder(&dir, &files, false).unwrap().is_empty());
        assert!(api_name(spec) == "Acme 1.2.0" && api_name("{}") == "the API");
    }

    /// GitHub as fixtures: open pull requests by repository, files by `repo@ref:path`.
    #[derive(Default)]
    struct Fixture {
        open: BTreeMap<&'static str, Vec<&'static str>>,
        files: BTreeMap<String, String>,
        read: RefCell<Vec<String>>,
    }

    impl Fixture {
        fn with(mut self, at: &str, path: &str, text: &str) -> Self {
            self.files.insert(format!("{at}:{path}"), text.to_owned());
            self
        }
    }

    impl Remote for Fixture {
        fn open_branches(&self, repo: &str) -> Result<Vec<String>> {
            let open = self.open.get(repo).cloned().unwrap_or_default();
            Ok(open.into_iter().map(str::to_owned).collect())
        }

        fn file(&self, repo: &str, at: Option<&str>, path: &str) -> Result<Option<String>> {
            let key = format!("{repo}@{}:{path}", at.unwrap_or("HEAD"));
            self.read.borrow_mut().push(key.clone());
            Ok(self.files.get(&key).cloned())
        }
    }

    const MANIFEST: &str = r#"{ "typescript": "1.4.0", ".": "0.3.0" }"#;
    const RELEASE_CONFIG: &str = r#"{
        "tag-separator": "/",
        "packages": {
            "typescript": { "component": "typescript" },
            ".": { "component": "go", "include-component-in-tag": false }
        }
    }"#;

    fn package(language: &str, repo: &str, path: &str) -> Package {
        Package {
            language: language.into(),
            repo: repo.into(),
            path: path.into(),
        }
    }

    fn released(head: &str, tagged: &str) -> Fixture {
        Fixture::default()
            .with("acme/sdks@HEAD", ".release-please-manifest.json", MANIFEST)
            .with(
                "acme/sdks@HEAD",
                "release-please-config.json",
                RELEASE_CONFIG,
            )
            .with(
                "acme/sdks@HEAD",
                "typescript/.perseid/generation.json",
                head,
            )
            .with(
                "acme/sdks@typescript/v1.4.0",
                "typescript/.perseid/generation.json",
                tagged,
            )
    }

    #[test]
    fn an_sdk_is_released_once_its_latest_tag_holds_what_perseid_generated_last() {
        let typescript = package("typescript", "acme/sdks", "typescript");
        let fixture = released("b", "b");
        assert_eq!(
            release(&fixture, &typescript).unwrap(),
            Release::Released("1.4.0".into())
        );
        assert!(fixture.read.borrow().contains(
            &"acme/sdks@typescript/v1.4.0:typescript/.perseid/generation.json".to_owned()
        ));

        let merged = release(&released("b", "a"), &typescript).unwrap();
        assert_eq!(
            merged,
            Release::Pending("typescript/v1.4.0 predates what perseid generated last".into())
        );

        let mut open = released("b", "b");
        open.open
            .insert("acme/sdks", vec!["main", "perseid/update-typescript"]);
        assert_eq!(
            release(&open, &typescript).unwrap(),
            Release::Pending("its pull request from perseid/update-typescript isn't merged".into())
        );
        open.open.insert(
            "acme/sdks",
            vec!["perseid/update-go", "perseid/targets/docs"],
        );
        assert!(matches!(
            release(&open, &typescript).unwrap(),
            Release::Released(_)
        ));

        let unreleased = Fixture::default();
        assert_eq!(
            release(&unreleased, &typescript).unwrap(),
            Release::Pending("it has no release yet".into())
        );
        let unconfigured = package("python", "acme/sdks", "python");
        let manifest = r#"{ "python": "0.1.0" }"#;
        let fixture =
            released("b", "b").with("acme/sdks@HEAD", ".release-please-manifest.json", manifest);
        assert_eq!(
            release(&fixture, &unconfigured).unwrap(),
            Release::Pending("release-please-config.json has no package python".into())
        );
    }

    #[test]
    fn sdks_generated_before_the_record_count_as_released() {
        let go = package("go", "acme/sdks", ".");
        let fixture = released("b", "b");
        assert_eq!(
            release(&fixture, &go).unwrap(),
            Release::Released("0.3.0".into())
        );
        assert!(
            fixture
                .read
                .borrow()
                .contains(&"acme/sdks@v0.3.0:.perseid/generation.json".to_owned())
        );
    }

    #[test]
    fn release_tags_follow_release_please_settings() {
        let config: Value = serde_json::from_str(RELEASE_CONFIG).unwrap();
        assert_eq!(
            tag(&config, "typescript", "1.0.0").as_deref(),
            Some("typescript/v1.0.0")
        );
        assert_eq!(tag(&config, ".", "1.0.0").as_deref(), Some("v1.0.0"));
        assert_eq!(tag(&config, "python", "1.0.0"), None);
        let go = json!({ "packages": { "api/go": { "component": "api/go", "include-v-in-tag": false } } });
        assert_eq!(tag(&go, "api/go", "0.4.0").as_deref(), Some("api/go-0.4.0"));
    }

    #[test]
    fn versions_are_those_of_the_released_sdks() {
        let releases = BTreeMap::from([
            (
                "go".to_owned(),
                Release::Pending("it has no release yet".into()),
            ),
            ("typescript".to_owned(), Release::Released("1.4.0".into())),
        ]);
        assert_eq!(
            versions(&releases),
            BTreeMap::from([("typescript".to_owned(), "1.4.0".to_owned())])
        );
        let body = describe("docs: up to date", "", &releases, Some("`a1b2c3d`"));
        assert!(
            body.contains("SDK versions: go (unreleased), typescript 1.4.0.\n\nGenerated by [perseid](https://github.com/meteroid-oss/perseid) from `a1b2c3d`."),
            "{body}"
        );
    }

    #[test]
    fn sdks_are_released_from_the_repository_holding_them() {
        let toml = "repo = \"acme/acme-{lang}\"\n[go]\nrepo = \"acme/sdks\"\npath = \"go\"\n";
        let mut config = config(toml).unwrap();
        config.home.url = Some("https://github.com/acme/sdks".into());
        config.home.dir = "api".into();
        let sdks = config.sdks(&[]).unwrap();
        let packages: Vec<Option<Package>> = sdks.iter().map(|s| Package::of(&config, s)).collect();
        assert_eq!(
            packages,
            [
                Some(package("typescript", "acme/acme-typescript", ".")),
                Some(package("go", "acme/sdks", "api/go")),
            ]
        );
        let config = super::tests::config("").unwrap();
        let sdks = config.sdks(&[]).unwrap();
        assert_eq!(Package::of(&config, &sdks[0]), None);
    }
}
