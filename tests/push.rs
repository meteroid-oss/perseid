//! `perseid push-spec`, which perseid-push.yml runs, against local repositories standing for the
//! repository holding the spec and the SDKs repository it pushes the spec to.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use perseid::github::{Auth, Push, PushOn, push_workflow};
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
    push_with(repos, sha, tag, false, "PERSEID_SDKS_TOKEN")
}

/// Runs `perseid push-spec` at `sha`, `private` leaving out the name of the repository it runs
/// in, authenticating with the `credential` variable.
fn push_with(repos: &Repos, sha: &str, tag: &str, private: bool, credential: &str) -> String {
    let (success, out) = run_push(repos, sha, tag, private, credential);
    assert!(success, "{out}");
    out
}

/// Runs `perseid push-spec` at `sha` expecting it to fail, returning its output.
fn push_failing(repos: &Repos, sha: &str, tag: &str) -> String {
    let (success, out) = run_push(repos, sha, tag, false, "PERSEID_SDKS_TOKEN");
    assert!(!success, "{out}");
    out
}

fn run_push(
    repos: &Repos,
    sha: &str,
    tag: &str,
    private: bool,
    credential: &str,
) -> (bool, String) {
    git(&repos.api, &["checkout", "--quiet", sha]);
    let url = format!("file://{}", repos.sdks.display());
    let mut command = Command::new(env!("CARGO_BIN_EXE_perseid"));
    command
        .args(["push-spec", "openapi.json", "--to", "acme/api-sdks"])
        .current_dir(&repos.api)
        .env("GITHUB_SHA", sha)
        .env("GITHUB_REPOSITORY", "acme/api")
        .env("GITHUB_ACTIONS", "true")
        .env(
            credential,
            "unused: the clone URL is rewritten to a local path",
        )
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", format!("url.{url}.insteadOf"))
        .env("GIT_CONFIG_VALUE_0", "git@github.com:acme/api-sdks.git")
        .env("GIT_CONFIG_KEY_1", format!("url.{url}.insteadOf"))
        .env("GIT_CONFIG_VALUE_1", "https://github.com/acme/api-sdks.git")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL");
    match tag {
        "" => command.env_remove("GITHUB_REF"),
        tag => command.env("GITHUB_REF", format!("refs/tags/{tag}")),
    };
    if private {
        command.arg("--private");
    }
    let output = command.output().unwrap();
    let out = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), out)
}

fn sdks_head(repos: &Repos) -> String {
    git(&repos.sdks, &["rev-parse", "main"])
}

fn sdks_file(repos: &Repos, path: &str) -> String {
    git(&repos.sdks, &["show", &format!("main:{path}")])
}

#[test]
fn newer_specs_are_pushed_older_ones_skipped_and_diverged_ones_fail() {
    let (repos, shas) = repos();
    let [a, b, c, d, e] = shas.as_slice() else {
        unreachable!()
    };
    seed(&repos, Some(b));
    let seeded = sdks_head(&repos);

    let out = push(&repos, a);
    assert!(
        out.contains(&format!(
            "::notice::acme/api-sdks has the spec of {}, newer than {}: skipped",
            &b[..7],
            &a[..7]
        )),
        "{out}"
    );
    let out = push_failing(&repos, e, "");
    assert!(
        out.contains(&format!(
            "which {} doesn't descend from (a release cut off the default branch, or rewritten history): to push this spec anyway, remove `sha` from .perseid/source.json in acme/api-sdks",
            &e[..7]
        )),
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
    commit(
        &work,
        &[("sdks/perseid.toml", "name = \"A\"\nsdks = [\"go\"]\n")],
        "init",
    );
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    let out = push(&repos, &shas[3]);
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
    let out = push_with(&repos, &shas[3], "v1.0.0", true, "PERSEID_SDKS_TOKEN");
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
        auth: Auth::Token,
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
    assert_eq!(steps[2]["with"]["spec"], "api/openapi.json");
    assert_eq!(workflow["concurrency"], "perseid-push");
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

    let out = push_failing(&repos, e, "v0.9.0-fork");
    assert!(
        out.contains("doesn't descend from"),
        "tags off the default branch's history fail: {out}"
    );
}

/// The workflow of each `--on`, also written for actionlint.
fn workflow(on: PushOn, auth: Auth, name: &str) -> Value {
    let yaml = push_workflow(&Push {
        branch: "main",
        on,
        tags: "api-v*",
        spec: "openapi.json",
        build: None,
        hub: "acme/api-sdks",
        auth,
        private: false,
    });
    fs::write(Path::new(env!("CARGO_TARGET_TMPDIR")).join(name), &yaml).unwrap();
    serde_norway::from_str(&yaml).unwrap()
}

#[test]
fn specs_are_pushed_on_changes_releases_or_tags() {
    let change = workflow(PushOn::Change, Auth::Token, "perseid-push-change.yml");
    assert_eq!(change["on"]["push"]["branches"][0], "main");
    assert_eq!(change["on"]["push"]["paths"][0], "openapi.json");

    let release = workflow(PushOn::Release, Auth::Token, "perseid-push-release.yml");
    assert_eq!(release["on"]["release"]["types"][0], "published");
    assert!(release["on"].get("push").is_none());

    let tag = workflow(PushOn::Tag, Auth::App, "perseid-push-tag.yml");
    assert_eq!(tag["on"]["push"]["tags"][0], "api-v*");
    assert!(tag["on"]["push"].get("paths").is_none());

    for workflow in [&change, &release, &tag] {
        assert!(workflow["on"].get("workflow_dispatch").is_some());
        let steps = workflow["jobs"]["push"]["steps"].as_array().unwrap();
        let step = steps.last().unwrap();
        assert_eq!(step["uses"], perseid::github::uses("push").as_str());
        assert_eq!(step["with"]["to"], "acme/api-sdks");
    }
}

#[test]
fn runs_without_a_credential_say_which_is_missing() {
    let (repos, shas) = repos();
    seed(&repos, None);
    git(&repos.api, &["checkout", "--quiet", &shas[3]]);
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["push-spec", "openapi.json", "--to", "acme/api-sdks"])
        .current_dir(&repos.api)
        .env("GITHUB_ACTIONS", "true")
        .env_remove("PERSEID_SDKS_TOKEN")
        .env_remove("PERSEID_SDKS_TOKEN")
        .env_remove("GITHUB_REF")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(err.contains("no credential for acme/api-sdks"), "{err}");
}

#[test]
fn specs_recorded_from_another_repository_are_replaced() {
    let (repos, shas) = repos();
    seed(&repos, Some(&shas[1]));
    let work = repos.sdks.with_extension("moved");
    git(
        repos.sdks.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            repos.sdks.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    let moved = "{\n  \"repo\": \"acme/old-api\",\n  \"sha\": \"0123456789abcdef\"\n}\n";
    commit(
        &work,
        &[(".perseid/source.json", moved)],
        "spec from another repository",
    );
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    let out = push(&repos, &shas[4]);
    assert!(out.contains("Pushed the spec of acme/api@"), "{out}");
}

#[test]
fn tokens_push_over_https() {
    let (repos, shas) = repos();
    seed(&repos, None);
    let out = push_with(&repos, &shas[3], "", false, "PERSEID_SDKS_TOKEN");
    assert!(out.contains("Pushed the spec of acme/api@"), "{out}");
}

#[test]
fn runs_off_the_default_branch_are_skipped() {
    let (repos, shas) = repos();
    seed(&repos, None);
    let before = sdks_head(&repos);
    let event = repos.api.with_extension("event.json");
    fs::write(&event, r#"{"repository": {"default_branch": "main"}}"#).unwrap();
    git(&repos.api, &["checkout", "--quiet", &shas[4]]);
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["push-spec", "openapi.json", "--to", "acme/api-sdks"])
        .current_dir(&repos.api)
        .env("GITHUB_REF", "refs/heads/fork")
        .env("GITHUB_EVENT_PATH", &event)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{out}");
    assert!(
        out.contains("`fork` isn't the default branch `main`: skipped"),
        "{out}"
    );
    assert_eq!(sdks_head(&repos), before);
}

#[test]
fn shallow_checkouts_fetch_the_history_they_compare() {
    let (repos, shas) = repos();
    seed(&repos, Some(&shas[1]));
    git(&repos.api, &["checkout", "--quiet", "main"]);
    let shallow = repos.api.with_extension("shallow");
    let url = format!("file://{}", repos.api.display());
    git(
        repos.api.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            "--depth",
            "1",
            &url,
            shallow.to_str().unwrap(),
        ],
    );
    let api = Repos {
        _dir: tempfile::tempdir().unwrap(),
        api: shallow,
        sdks: repos.sdks.clone(),
    };
    let out = push(&api, &shas[3]);
    assert!(out.contains("Pushed the spec of acme/api@"), "{out}");
}
