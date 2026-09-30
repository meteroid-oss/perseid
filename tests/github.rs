//! `perseid setup` and `perseid status` against a fake GitHub serving the REST API, OAuth and App
//! pages, for each repository layout perseid.toml can declare.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use crypto_box::{SecretKey, aead::OsRng};
use serde_json::{Value, json};

const PEM: &str = include_str!("fixtures/github-app.pem");
const PUBLIC_PEM: &str = include_str!("fixtures/github-app.pub.pem");
const SPEC: &str = include_str!("fixtures/petstore.yaml");

#[derive(Default)]
struct Repo {
    private: bool,
    readonly: bool,
    refs: BTreeMap<String, String>,
    variables: BTreeMap<String, String>,
    /// Secrets as decrypted with the repository key.
    secrets: BTreeMap<String, String>,
    key: Option<SecretKey>,
    deploy_keys: Vec<Value>,
    pull_requests_allowed: bool,
}

#[derive(Default)]
struct GitHub {
    calls: Vec<String>,
    repos: BTreeMap<String, Repo>,
    blobs: BTreeMap<String, Vec<u8>>,
    trees: BTreeMap<String, BTreeMap<String, String>>,
    commits: BTreeMap<String, (String, String)>,
    pulls: Vec<Value>,
    token_polls: usize,
    installation_checks: usize,
    app_created: bool,
}

fn digest(value: impl Hash) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl GitHub {
    /// acme/petstore, committing its spec.
    fn with_spec_repo() -> Self {
        let mut github = Self::default();
        github.add_repo(
            "acme/petstore",
            &[("README.md", "# Petstore\n"), ("openapi.yaml", SPEC)],
        );
        github
    }

    fn add_repo(&mut self, name: &str, files: &[(&str, &str)]) {
        let tree = files
            .iter()
            .map(|(path, text)| (path.to_string(), self.blob(text.as_bytes())))
            .collect();
        let commit = self.commit(tree, "init");
        let repo = Repo {
            private: true,
            refs: BTreeMap::from([("main".to_owned(), commit)]),
            ..Repo::default()
        };
        self.repos.insert(name.into(), repo);
    }

    fn blob(&mut self, content: &[u8]) -> String {
        let sha = digest(content);
        self.blobs.insert(sha.clone(), content.to_vec());
        sha
    }

    fn tree(&mut self, tree: BTreeMap<String, String>) -> String {
        let sha = digest(&tree);
        self.trees.insert(sha.clone(), tree);
        sha
    }

    fn commit(&mut self, tree: BTreeMap<String, String>, message: &str) -> String {
        let tree = self.tree(tree);
        let sha = digest((&tree, message, self.commits.len()));
        self.commits.insert(sha.clone(), (tree, message.into()));
        sha
    }

    /// Files of `repo` at the tip of `branch`.
    fn files(&self, repo: &str, branch: &str) -> BTreeMap<String, String> {
        let commit = &self.repos[repo].refs[branch];
        let tree = &self.trees[&self.commits[commit].0];
        tree.iter()
            .map(|(path, blob)| {
                (
                    path.clone(),
                    String::from_utf8_lossy(&self.blobs[blob]).into(),
                )
            })
            .collect()
    }

    /// Requests that changed something since the `since`th.
    fn writes(&self, since: usize) -> Vec<&String> {
        self.calls[since..]
            .iter()
            .filter(|c| !c.starts_with("GET"))
            .collect()
    }

    fn handle(
        &mut self,
        method: &str,
        target: &str,
        auth: Option<&str>,
        body: &Value,
        raw: bool,
    ) -> (u16, Value) {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        self.calls.push(format!("{method} {path}"));
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (method, segments.as_slice()) {
            ("POST", ["web", "login", "device", "code"]) => (
                200,
                json!({
                    "device_code": "device-1", "user_code": "ABCD-1234",
                    "verification_uri": "http://github.test/web/login/device",
                    "expires_in": 900, "interval": 1,
                }),
            ),
            ("POST", ["web", "login", "oauth", "access_token"]) => {
                self.token_polls += 1;
                match self.token_polls {
                    1 => (200, json!({ "error": "authorization_pending" })),
                    2 => (200, json!({ "error": "slow_down", "interval": 1 })),
                    _ => (
                        200,
                        json!({ "access_token": "user-token", "token_type": "bearer", "scope": "repo,workflow" }),
                    ),
                }
            }
            ("GET", ["spec.yaml"]) => (200, Value::String(SPEC.into())),
            (_, ["api", ..]) if auth.is_none() && segments[1] != "app-manifests" => {
                (401, json!({ "message": "Requires authentication" }))
            }
            ("GET", ["api", "user"]) => (200, json!({ "login": "octo" })),
            ("GET", ["api", "users", login]) => {
                (200, json!({ "login": login, "type": "Organization" }))
            }
            ("GET", ["api", "orgs", _, "installations"]) => {
                let installations = match self.app_created {
                    true => json!([{ "app_id": 42, "repository_selection": "selected" }]),
                    false => json!([]),
                };
                (200, json!({ "installations": installations }))
            }
            ("POST", ["api", "orgs", org, "repos"]) => {
                let name = format!("{org}/{}", body["name"].as_str().unwrap());
                assert!(!self.repos.contains_key(&name), "{name} created twice");
                assert_eq!(body["visibility"], "private");
                let repo = Repo {
                    private: body["private"] == true,
                    ..Repo::default()
                };
                self.repos.insert(name.clone(), repo);
                (201, json!({ "full_name": name, "default_branch": "main" }))
            }
            ("POST", ["api", "app-manifests", code, "conversions"]) => {
                assert_eq!(*code, "manifest-code");
                self.app_created = true;
                (
                    201,
                    json!({ "id": 42, "slug": "petstore-sdk-bot", "pem": PEM, "client_id": "Iv1" }),
                )
            }
            (_, ["api", "repos", owner, name, rest @ ..]) => {
                let repo = format!("{owner}/{name}");
                if !self.repos.contains_key(&repo) {
                    return (404, json!({ "message": "Not Found" }));
                }
                self.repo(method, &repo, rest, query, auth.unwrap(), body, raw)
            }
            _ => (
                404,
                json!({ "message": format!("fake GitHub has no {method} {path}") }),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repo(
        &mut self,
        method: &str,
        name: &str,
        rest: &[&str],
        query: &str,
        auth: &str,
        body: &Value,
        raw: bool,
    ) -> (u16, Value) {
        let param = |key: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
        };
        match (method, rest) {
            ("GET", []) => {
                let repo = &self.repos[name];
                let visibility = if repo.private { "private" } else { "public" };
                (
                    200,
                    json!({
                        "full_name": name, "private": repo.private, "visibility": visibility,
                        "default_branch": "main",
                        "permissions": { "admin": !repo.readonly, "push": true },
                    }),
                )
            }
            ("GET", ["installation"]) => {
                let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
                validation.set_required_spec_claims(&["exp"]);
                let key = jsonwebtoken::DecodingKey::from_rsa_pem(PUBLIC_PEM.as_bytes()).unwrap();
                let jwt =
                    jsonwebtoken::decode::<Value>(auth, &key, &validation).expect("an App JWT");
                assert_eq!(jwt.claims["iss"], 42);
                self.installation_checks += 1;
                match self.installation_checks {
                    1 => (404, json!({ "message": "Not Found" })),
                    _ => (200, json!({ "id": 7, "app_id": 42 })),
                }
            }
            ("GET", ["contents", path @ ..]) => {
                let branch = param("ref").unwrap();
                match self.repos[name].refs.get(&branch) {
                    None => (404, json!({ "message": "No commit found for the ref" })),
                    Some(_) => match self.files(name, &branch).get(&path.join("/")) {
                        Some(text) if raw => (200, Value::String(text.clone())),
                        Some(text) => (200, json!({ "content": BASE64.encode(text) })),
                        None => (404, json!({ "message": "Not Found" })),
                    },
                }
            }
            ("PUT", ["contents", path @ ..]) => {
                assert!(
                    self.repos[name].refs.is_empty(),
                    "contents API used on a non-empty repo"
                );
                let content = BASE64.decode(body["content"].as_str().unwrap()).unwrap();
                let tree = BTreeMap::from([(path.join("/"), self.blob(&content))]);
                let commit = self.commit(tree, body["message"].as_str().unwrap());
                let branch = body["branch"].as_str().unwrap().to_owned();
                self.repos
                    .get_mut(name)
                    .unwrap()
                    .refs
                    .insert(branch, commit.clone());
                (201, json!({ "commit": { "sha": commit } }))
            }
            ("GET", ["git", "ref", "heads", branch @ ..]) => {
                let refs = &self.repos[name].refs;
                match refs.get(&branch.join("/")) {
                    _ if refs.is_empty() => (409, json!({ "message": "Git Repository is empty." })),
                    Some(sha) => (200, json!({ "object": { "sha": sha } })),
                    None => (404, json!({ "message": "Not Found" })),
                }
            }
            ("GET", ["git", "trees", branch]) => match self.repos[name].refs.get(*branch) {
                Some(commit) => {
                    let tree = &self.trees[&self.commits[commit].0];
                    let entries: Vec<Value> = tree
                        .iter()
                        .map(|(path, sha)| json!({ "path": path, "type": "blob", "sha": sha }))
                        .collect();
                    (200, json!({ "tree": entries }))
                }
                None => (404, json!({ "message": "Not Found" })),
            },
            ("GET", ["git", "commits", sha]) => (
                200,
                json!({ "sha": sha, "tree": { "sha": self.commits[*sha].0 } }),
            ),
            ("POST", ["git", "blobs"]) => {
                let content = BASE64.decode(body["content"].as_str().unwrap()).unwrap();
                (201, json!({ "sha": self.blob(&content) }))
            }
            ("POST", ["git", "trees"]) => {
                let mut tree = self.trees[body["base_tree"].as_str().unwrap()].clone();
                for entry in body["tree"].as_array().unwrap() {
                    tree.insert(
                        entry["path"].as_str().unwrap().into(),
                        entry["sha"].as_str().unwrap().into(),
                    );
                }
                (201, json!({ "sha": self.tree(tree) }))
            }
            ("POST", ["git", "commits"]) => {
                let tree = self.trees[body["tree"].as_str().unwrap()].clone();
                (
                    201,
                    json!({ "sha": self.commit(tree, body["message"].as_str().unwrap()) }),
                )
            }
            ("POST", ["git", "refs"]) => {
                let branch = body["ref"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("refs/heads/")
                    .unwrap();
                let refs = &mut self.repos.get_mut(name).unwrap().refs;
                assert!(!refs.contains_key(branch));
                refs.insert(branch.into(), body["sha"].as_str().unwrap().into());
                (201, json!({}))
            }
            ("PATCH", ["git", "refs", "heads", branch @ ..]) => {
                let refs = &mut self.repos.get_mut(name).unwrap().refs;
                refs.insert(branch.join("/"), body["sha"].as_str().unwrap().into());
                (200, json!({}))
            }
            ("GET", ["commits"]) => match self.repos[name].refs.get("main") {
                Some(sha) => (
                    200,
                    json!([{ "sha": sha, "commit": { "committer": { "date": "2026-09-30T10:00:00Z" } } }]),
                ),
                None => (409, json!({ "message": "Git Repository is empty." })),
            },
            ("GET", ["keys"]) => (200, Value::from(self.repos[name].deploy_keys.clone())),
            ("POST", ["keys"]) => {
                let keys = &mut self.repos.get_mut(name).unwrap().deploy_keys;
                let mut key = body.clone();
                key["id"] = (keys.len() + 1).into();
                keys.push(key.clone());
                (201, key)
            }
            ("DELETE", ["keys", id]) => {
                let keys = &mut self.repos.get_mut(name).unwrap().deploy_keys;
                keys.retain(|k| k["id"].as_u64() != id.parse().ok());
                (204, Value::Null)
            }
            ("GET", ["actions", "permissions", "workflow"]) => (
                200,
                json!({
                    "default_workflow_permissions": "read",
                    "can_approve_pull_request_reviews": self.repos[name].pull_requests_allowed,
                }),
            ),
            ("PUT", ["actions", "permissions", "workflow"]) => {
                assert_eq!(body["default_workflow_permissions"], "read");
                let allowed = body["can_approve_pull_request_reviews"] == true;
                self.repos.get_mut(name).unwrap().pull_requests_allowed = allowed;
                (204, Value::Null)
            }
            ("GET", ["actions", "workflows", _, "runs"]) => (200, json!({ "workflow_runs": [] })),
            ("GET", ["actions", "variables", key]) => match self.repos[name].variables.get(*key) {
                Some(value) => (200, json!({ "name": key, "value": value })),
                None => (404, json!({ "message": "Not Found" })),
            },
            ("POST", ["actions", "variables"]) => {
                let variables = &mut self.repos.get_mut(name).unwrap().variables;
                let key = body["name"].as_str().unwrap();
                assert!(!variables.contains_key(key));
                variables.insert(key.into(), body["value"].as_str().unwrap().into());
                (201, json!({}))
            }
            ("PATCH", ["actions", "variables", key]) => {
                let variables = &mut self.repos.get_mut(name).unwrap().variables;
                variables.insert((*key).into(), body["value"].as_str().unwrap().into());
                (204, Value::Null)
            }
            ("GET", ["actions", "secrets", "public-key"]) => {
                let repo = self.repos.get_mut(name).unwrap();
                let key = repo
                    .key
                    .get_or_insert_with(|| SecretKey::generate(&mut OsRng));
                (
                    200,
                    json!({ "key_id": "key-1", "key": BASE64.encode(key.public_key().as_bytes()) }),
                )
            }
            ("GET", ["actions", "secrets", key]) => {
                match self.repos[name].secrets.contains_key(*key) {
                    true => (200, json!({ "name": key })),
                    false => (404, json!({ "message": "Not Found" })),
                }
            }
            ("PUT", ["actions", "secrets", key]) => {
                assert_eq!(body["key_id"], "key-1");
                let repo = self.repos.get_mut(name).unwrap();
                let sealed = BASE64
                    .decode(body["encrypted_value"].as_str().unwrap())
                    .unwrap();
                let value = repo
                    .key
                    .as_ref()
                    .unwrap()
                    .unseal(&sealed)
                    .expect("a sealed box");
                repo.secrets
                    .insert((*key).into(), String::from_utf8(value).unwrap());
                (201, json!({}))
            }
            ("GET", ["pulls"]) => {
                let head = param("head").unwrap();
                let open: Vec<_> = self
                    .pulls
                    .iter()
                    .filter(|p| {
                        p["repo"] == name && format!("acme:{}", p["head"].as_str().unwrap()) == head
                    })
                    .cloned()
                    .collect();
                (200, Value::Array(open))
            }
            ("POST", ["pulls"]) => {
                let url = format!("https://github.com/{name}/pull/{}", self.pulls.len() + 1);
                let mut pull = body.clone();
                pull["repo"] = name.into();
                pull["html_url"] = url.clone().into();
                self.pulls.push(pull);
                (201, json!({ "html_url": url }))
            }
            _ => (
                404,
                json!({ "message": format!("fake GitHub has no {method} {name}/{}", rest.join("/")) }),
            ),
        }
    }
}

fn serve(github: Arc<Mutex<GitHub>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let github = github.clone();
            std::thread::spawn(move || respond(stream, &github));
        }
    });
    port
}

fn respond(stream: TcpStream, github: &Mutex<GitHub>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_owned(),
        parts.next().unwrap_or("").to_owned(),
    );
    let (mut length, mut auth, mut form, mut raw) = (0, None, false, false);
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        match name.to_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap(),
            "authorization" => auth = value.trim().strip_prefix("Bearer ").map(str::to_owned),
            "content-type" => form = value.contains("x-www-form-urlencoded"),
            "accept" => raw = value.contains(".raw"),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let body = match form {
        true => Value::Object(
            String::from_utf8(body)
                .unwrap()
                .split('&')
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_owned(), Value::from(v)))
                .collect(),
        ),
        false => serde_json::from_slice(&body).unwrap_or(Value::Null),
    };
    let (status, reply) =
        github
            .lock()
            .unwrap()
            .handle(&method, &target, auth.as_deref(), &body, raw);
    let text = match reply {
        Value::Null => String::new(),
        Value::String(text) => text,
        reply => reply.to_string(),
    };
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nX-OAuth-Scopes: repo, workflow\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A checkout whose origin is `repo`, and a PATH offering git but not gh.
fn checkout(repo: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--quiet", "--initial-branch", "main"]);
    let url = format!("https://github.com/{repo}.git");
    git(dir.path(), &["remote", "add", "origin", &url]);
    let bin = tempfile::tempdir().unwrap();
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(real.trim(), bin.path().join("git")).unwrap();
    (dir, bin)
}

/// A clone of acme/petstore holding its spec, committed or not.
fn api_checkout(committed: bool) -> (tempfile::TempDir, tempfile::TempDir) {
    let (dir, bin) = checkout("acme/petstore");
    fs::write(dir.path().join("openapi.yaml"), SPEC).unwrap();
    if committed {
        git(dir.path(), &["add", "openapi.yaml"]);
        git(dir.path(), &["commit", "--quiet", "-m", "spec"]);
    }
    (dir, bin)
}

fn edit(dir: &Path, edit: impl FnOnce(String) -> String) {
    let path = dir.join("perseid.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(path, edit(text)).unwrap();
}

/// Uncomments the example setting of perseid.toml that starts with `setting`.
fn uncomment(dir: &Path, setting: &str) {
    edit(dir, |text| {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("# {setting}")))
            .unwrap_or_else(|| panic!("no `{setting}` example in {text}"));
        let value = line[2..].split("  #").next().unwrap().trim_end();
        text.replace(line, value)
    });
}

/// Runs perseid with `answers` on stdin, acting as the browser when it asks to create the App.
fn perseid(
    dir: &Path,
    bin: &Path,
    port: u16,
    args: &[&str],
    answers: &str,
    env: &[(&str, &str)],
) -> (i32, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_perseid"));
    command
        .args(args)
        .current_dir(dir)
        .env("PATH", bin)
        .env("PERSEID_GITHUB_API", format!("http://127.0.0.1:{port}/api"))
        .env("PERSEID_GITHUB_WEB", format!("http://127.0.0.1:{port}/web"))
        .env("PERSEID_GITHUB_CLIENT_ID", "test-client")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(var);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answers.as_bytes())
        .unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut out = String::new();
    for line in stdout.lines() {
        let line = line.unwrap();
        if line.starts_with("→ Create the GitHub App") {
            let local = line.rsplit(' ').next().unwrap().to_owned();
            std::thread::spawn(move || browser(&local, port));
        }
        out += &line;
        out.push('\n');
    }
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let code = child.wait().unwrap().code().unwrap_or(-1);
    eprintln!("$ perseid {}\n{out}{stderr}", args.join(" "));
    (code, out + &stderr)
}

const TOKEN: [(&str, &str); 1] = [("GH_TOKEN", "user-token")];

/// What GitHub does with the manifest page: check the posted manifest, then redirect back.
fn browser(local: &str, port: u16) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let page = agent
        .get(local)
        .call()
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    let attribute = |name: &str| {
        let start = page.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
        let value = &page[start..start + page[start..].find('"').unwrap()];
        value
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    };
    let action = attribute("action");
    let prefix = format!("http://127.0.0.1:{port}/web/organizations/acme/settings/apps/new?state=");
    let state = action
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{action}"));
    let manifest: Value = serde_json::from_str(&attribute("value")).unwrap();
    assert_eq!(manifest["name"], "petstore-sdk-bot");
    assert!(
        manifest["url"]
            .as_str()
            .unwrap()
            .starts_with("https://github.com/acme/petstore")
    );
    assert_eq!(manifest["public"], false);
    assert_eq!(manifest["hook_attributes"]["active"], false);
    assert_eq!(
        manifest["default_permissions"],
        json!({ "contents": "write", "pull_requests": "write", "metadata": "read" })
    );
    let redirect = manifest["redirect_url"].as_str().unwrap();
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"));
    let forged = agent
        .get(format!("{redirect}?code=stolen&state=forged"))
        .call()
        .unwrap();
    assert_eq!(forged.status().as_u16(), 400);
    let done = agent
        .get(format!("{redirect}?code=manifest-code&state={state}"))
        .call()
        .unwrap();
    assert_eq!(done.status().as_u16(), 200);
}

/// Keeps a generated workflow for actionlint, which CI runs on `target/tmp/*.yml`.
fn keep(name: &str, yaml: &str) {
    fs::write(Path::new(env!("CARGO_TARGET_TMPDIR")).join(name), yaml).unwrap();
}

#[test]
fn setup_orchestrates_one_repository_per_language() {
    let mut fake = GitHub::with_spec_repo();
    let existing = r#"{ "packages": { "legacy": { "release-type": "simple" } } }"#;
    fake.add_repo(
        "acme/petstore-node",
        &[
            ("README.md", "# Node\n"),
            ("release-please-config.json", existing),
        ],
    );
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(false);
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["init", "typescript", "python", "go"],
        "",
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Next: `perseid setup`"), "{out}");
    uncomment(dir.path(), "repo = \"acme/petstore-{lang}\"");
    fs::remove_dir_all(dir.path().join("typescript")).unwrap();

    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--no-browser"],
        "\n\n",
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("and enter the code ABCD-1234"), "{out}");
    assert!(
        out.contains("✓ Signed in to GitHub as octo (browser)"),
        "{out}"
    );
    assert!(
        out.contains(
            "acme/petstore ──PRs──▶ acme/petstore-node, acme/petstore-python, acme/petstore-go"
        ),
        "{out}"
    );
    for line in [
        "  + create acme/petstore-python (private)",
        "  + create acme/petstore-go (private)",
        "  = acme/petstore-node: existing, files added only where missing",
        "  + acme/petstore-node: commit release-please-config.json, .release-please-manifest.json, .github/workflows/sdk-release.yml (updating release-please-config.json)",
        "  + GitHub App petstore-sdk-bot on acme, created in your browser",
        "  + acme/petstore: pull request with .github/workflows/sdks.yml, perseid.toml",
        "? Apply these 7 changes? [Y/n]",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let github = server.lock().unwrap();
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    for repo in [
        "acme/petstore-go",
        "acme/petstore-node",
        "acme/petstore-python",
    ] {
        let files = github.files(repo, "main");
        assert!(
            files.contains_key(".github/workflows/sdk-release.yml"),
            "{repo}: {files:?}"
        );
        let config: Value = serde_json::from_str(&files["release-please-config.json"]).unwrap();
        assert_eq!(config["packages"]["."]["include-component-in-tag"], false);
        let repo = &github.repos[repo];
        assert_eq!(repo.variables["SDK_APP_ID"], "42");
        assert_eq!(
            repo.secrets["SDK_APP_PRIVATE_KEY"], PEM,
            "decrypted with the repository key"
        );
    }
    let node = &github.files("acme/petstore-node", "main")["release-please-config.json"];
    let node: Value = serde_json::from_str(node).unwrap();
    assert_eq!(
        node["packages"]["legacy"]["release-type"], "simple",
        "merged, not replaced"
    );
    assert_eq!(node["packages"]["."]["release-type"], "node");
    assert_eq!(
        github.repos["acme/petstore"].variables["SDK_APP_SLUG"],
        "petstore-sdk-bot"
    );
    assert!(github.installation_checks > 3, "polled until installed");

    let files = github.files("acme/petstore", "perseid/setup");
    let workflow = &files[".github/workflows/sdks.yml"];
    keep("per-language-sdks.yml", workflow);
    assert!(
        workflow.contains("paths: [\"openapi.yaml\",\"perseid.toml\"]"),
        "{workflow}"
    );
    assert!(
        workflow.contains("repositories: petstore,petstore-go,petstore-node,petstore-python\n"),
        "{workflow}"
    );
    assert_eq!(files["perseid.toml"], config);
    assert_eq!(github.pulls.len(), 1);
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, ".github/workflows/sdks.yml\nperseid.toml\n");
    assert!(
        out.contains("PyPI: add a pending publisher for petstore"),
        "{out}"
    );
    drop(github);

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert!(
        out.contains("  = acme/petstore: https://github.com/acme/petstore/pull/1 holds"),
        "{out}"
    );
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    for line in [
        "✓ In sync with perseid.toml",
        "✓ acme/petstore-go: no SDK pull request waiting",
        "✓ The App is installed on acme (selected repositories)",
        "! acme/petstore: sdks.yml hasn't run yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
}

#[test]
fn setup_without_credentials_says_how_to_get_them() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server);
    let (dir, bin) = api_checkout(false);
    let args = ["init", "rust", "--github", "--yes"];
    let env = [("PERSEID_GITHUB_CLIENT_ID", "")];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &env);
    assert_ne!(code, 0);
    assert!(
        out.contains("is now `perseid init`, then `perseid setup`"),
        "{out}"
    );
    assert!(
        out.contains("set GH_TOKEN") && out.contains("gh auth login"),
        "{out}"
    );
    assert!(dir.path().join("rust/Cargo.toml").exists());

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &[]);
    assert_eq!(code, 1, "sdks.yml is missing: {out}");
    for line in [
        "acme/petstore ──PRs──▶ acme/petstore (rust/)",
        "✓ perseid.toml is valid",
        "✗ .github/workflows/sdks.yml is missing",
        "! Not signed in to GitHub: only local checks ran",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
}

/// The deploy key registered on `repo` for acme/petstore, checked against the secret holding
/// its private half.
fn deploy_key(github: &GitHub, repo: &str) -> Value {
    let keys = &github.repos[repo].deploy_keys;
    assert_eq!(keys.len(), 1, "{keys:?}");
    let key = &keys[0];
    assert_eq!(key["title"], "perseid: spec pushes from acme/petstore");
    assert_eq!(key["read_only"], false);
    let private = &github.repos["acme/petstore"].secrets["PERSEID_SDKS_DEPLOY_KEY"];
    let private = ssh_key::PrivateKey::from_openssh(private).expect("an OpenSSH private key");
    assert_eq!(private.algorithm(), ssh_key::Algorithm::Ed25519);
    let public = private.public_key().to_openssh().unwrap();
    let registered = key["key"].as_str().unwrap();
    assert_eq!(
        public.split(' ').take(2).collect::<Vec<_>>(),
        registered.split(' ').take(2).collect::<Vec<_>>()
    );
    key.clone()
}

#[test]
fn setup_sends_the_spec_to_an_sdks_repository_holding_every_sdk() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(true);
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["init", "typescript", "python"],
        "",
        &[],
    );
    assert_eq!(code, 0, "{out}");
    uncomment(dir.path(), "push_spec");

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--dry-run"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 2, "changes pending: {out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    let diagram = "acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (typescript/, python/)";
    for line in [
        diagram,
        "  + create acme/petstore-sdks (private)",
        "  ~ acme/petstore-sdks: let GitHub Actions open pull requests",
        "  + acme/petstore-sdks: commit .github/workflows/sdks.yml, perseid.toml, openapi.yaml, .perseid/source.json and ",
        "  + acme/petstore-sdks: write deploy key for acme/petstore, its private half the PERSEID_SDKS_DEPLOY_KEY secret of acme/petstore",
        "  + acme/petstore: variable PERSEID_SDKS_REPO = acme/petstore-sdks",
        "  + acme/petstore: pull request with .github/workflows/perseid-push.yml, perseid.toml",
        "! typescript/, python/ aren't generated here anymore: delete them",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(
        !out.contains("GitHub App"),
        "no App when every SDK lives in the SDKs repository: {out}"
    );

    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    let github = server.lock().unwrap();
    let sdks = &github.repos["acme/petstore-sdks"];
    assert!(sdks.private && sdks.pull_requests_allowed);
    let files = github.files("acme/petstore-sdks", "main");
    let config = &files["perseid.toml"];
    assert!(
        config.starts_with("spec = \"github:acme/petstore/openapi.yaml\"\n"),
        "{config}"
    );
    assert!(
        config.contains("repository = \"https://github.com/acme/petstore-sdks\""),
        "{config}"
    );
    assert!(!config.contains("\npush_spec"), "{config}");
    assert!(
        config.contains("[typescript]") && config.contains("[python]"),
        "{config}"
    );
    assert_eq!(files["openapi.yaml"], SPEC);
    let source: Value = serde_json::from_str(&files[".perseid/source.json"]).unwrap();
    let head = github.repos["acme/petstore"].refs["main"].clone();
    assert_eq!(
        source,
        json!({ "repo": "acme/petstore", "path": "openapi.yaml", "sha": head })
    );
    assert!(files.contains_key("typescript/package.json"), "{files:?}");
    let release: Value = serde_json::from_str(&files["release-please-config.json"]).unwrap();
    assert_eq!(release["packages"]["python"]["release-type"], "python");
    let workflow = &files[".github/workflows/sdks.yml"];
    keep("sdks-repo-sdks.yml", workflow);
    assert!(
        workflow.contains(
            "paths: [\"openapi.yaml\",\".perseid/source.json\",\".github/workflows/sdks.yml\"]"
        ),
        "{workflow}"
    );
    assert!(
        workflow.contains("pull-requests: write") && !workflow.contains("SDK_APP_ID"),
        "{workflow}"
    );
    assert!(
        workflow.contains("- if: hashFiles('openapi.yaml') != ''"),
        "{workflow}"
    );

    deploy_key(&github, "acme/petstore-sdks");
    let api = &github.repos["acme/petstore"];
    assert_eq!(api.variables["PERSEID_SDKS_REPO"], "acme/petstore-sdks");
    assert!(
        !api.variables.contains_key("SDK_APP_ID"),
        "the API repository gets no App"
    );
    assert_eq!(
        api.secrets.keys().collect::<Vec<_>>(),
        ["PERSEID_SDKS_DEPLOY_KEY"]
    );
    let setup = github.files("acme/petstore", "perseid/setup");
    let push = &setup[".github/workflows/perseid-push.yml"];
    keep("perseid-push.yml", push);
    assert!(
        push.contains("paths: [\"openapi.yaml\",\".github/workflows/perseid-push.yml\"]"),
        "{push}"
    );
    assert_eq!(
        github.pulls.len(),
        1,
        "the SDKs repository is new: committed directly"
    );
    assert!(
        out.contains("Merge https://github.com/acme/petstore/pull/1, then `git pull`"),
        "{out}"
    );
    assert!(out.contains(&format!("✓ {diagram}")), "{out}");
    drop(github);
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, ".github/workflows/perseid-push.yml\nperseid.toml\n");

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    for line in [
        diagram,
        "✓ .github/workflows/perseid-push.yml is here",
        "✓ In sync with perseid.toml",
        &format!("✓ Last synced acme/petstore@{}, ", &head[..7]),
        "✓ acme/petstore-sdks: no SDK pull request waiting",
        "! acme/petstore: perseid-push.yml hasn't run yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ …"),
        "{out}"
    );
    assert!(
        out.contains("! Not signed in to GitHub: only local checks ran"),
        "{out}"
    );

    let old = {
        let mut github = server.lock().unwrap();
        let api = github.repos.get_mut("acme/petstore").unwrap();
        api.secrets.remove("PERSEID_SDKS_DEPLOY_KEY");
        github.repos["acme/petstore-sdks"].deploy_keys[0]["key"].clone()
    };
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("✗ 1 change pending:\n    ~ acme/petstore-sdks: write deploy key"),
        "{out}"
    );
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    let key = deploy_key(&server.lock().unwrap(), "acme/petstore-sdks");
    assert_ne!(key["key"], old, "the key without its secret is replaced");
}

#[test]
fn setup_reuses_an_existing_sdks_repository_from_the_api_repository() {
    let mut fake = GitHub::with_spec_repo();
    let config = "spec = \"https://api.example.com/openapi.yaml\"\nname = \"Petstore\"\n\n[go]\nmodule = \"example.com/petstore\"\n";
    fake.add_repo("acme/petstore-sdks", &[("perseid.toml", config)]);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(true);
    let args = ["init", "typescript", "--no-release"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &[]);
    assert_eq!(code, 0, "{out}");
    uncomment(dir.path(), "push_spec");
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (go/)",
        "(updating perseid.toml)",
        "Merge https://github.com/acme/petstore-sdks/pull/1 first, then https://github.com/acme/petstore/pull/2",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let github = server.lock().unwrap();
    let setup = github.files("acme/petstore-sdks", "perseid/setup");
    assert_eq!(
        setup["perseid.toml"],
        config.replace(
            "https://api.example.com/openapi.yaml",
            "github:acme/petstore/openapi.yaml"
        ),
        "only the spec changes"
    );
    assert!(setup.contains_key("go/go.mod") && !setup.contains_key("typescript/package.json"));
}

#[test]
fn setup_sends_the_spec_to_an_sdks_repository_orchestrating_one_per_language() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(true);
    let args = ["init", "typescript", "python", "--no-release"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &[]);
    assert_eq!(code, 0, "{out}");
    uncomment(dir.path(), "push_spec");
    uncomment(dir.path(), "repo = \"acme/petstore-{lang}\"");
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--no-browser"],
        "y\n\n",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-node, acme/petstore-python"),
        "{out}"
    );
    let github = server.lock().unwrap();
    for repo in [
        "acme/petstore-sdks",
        "acme/petstore-node",
        "acme/petstore-python",
    ] {
        assert_eq!(
            github.repos[repo].secrets["SDK_APP_PRIVATE_KEY"], PEM,
            "{repo}"
        );
    }
    let api = &github.repos["acme/petstore"];
    assert!(
        !api.secrets.contains_key("SDK_APP_PRIVATE_KEY"),
        "the App never reaches the API repository"
    );
    let files = github.files("acme/petstore-sdks", "main");
    assert!(files["perseid.toml"].contains("\nrepo = \"acme/petstore-{lang}\"\n"));
    assert!(!files.contains_key("typescript/package.json"), "{files:?}");
    let workflow = &files[".github/workflows/sdks.yml"];
    keep("sdks-repo-split-sdks.yml", workflow);
    assert!(
        workflow.contains("repositories: petstore-sdks,petstore-node,petstore-python\n"),
        "{workflow}"
    );
    assert!(
        github
            .files("acme/petstore-node", "main")
            .contains_key(".github/workflows/sdk-release.yml")
    );
    deploy_key(&github, "acme/petstore-sdks");
}

/// A clone of acme/petstore-sdks, an SDK monorepo generated from a URL until now.
fn sdks_checkout(fake: &mut GitHub) -> (tempfile::TempDir, tempfile::TempDir) {
    let config = "spec = \"https://api.example.com/openapi.yaml\"\nname = \"Petstore\"\nmethod_names = \"resource\"\n\n[typescript]\npath = \"ts\"\ntyped_unions = true\n";
    let release = "{\n  \"packages\": {\n    \"docs\": {\n      \"release-type\": \"simple\"\n    }\n  }\n}\n";
    let handwritten = "export const mine = 1;\n";
    let files = [
        ("perseid.toml", config),
        ("release-please-config.json", release),
        ("ts/src/index.ts", handwritten),
    ];
    fake.add_repo("acme/petstore-sdks", &files);
    let (dir, bin) = checkout("acme/petstore-sdks");
    for (path, text) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "--quiet", "-m", "sdks"]);
    (dir, bin)
}

#[test]
fn setup_from_an_existing_sdks_repository_links_the_api_repository() {
    let mut fake = GitHub::with_spec_repo();
    let (dir, bin) = sdks_checkout(&mut fake);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    edit(dir.path(), |t| {
        t.replace(
            "https://api.example.com/openapi.yaml",
            "github:acme/petstore/openapi.yaml",
        )
    });

    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (ts/)",
        "  ~ acme/petstore-sdks: let GitHub Actions open pull requests",
        "  + acme/petstore-sdks: pull request with .github/workflows/sdks.yml, perseid.toml, openapi.yaml, .perseid/source.json and ",
        "(updating perseid.toml, release-please-config.json)",
        "! acme/petstore-sdks: perseid would overwrite files it didn't generate, and stops instead: ts/src/index.ts",
        "  + acme/petstore: pull request with .github/workflows/perseid-push.yml",
        "Merge https://github.com/acme/petstore-sdks/pull/1 first, then https://github.com/acme/petstore/pull/2",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let github = server.lock().unwrap();
    let setup = github.files("acme/petstore-sdks", "perseid/setup");
    let release: Value = serde_json::from_str(&setup["release-please-config.json"]).unwrap();
    assert_eq!(
        release["packages"]["docs"]["release-type"], "simple",
        "kept"
    );
    assert_eq!(release["packages"]["ts"]["release-type"], "node", "added");
    assert_eq!(
        setup["ts/src/index.ts"], "export const mine = 1;\n",
        "never overwritten"
    );
    assert!(
        setup["perseid.toml"].contains("[typescript]\npath = \"ts\"\n"),
        "user settings kept"
    );
    assert_eq!(setup["openapi.yaml"], SPEC);
    assert!(
        setup.contains_key("ts/package.json"),
        "skeleton added: {setup:?}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("openapi.yaml")).unwrap(),
        SPEC
    );
    deploy_key(&github, "acme/petstore-sdks");
    assert_eq!(
        github.repos["acme/petstore"].variables["PERSEID_SDKS_REPO"],
        "acme/petstore-sdks"
    );
    let push =
        &github.files("acme/petstore", "perseid/setup")[".github/workflows/perseid-push.yml"];
    assert!(push.contains("SNAPSHOT: openapi.yaml\n"), "{push}");
    drop(github);

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ Last synced acme/petstore@"), "{out}");
}

#[test]
fn setup_from_a_new_sdks_repository_seeds_its_spec_from_the_api_repository() {
    let mut fake = GitHub::with_spec_repo();
    fake.add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = checkout("acme/petstore-sdks");
    let args = [
        "init",
        "python",
        "--spec",
        "github:acme/petstore/openapi.yaml",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &[]);
    assert_eq!(code, 0, "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(
        config.starts_with("spec = \"github:acme/petstore/openapi.yaml\"\nname = \"Petstore\"\n"),
        "{config}"
    );
    assert!(!config.contains("# push_spec"), "{config}");

    let (code, out) = perseid(dir.path(), bin.path(), port, &["setup"], "\n", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("? Apply these 5 changes? [Y/n]"), "{out}");
    let github = server.lock().unwrap();
    let setup = github.files("acme/petstore-sdks", "perseid/setup");
    for path in [
        "openapi.yaml",
        ".perseid/source.json",
        "perseid.toml",
        "python/pyproject.toml",
        ".github/workflows/sdk-release.yml",
    ] {
        assert!(setup.contains_key(path), "{path}: {setup:?}");
    }
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.contains("openapi.yaml\n") && staged.contains(".perseid/source.json\n"),
        "{staged}"
    );
    deploy_key(&github, "acme/petstore-sdks");
    drop(github);
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "acme/petstore ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (python/)"
        ),
        "{out}"
    );
    assert!(out.contains("✓ Last synced acme/petstore@"), "{out}");
}

#[test]
fn setup_asks_an_admin_of_the_api_repository() {
    let mut fake = GitHub::with_spec_repo();
    fake.repos.get_mut("acme/petstore").unwrap().readonly = true;
    let (dir, bin) = sdks_checkout(&mut fake);
    let port = serve(Arc::new(Mutex::new(fake)));
    edit(dir.path(), |t| {
        t.replace(
            "https://api.example.com/openapi.yaml",
            "github:acme/petstore/openapi.yaml",
        )
    });
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--yes"],
        "",
        &TOKEN,
    );
    assert_ne!(code, 0);
    assert!(
        out.contains("octo isn't an admin of acme/petstore")
            && out.contains("ask an admin of acme/petstore"),
        "{out}"
    );
}

#[test]
fn setup_polls_a_url_spec_daily() {
    let server = Arc::new(Mutex::new(GitHub::default()));
    let port = serve(server.clone());
    server
        .lock()
        .unwrap()
        .add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let (dir, bin) = checkout("acme/petstore-sdks");
    let url = format!("http://127.0.0.1:{port}/spec.yaml");
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["init", "go", "--spec", &url],
        "",
        &[],
    );
    assert_eq!(code, 0, "{out}");
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--no-browser"],
        "\n\n",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&format!(
            "{url} ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (go/)"
        )),
        "{out}"
    );
    let github = server.lock().unwrap();
    let workflow =
        &github.files("acme/petstore-sdks", "perseid/setup")[".github/workflows/sdks.yml"];
    keep("url-sdks.yml", workflow);
    assert!(
        workflow.contains("paths: [\"perseid.toml\"]\n  schedule:\n    - cron: '"),
        "{workflow}"
    );
    assert!(out.contains("perseid checks the spec every day"), "{out}");
}
