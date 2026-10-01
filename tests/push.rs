//! The shell of perseid-push.yml, run against local repositories standing for the repository
//! holding the spec and the SDKs repository it pushes the spec to.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use perseid::github::{Push, PushOn, push_workflow};
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "advice.detachedHead=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (path, text) in files {
        fs::write(dir.join(path), text).unwrap();
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Repos {
    _dir: tempfile::TempDir,
    api: PathBuf,
    sdks: PathBuf,
}

/// An API repository, and an SDKs repository holding the spec of `synced` when set up.
fn repos() -> (Repos, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let api = dir.path().join("api");
    fs::create_dir(&api).unwrap();
    git(&api, &["init", "--quiet"]);
    let a = commit(&api, &[("openapi.json", "{\"v\": 0}\n")], "a");
    let b = commit(&api, &[("openapi.json", "{\"v\": 1}\n")], "b");
    let c = commit(&api, &[("README.md", "docs\n")], "c");
    let d = commit(&api, &[("openapi.json", "{\"v\": 2}\n")], "d");
    git(&api, &["checkout", "--quiet", "-b", "fork", &a]);
    let e = commit(&api, &[("openapi.json", "{\"v\": 3}\n")], "e");
    let sdks = dir.path().join("sdks.git");
    git(dir.path(), &["init", "--quiet", "--bare", "sdks.git"]);
    let (repos, shas) = (
        Repos {
            _dir: dir,
            api,
            sdks,
        },
        vec![a, b, c, d, e],
    );
    (repos, shas)
}

/// Commits perseid.toml to the SDKs repository, as `perseid init` writes it, and the spec of
/// `sha` when set, as a push would have.
fn seed(repos: &Repos, sha: Option<&str>) {
    seed_with(repos, true, sha);
}

fn seed_with(repos: &Repos, config: bool, sha: Option<&str>) {
    let work = repos.sdks.with_extension("work");
    git(
        repos.sdks.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            repos.sdks.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    let mut files = vec![("README.md", "SDKs\n".to_owned())];
    if config {
        files.push((
            "perseid.toml",
            "name = \"Acme\"\nsdks = [\"go\"]\n".to_owned(),
        ));
    }
    if let Some(sha) = sha {
        let spec = git(&repos.api, &["show", &format!("{sha}:openapi.json")]) + "\n";
        fs::create_dir_all(work.join(".perseid")).unwrap();
        let source = format!("{{\n  \"repo\": \"acme/api\",\n  \"sha\": \"{sha}\"\n}}\n");
        files.push((".perseid/source.json", source));
        files.push(("openapi.json", spec));
    }
    let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    commit(&work, &files, "setup");
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    fs::remove_dir_all(work).unwrap();
}

/// Runs the push step at `sha` of the API repository, returning its output.
fn push(repos: &Repos, sha: &str) -> String {
    push_at(repos, sha, "")
}

/// Runs the push step at `sha`, triggered by the release or tag `tag` when not empty.
fn push_at(repos: &Repos, sha: &str, tag: &str) -> String {
    push_with(repos, sha, tag, false, "")
}

/// Runs the push step at `sha`, `private` leaving out the name of the repository it runs in,
/// to an SDKs repository keeping perseid.toml in `dir`.
fn push_with(repos: &Repos, sha: &str, tag: &str, private: bool, dir: &str) -> String {
    let (destination, config) = (format!("{dir}openapi.json"), format!("{dir}perseid.toml"));
    let yaml = push_workflow(&Push {
        branch: "main",
        on: PushOn::Release,
        tags: "v*",
        spec: "openapi.json",
        build: None,
        hub: "acme/api-sdks",
        destination: &destination,
        config: &config,
        private,
    });
    let workflow: Value = serde_norway::from_str(&yaml).unwrap();
    let step = workflow["jobs"]["push"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Push the spec to the SDKs repository")
        .unwrap();
    git(&repos.api, &["checkout", "--quiet", sha]);
    let temp = tempfile::tempdir().unwrap();
    let url = format!("file://{}", repos.sdks.display());
    let output = Command::new("bash")
        .arg("-c")
        .arg(step["run"].as_str().unwrap())
        .current_dir(&repos.api)
        .env("SPEC", step["env"]["SPEC"].as_str().unwrap())
        .env("SDKS_SPEC", step["env"]["SDKS_SPEC"].as_str().unwrap())
        .env("SDKS_CONFIG", step["env"]["SDKS_CONFIG"].as_str().unwrap())
        .env(
            "SOURCE_REPOSITORY",
            match step["env"]["SOURCE_REPOSITORY"].as_str().unwrap() {
                "" => "",
                _ => "acme/api",
            },
        )
        .env("REF", tag)
        .env("KNOWN_HOSTS", step["env"]["KNOWN_HOSTS"].as_str().unwrap())
        .env("SDKS_REPO", "acme/api-sdks")
        .env(
            "DEPLOY_KEY",
            "unused: the clone URL is rewritten to a local path",
        )
        .env("RUNNER_TEMP", temp.path())
        .env("GITHUB_SHA", sha)
        .env("GITHUB_REPOSITORY", "acme/api")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", format!("url.{url}.insteadOf"))
        .env("GIT_CONFIG_VALUE_0", "git@github.com:acme/api-sdks.git")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{out}");
    assert!(
        temp.path().join("known_hosts").exists() && temp.path().join("deploy_key").exists(),
        "{out}"
    );
    out
}

fn sdks_head(repos: &Repos) -> String {
    git(&repos.sdks, &["rev-parse", "main"])
}

fn sdks_file(repos: &Repos, path: &str) -> String {
    git(&repos.sdks, &["show", &format!("main:{path}")])
}

#[test]
fn newer_specs_are_pushed_and_older_or_diverged_ones_skipped() {
    let (repos, shas) = repos();
    let [a, b, c, d, e] = shas.as_slice() else {
        unreachable!()
    };
    seed(&repos, Some(b));
    let seeded = sdks_head(&repos);

    let out = push(&repos, a);
    assert!(
        out.contains(&format!(
            "has the spec of {b}, which {a} doesn't descend from: skipped"
        )),
        "{out}"
    );
    let out = push(&repos, e);
    assert!(
        out.contains("doesn't descend from: skipped"),
        "diverged: {out}"
    );
    let out = push(&repos, c);
    assert!(out.contains("acme/api-sdks already has this spec"), "{out}");
    assert_eq!(sdks_head(&repos), seeded, "nothing committed");

    let out = push(&repos, d);
    assert!(
        out.contains(&format!(
            "Pushed the spec of acme/api@{} to acme/api-sdks",
            &d[..7]
        )),
        "{out}"
    );
    assert_eq!(sdks_file(&repos, "openapi.json"), "{\"v\": 2}");
    let source: Value = serde_json::from_str(&sdks_file(&repos, ".perseid/source.json")).unwrap();
    assert_eq!(source["sha"], d.as_str());
    assert_eq!(source["repo"], "acme/api");
    assert!(source.get("ref").is_none(), "{source}");
    let message = git(&repos.sdks, &["log", "-1", "--format=%s%n%an", "main"]);
    assert_eq!(
        message,
        format!("spec: acme/api@{}\ngithub-actions[bot]", &d[..7])
    );

    let out = push(&repos, b);
    assert!(
        out.contains("skipped"),
        "{d} is synced, {b} is older: {out}"
    );
}

#[test]
fn repositories_without_perseid_toml_are_left_alone() {
    let (repos, shas) = repos();
    seed_with(&repos, false, None);
    let before = sdks_head(&repos);
    let out = push(&repos, &shas[3]);
    assert!(
        out.contains("acme/api-sdks has no perseid.toml yet"),
        "{out}"
    );
    assert_eq!(sdks_head(&repos), before);
}

#[test]
fn first_specs_are_pushed_without_a_seed() {
    let (repos, shas) = repos();
    seed(&repos, None);
    let out = push(&repos, &shas[4]);
    assert!(out.contains("Pushed the spec of acme/api@"), "{out}");
    assert_eq!(sdks_file(&repos, "openapi.json"), "{\"v\": 3}");
}

#[test]
fn specs_reach_a_perseid_toml_in_a_folder() {
    let (repos, shas) = repos();
    seed_with(&repos, false, None);
    let work = repos.sdks.with_extension("folder");
    let url = repos.sdks.to_str().unwrap();
    git(
        repos.sdks.parent().unwrap(),
        &["clone", "--quiet", url, work.to_str().unwrap()],
    );
    fs::create_dir(work.join("sdks")).unwrap();
    commit(&work, &[("sdks/perseid.toml", "name = \"A\"\n")], "init");
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    let out = push_with(&repos, &shas[3], "", false, "sdks/");
    assert!(out.contains("Pushed the spec of acme/api@"), "{out}");
    assert_eq!(sdks_file(&repos, "sdks/openapi.json"), "{\"v\": 2}");
    let source: Value =
        serde_json::from_str(&sdks_file(&repos, "sdks/.perseid/source.json")).unwrap();
    assert_eq!(source["sha"], shas[3].as_str());
}

#[test]
fn private_sources_leave_their_name_out() {
    let (repos, shas) = repos();
    seed(&repos, None);
    let out = push_with(&repos, &shas[3], "v1.0.0", true, "");
    let at = format!("v1.0.0 ({})", &shas[3][..7]);
    assert!(out.contains(&format!("Pushed the spec of {at}")), "{out}");
    let source: Value = serde_json::from_str(&sdks_file(&repos, ".perseid/source.json")).unwrap();
    assert_eq!(
        source,
        serde_json::json!({ "sha": shas[3], "ref": "v1.0.0" })
    );
    let message = git(&repos.sdks, &["log", "-1", "--format=%s", "main"]);
    assert_eq!(message, format!("spec: {at}"));
}

#[test]
fn specs_never_synced_are_pushed() {
    let (repos, shas) = repos();
    seed(&repos, Some(&shas[1]));
    let work = repos.sdks.with_extension("reset");
    git(
        repos.sdks.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            repos.sdks.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    let pending = "{\n  \"repo\": \"acme/api\",\n  \"sha\": null\n}\n";
    commit(
        &work,
        &[(".perseid/source.json", pending)],
        "generated spec, not pushed yet",
    );
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    let out = push(&repos, &shas[4]);
    assert!(out.contains("Pushed the spec of"), "{out}");
}

#[test]
fn generated_specs_are_written_before_being_pushed() {
    let yaml = push_workflow(&Push {
        branch: "main",
        on: PushOn::Change,
        tags: "v*",
        spec: "api/openapi.json",
        build: Some("cargo run --bin openapi > api/openapi.json"),
        hub: "acme/api-sdks",
        destination: "openapi.json",
        config: "perseid.toml",
        private: false,
    });
    fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("perseid-push-generate.yml"),
        &yaml,
    )
    .unwrap();
    let workflow: Value = serde_norway::from_str(&yaml).unwrap();
    assert!(workflow["on"]["push"].get("paths").is_none(), "{yaml}");
    let steps = workflow["jobs"]["push"]["steps"].as_array().unwrap();
    assert_eq!(
        steps[1]["run"],
        "cargo run --bin openapi > api/openapi.json\n"
    );
    assert_eq!(workflow["concurrency"]["cancel-in-progress"], false);
}

#[test]
fn releases_push_their_tag_and_record_it() {
    let (repos, shas) = repos();
    let [_, b, _, d, e] = shas.as_slice() else {
        unreachable!()
    };
    seed(&repos, Some(b));
    let out = push_at(&repos, d, "v1.4.0");
    let at = format!("acme/api@v1.4.0 ({})", &d[..7]);
    assert!(
        out.contains(&format!("Pushed the spec of {at} to acme/api-sdks")),
        "{out}"
    );
    let source: Value = serde_json::from_str(&sdks_file(&repos, ".perseid/source.json")).unwrap();
    assert_eq!(
        (source["sha"].as_str(), source["ref"].as_str()),
        (Some(d.as_str()), Some("v1.4.0"))
    );
    let message = git(&repos.sdks, &["log", "-1", "--format=%s", "main"]);
    assert_eq!(message, format!("spec: {at}"));

    let out = push_at(&repos, e, "v0.9.0-fork");
    assert!(
        out.contains("doesn't descend from: skipped"),
        "tags off the default branch's history are skipped: {out}"
    );
}

/// The workflow of each `--on`, also written for actionlint.
fn workflow(on: PushOn, name: &str) -> Value {
    let yaml = push_workflow(&Push {
        branch: "main",
        on,
        tags: "api-v*",
        spec: "openapi.json",
        build: None,
        hub: "acme/api-sdks",
        destination: "openapi.json",
        config: "perseid.toml",
        private: false,
    });
    fs::write(Path::new(env!("CARGO_TARGET_TMPDIR")).join(name), &yaml).unwrap();
    serde_norway::from_str(&yaml).unwrap()
}

#[test]
fn specs_are_pushed_on_changes_releases_or_tags() {
    let change = workflow(PushOn::Change, "perseid-push-change.yml");
    assert_eq!(change["on"]["push"]["branches"][0], "main");
    assert_eq!(change["on"]["push"]["paths"][0], "openapi.json");
    assert_eq!(change["jobs"]["push"]["if"], "github.ref_name == 'main'");

    let release = workflow(PushOn::Release, "perseid-push-release.yml");
    assert_eq!(release["on"]["release"]["types"][0], "published");
    assert!(release["on"].get("push").is_none());
    let guard = "startsWith(github.ref, 'refs/tags/') || github.ref_name == 'main'";
    assert_eq!(release["jobs"]["push"]["if"], guard);

    let tag = workflow(PushOn::Tag, "perseid-push-tag.yml");
    assert_eq!(tag["on"]["push"]["tags"][0], "api-v*");
    assert!(tag["on"]["push"].get("paths").is_none());
    assert_eq!(tag["jobs"]["push"]["if"], guard);

    for workflow in [&change, &release, &tag] {
        assert!(workflow["on"].get("workflow_dispatch").is_some());
        let step = &workflow["jobs"]["push"]["steps"][1];
        assert!(
            step["env"]["REF"]
                .as_str()
                .unwrap()
                .contains("github.ref_name")
        );
    }
}
