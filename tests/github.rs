//! `perseid setup`, `perseid connect` and `perseid status` against a fake GitHub serving the REST
//! API, OAuth and App pages, for each repository layout.

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
    released: bool,
    default_branch: Option<String>,
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

    /// Makes `branch` the default branch of `repo`, in place of `main`.
    fn rename_main(&mut self, repo: &str, branch: &str) {
        let repo = self.repos.get_mut(repo).unwrap();
        let head = repo.refs.remove("main").unwrap();
        repo.refs.insert(branch.into(), head);
        repo.default_branch = Some(branch.into());
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
                        "default_branch": repo.default_branch.as_deref().unwrap_or("main"),
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
            ("GET", ["keys"]) if self.repos[name].readonly => {
                (404, json!({ "message": "Not Found" }))
            }
            ("GET", ["keys"]) => (200, Value::from(self.repos[name].deploy_keys.clone())),
            ("GET", ["releases"]) => match self.repos[name].released {
                true => (200, json!([{ "tag_name": "v1.0.0" }])),
                false => (200, json!([])),
            },
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

/// Merges the setup pull request of `repo`.
fn merge(github: &mut GitHub, repo: &str) {
    let refs = &mut github.repos.get_mut(repo).unwrap().refs;
    let head = refs["perseid/setup"].clone();
    refs.insert("main".into(), head);
    github.pulls.retain(|p| p["repo"] != repo);
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
        "acme/petstore-typescript",
        &[
            ("README.md", "# TypeScript\n"),
            ("release-please-config.json", existing),
        ],
    );
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(false);
    let args = [
        "init",
        "--sdks",
        "typescript,python,go",
        "--repo",
        "acme/petstore-{lang}",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Next: `perseid generate`"), "{out}");
    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]),
        "?? openapi.yaml\n?? perseid.toml\n",
        "init writes perseid.toml only"
    );

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
            "acme/petstore ──PRs──▶ acme/petstore-typescript, acme/petstore-python, acme/petstore-go"
        ),
        "{out}"
    );
    for line in [
        "  + create acme/petstore-python (private)",
        "  + create acme/petstore-go (private)",
        "  = acme/petstore-typescript: existing, files added only where missing",
        "  + acme/petstore-typescript: commit release-please-config.json, .release-please-manifest.json, .github/workflows/sdk-release.yml (updating release-please-config.json)",
        "  + GitHub App petstore-sdk-bot on acme, created in your browser",
        "  + acme/petstore: pull request with .github/workflows/sdks.yml, perseid.toml\n",
        "? Apply these 7 changes? [Y/n]",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!out.contains("perseid connect"), "the spec is here: {out}");
    let github = server.lock().unwrap();
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    for repo in [
        "acme/petstore-go",
        "acme/petstore-typescript",
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
    let node = &github.files("acme/petstore-typescript", "main")["release-please-config.json"];
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
        workflow
            .contains("paths: [\"openapi.yaml\",\"perseid.toml\",\".github/workflows/sdks.yml\"]"),
        "{workflow}"
    );
    assert!(
        workflow
            .contains("repositories: petstore,petstore-go,petstore-python,petstore-typescript\n"),
        "{workflow}"
    );
    assert_eq!(files["perseid.toml"], config);
    assert_eq!(files["openapi.yaml"], SPEC);
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
        "✓ In sync",
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
    let env = [("PERSEID_GITHUB_CLIENT_ID", "")];
    let args = ["init", "--sdks", "rust"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &env);
    assert_eq!(code, 0, "{out}");
    let (code, out) = perseid(dir.path(), bin.path(), port, &["setup", "--yes"], "", &env);
    assert_ne!(code, 0);
    assert!(
        out.contains("set GH_TOKEN") && out.contains("gh auth login"),
        "{out}"
    );

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

/// A clone of the new SDKs repository acme/petstore-sdks, `init` run with `args`, and set up.
fn set_up_sdks_repository(
    server: &Arc<Mutex<GitHub>>,
    port: u16,
    args: &[&str],
) -> (tempfile::TempDir, tempfile::TempDir, String) {
    server
        .lock()
        .unwrap()
        .add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let (dir, bin) = checkout("acme/petstore-sdks");
    let (code, out) = perseid(dir.path(), bin.path(), port, args, "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("No OpenAPI spec here yet")
            && out.contains("`npx perseid connect acme/petstore-sdks`"),
        "{out}"
    );
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--no-browser"],
        "y\n\n",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    merge(&mut server.lock().unwrap(), "acme/petstore-sdks");
    (dir, bin, out)
}

#[test]
fn connect_pushes_the_spec_to_a_repository_holding_every_sdk() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let args = ["init", "--sdks", "typescript,python"];
    let (sdks, sdks_bin, out) = set_up_sdks_repository(&server, port, &args);
    let diagram = "acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (typescript/, python/)";
    for line in [
        diagram,
        "  ~ acme/petstore-sdks: let GitHub Actions open pull requests",
        "  + acme/petstore-sdks: pull request with .github/workflows/sdks.yml, perseid.toml, release-please-config.json, .release-please-manifest.json and 1 more files",
        "In the repository that holds your OpenAPI spec, run `npx perseid connect acme/petstore-sdks`",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(
        !out.contains("GitHub App"),
        "the default token is enough: {out}"
    );
    {
        let github = server.lock().unwrap();
        let files = github.files("acme/petstore-sdks", "main");
        let workflow = &files[".github/workflows/sdks.yml"];
        keep("sdks-repo-sdks.yml", workflow);
        assert!(
            workflow.contains("- if: hashFiles('openapi.json') != ''"),
            "{workflow}"
        );
        assert!(
            workflow.contains("pull-requests: write") && !workflow.contains("SDK_APP_ID"),
            "{workflow}"
        );
        assert!(!files.contains_key("typescript/package.json"), "{files:?}");
        let api = &github.repos["acme/petstore"];
        assert!(
            api.secrets.is_empty() && api.variables.is_empty(),
            "setup never touches the repository holding the spec"
        );
    }

    let (dir, bin) = api_checkout(true);
    let before = server.lock().unwrap().calls.len();
    let connect = ["connect", "acme/petstore-sdks", "--dry-run"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 2, "changes pending: {out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks (openapi.json)",
        "  + acme/petstore-sdks: write deploy key for acme/petstore, its private half the PERSEID_SDKS_DEPLOY_KEY secret of acme/petstore",
        "  + .github/workflows/perseid-push.yml, written here for you to commit",
        "! `main` of acme/petstore doesn't hold this .github/workflows/perseid-push.yml yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }

    let connect = ["connect", "acme/petstore-sdks", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("1. Review .github/workflows/perseid-push.yml, then commit and push it"),
        "{out}"
    );
    assert!(
        out.contains("on each change"),
        "no release, so each change: {out}"
    );
    let github = server.lock().unwrap();
    deploy_key(&github, "acme/petstore-sdks");
    let api = &github.repos["acme/petstore"];
    assert!(api.variables.is_empty(), "{:?}", api.variables);
    assert_eq!(
        api.secrets.keys().collect::<Vec<_>>(),
        ["PERSEID_SDKS_DEPLOY_KEY"]
    );
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push.yml", push);
    assert!(
        push.starts_with("# Written by `perseid connect acme/petstore-sdks`"),
        "{push}"
    );
    for line in [
        "    paths: [\"openapi.yaml\",\".github/workflows/perseid-push.yml\"]\n",
        "      - uses: meteroid-oss/perseid/push@v0\n        with:\n          spec: \"openapi.yaml\"\n          to: acme/petstore-sdks\n          deploy-key: ${{ secrets.PERSEID_SDKS_DEPLOY_KEY }}\n",
    ] {
        assert!(push.contains(line), "{line}\n{push}");
    }
    assert!(
        !github.repos["acme/petstore"]
            .refs
            .contains_key("perseid/setup"),
        "no commit to the repository holding the spec: its user commits the workflow"
    );
    drop(github);
    let local = git(dir.path(), &["status", "--porcelain"]);
    assert_eq!(local, "?? .github/\n");

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks",
        "✓ .github/workflows/perseid-push.yml is here",
        "✓ In sync",
        "! acme/petstore-sdks hasn't received a spec yet",
        "! acme/petstore: perseid-push.yml hasn't run yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }

    {
        let mut github = server.lock().unwrap();
        let source = "{\n  \"repo\": \"acme/petstore\",\n  \"sha\": \"a1b2c3d4e5f6\",\n  \"ref\": \"v1.4.0\"\n}\n";
        let mut tree = github.files("acme/petstore-sdks", "main");
        tree.insert(".perseid/source.json".into(), source.into());
        tree.insert("openapi.json".into(), SPEC.into());
        let tree = tree
            .into_iter()
            .map(|(path, text)| (path, github.blob(text.as_bytes())))
            .collect();
        let commit = github.commit(tree, "spec: acme/petstore@v1.4.0 (a1b2c3d)");
        let sdks = github.repos.get_mut("acme/petstore-sdks").unwrap();
        sdks.refs.insert("main".into(), commit);
    }
    let (code, out) = perseid(sdks.path(), sdks_bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("✓ Last spec from acme/petstore at v1.4.0 (a1b2c3d), "),
        "{out}"
    );
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("✓ Last spec from acme/petstore at v1.4.0 (a1b2c3d), "),
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
    assert!(
        out.contains("→ run `perseid connect acme/petstore-sdks`"),
        "{out}"
    );
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    let key = deploy_key(&server.lock().unwrap(), "acme/petstore-sdks");
    assert_ne!(key["key"], old, "the key without its secret is replaced");
}

#[test]
fn connect_pushes_releases_to_an_orchestrator_of_one_repository_per_language() {
    let mut fake = GitHub::with_spec_repo();
    fake.repos.get_mut("acme/petstore").unwrap().released = true;
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let args = [
        "init",
        "--sdks",
        "typescript,python",
        "--repo",
        "acme/petstore-{lang}",
        "--name",
        "Petstore",
    ];
    let (_sdks, _bin, out) = set_up_sdks_repository(&server, port, &args);
    assert!(
        out.contains("acme/petstore-sdks ──PRs──▶ acme/petstore-typescript, acme/petstore-python"),
        "{out}"
    );
    {
        let github = server.lock().unwrap();
        for repo in [
            "acme/petstore-sdks",
            "acme/petstore-typescript",
            "acme/petstore-python",
        ] {
            assert_eq!(
                github.repos[repo].secrets["SDK_APP_PRIVATE_KEY"], PEM,
                "{repo}"
            );
        }
        let files = github.files("acme/petstore-sdks", "main");
        let workflow = &files[".github/workflows/sdks.yml"];
        keep("sdks-repo-split-sdks.yml", workflow);
        assert!(
            workflow.contains("repositories: petstore-sdks,petstore-python,petstore-typescript\n"),
            "{workflow}"
        );
        assert!(
            !files.contains_key(".github/workflows/sdk-release.yml"),
            "{files:?}"
        );
    }

    let (dir, bin) = api_checkout(false);
    let connect = ["connect", "acme/petstore-sdks", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_ne!(code, 0);
    assert!(
        out.contains("openapi.yaml isn't committed: pass --build"),
        "{out}"
    );

    let connect = [
        "connect",
        "acme/petstore-sdks",
        "--yes",
        "--build",
        "make openapi.yaml",
        "--private",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("on each published release"), "{out}");
    let github = server.lock().unwrap();
    let api = &github.repos["acme/petstore"];
    assert!(
        !api.secrets.contains_key("SDK_APP_PRIVATE_KEY"),
        "the App never reaches the repository holding the spec"
    );
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-release-connect.yml", push);
    for line in [
        "\n  release:\n    types: [published]\n  workflow_dispatch:\n",
        "      - name: Write the spec\n        run: |\n          make openapi.yaml\n",
        "          private: true\n",
    ] {
        assert!(push.contains(line), "{line}\n{push}");
    }
    deploy_key(&github, "acme/petstore-sdks");
    drop(github);

    let retag = [
        "connect",
        "acme/petstore-sdks",
        "--yes",
        "--on",
        "tag",
        "--tags",
        "api-v*",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &retag, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("  ~ .github/workflows/perseid-push.yml, written here for you to commit"),
        "{out}"
    );
    let github = server.lock().unwrap();
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-tag-connect.yml", push);
    assert!(push.contains("\n    tags: [\"api-v*\"]\n"), "{push}");
    assert!(
        push.contains("make openapi.yaml") && push.contains("private: true"),
        "the other settings are kept: {push}"
    );
    assert_eq!(github.repos["acme/petstore-sdks"].deploy_keys.len(), 1);
}

#[test]
fn connect_declined_writes_the_workflow_only_and_says_how_to_add_the_key() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    set_up_sdks_repository(&server, port, &["init", "--sdks", "go"]);
    let (dir, bin) = api_checkout(true);
    let before = server.lock().unwrap().calls.len();
    let connect = ["connect", "acme/petstore-sdks", "--on", "change"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "n\n", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    for line in [
        "its private half the PERSEID_SDKS_DEPLOY_KEY secret of acme/petstore.\nNothing is saved on this machine.",
        "Nothing changed on GitHub. To add the deploy key yourself:",
        "gh repo deploy-key add perseid_key.pub -R acme/petstore-sdks --allow-write",
        "gh secret set PERSEID_SDKS_DEPLOY_KEY -R acme/petstore < perseid_key",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(
        dir.path()
            .join(".github/workflows/perseid-push.yml")
            .exists()
    );
}

#[test]
fn connect_with_a_token_changes_nothing_on_github() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    set_up_sdks_repository(&server, port, &["init", "--sdks", "go"]);
    let (dir, bin) = api_checkout(true);
    let before = server.lock().unwrap().calls.len();
    let connect = [
        "connect",
        "acme/petstore-sdks",
        "--on",
        "change",
        "--auth",
        "token",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    assert!(
        out.contains("! acme/petstore needs PERSEID_SDKS_TOKEN, a fine-grained token with Contents read and write on acme/petstore-sdks: `gh secret set PERSEID_SDKS_TOKEN -R acme/petstore`"),
        "{out}"
    );
    assert!(!out.contains("deploy key"), "{out}");
    let push = fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-token-connect.yml", &push);
    assert!(
        push.contains("          token: ${{ secrets.PERSEID_SDKS_TOKEN }}\n"),
        "{push}"
    );

    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["connect", "acme/petstore-sdks"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("In sync"), "--auth is kept: {out}");
}

#[test]
fn connect_without_admin_rights_on_the_sdks_repository_says_what_an_admin_does() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let args = ["init", "--sdks", "go"];
    set_up_sdks_repository(&server, port, &args);
    server
        .lock()
        .unwrap()
        .repos
        .get_mut("acme/petstore-sdks")
        .unwrap()
        .readonly = true;
    let (dir, bin) = api_checkout(true);
    let connect = ["connect", "acme/petstore-sdks", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "→ An admin of acme/petstore-sdks must add its public half at http://127.0.0.1:"
        ),
        "{out}"
    );
    assert!(
        out.contains("/web/acme/petstore-sdks/settings/keys/new"),
        "{out}"
    );
    assert!(out.contains("  Key:   ssh-ed25519 "), "{out}");
    let github = server.lock().unwrap();
    assert!(github.repos["acme/petstore-sdks"].deploy_keys.is_empty());
    assert!(
        github.repos["acme/petstore"]
            .secrets
            .contains_key("PERSEID_SDKS_DEPLOY_KEY")
    );
    drop(github);

    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert!(
        out.contains("can't see the deploy keys of acme/petstore-sdks"),
        "{out}"
    );
}

#[test]
fn connect_needs_the_sdks_repository_set_up_and_admin_rights_here() {
    let mut fake = GitHub::with_spec_repo();
    fake.add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(true);
    let connect = ["connect", "acme/petstore-sdks", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_ne!(code, 0);
    assert!(
        out.contains(
            "acme/petstore-sdks has no perseid.toml on `main`: run `npx perseid init` there"
        ),
        "{out}"
    );
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["connect", "acme/petstore"],
        "",
        &TOKEN,
    );
    assert_ne!(code, 0);
    assert!(out.contains("nothing to connect"), "{out}");

    server
        .lock()
        .unwrap()
        .repos
        .get_mut("acme/petstore")
        .unwrap()
        .readonly = true;
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_ne!(code, 0);
    assert!(
        out.contains("octo isn't an admin of acme/petstore")
            && out.contains(
                "ask an admin of acme/petstore to run `npx perseid connect acme/petstore-sdks`"
            ),
        "{out}"
    );
}

/// A clone of acme/petstore-sdks, an SDK monorepo generated from a URL until now.
fn sdks_checkout(fake: &mut GitHub) -> (tempfile::TempDir, tempfile::TempDir) {
    let config = "spec = \"https://api.example.com/openapi.yaml\"\nname = \"Petstore\"\nsdks = [\"typescript\"]\n\n[typescript]\npath = \"ts\"\n";
    let release = "{\n  \"packages\": {\n    \"docs\": {\n      \"release-type\": \"simple\"\n    }\n  }\n}\n";
    let handwritten = "export const mine = 1;\n";
    let files = [
        ("perseid.toml", config),
        ("release-please-config.json", release),
        ("ts/src/index.ts", handwritten),
        (".github/workflows/sdk-release.yml", "name: Releases\n"),
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
fn setup_completes_an_existing_sdks_repository_receiving_the_spec() {
    let mut fake = GitHub::with_spec_repo();
    let (dir, bin) = sdks_checkout(&mut fake);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    edit(dir.path(), |t| {
        t.replace("https://api.example.com/openapi.yaml", "openapi.yaml")
    });
    fs::write(dir.path().join("openapi.yaml"), SPEC).unwrap();

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
        "acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (ts/)",
        "  + acme/petstore-sdks: pull request with .github/workflows/sdks.yml, perseid.toml, release-please-config.json, .release-please-manifest.json and ",
        "(updating perseid.toml, release-please-config.json)",
        "! acme/petstore-sdks: perseid would overwrite files it didn't generate, and stops instead: ts/src/index.ts",
        "! acme/petstore-sdks keeps its own .github/workflows/sdk-release.yml: check it runs release-please on `main`",
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
    assert_eq!(
        setup[".github/workflows/sdk-release.yml"],
        "name: Releases\n"
    );
    assert!(
        setup["perseid.toml"].contains("[typescript]\npath = \"ts\"\n"),
        "user settings kept"
    );
    assert_eq!(setup["openapi.yaml"], SPEC);
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

    edit(dir.path(), |t| format!("release = false\n{t}"));
    let (code, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &["setup", "--dry-run"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 2, "{out}");
    assert!(
        !out.contains("sdk-release.yml") && !out.contains(".release-please-manifest.json"),
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
        &["init", "--sdks", "go", "--spec", &url],
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

#[test]
fn setup_writes_the_release_files_at_the_root_of_a_repository_holding_perseid_toml_in_a_folder() {
    let config = "spec = \"openapi.yaml\"\nname = \"Petstore\"\nsdks = [\"typescript\", \"go\"]\n";
    let outdated = "# Written by `perseid setup`, yours to edit.\nname: SDK Release\n";
    let mut fake = GitHub::default();
    fake.add_repo(
        "acme/petstore",
        &[
            ("api/openapi.yaml", SPEC),
            (".github/workflows/sdk-release.yml", outdated),
        ],
    );
    fake.rename_main("acme/petstore", "trunk");
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = checkout("acme/petstore");
    let api = dir.path().join("api");
    fs::create_dir_all(&api).unwrap();
    fs::write(api.join("openapi.yaml"), SPEC).unwrap();
    fs::write(api.join("perseid.toml"), config).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "--quiet", "-m", "spec"]);

    let (code, out) = perseid(&api, bin.path(), port, &["setup", "--yes"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("(updating .github/workflows/sdk-release.yml)"),
        "perseid wrote it, so it is rewritten: {out}"
    );
    let github = server.lock().unwrap();
    let files = github.files("acme/petstore", "perseid/setup");
    assert!(
        !files
            .keys()
            .any(|path| path.starts_with("api/.") || path.starts_with("api/release-please")),
        "{files:?}"
    );
    let release: Value = serde_json::from_str(&files["release-please-config.json"]).unwrap();
    let packages: Vec<&String> = release["packages"].as_object().unwrap().keys().collect();
    assert_eq!(packages, ["api/typescript", "api/go"]);
    assert_eq!(release["packages"]["api/go"]["component"], "api/go");
    let workflow = &files[".github/workflows/sdk-release.yml"];
    keep("subfolder-sdk-release.yml", workflow);
    assert!(workflow.contains("branches: [\"trunk\"]\n"), "{workflow}");
    assert!(
        files[".github/workflows/sdks.yml"].contains("working-directory: api\n"),
        "{files:?}"
    );
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    for path in [
        ".github/workflows/sdk-release.yml",
        ".release-please-manifest.json",
        "release-please-config.json",
    ] {
        assert!(staged.lines().any(|line| line == path), "{path}\n{staged}");
    }
}
