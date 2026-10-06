//! `perseid sync`, `perseid app`, `perseid connect` and `perseid status` against a fake GitHub
//! serving the REST API, OAuth and App pages, for each repository layout.

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
    released: bool,
    default_branch: Option<String>,
    /// The default branch takes no direct push.
    protected: bool,
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
    /// Repositories of the hosted perseid App's installation, once listed `hosted_after` times.
    hosted: Vec<String>,
    hosted_after: usize,
    hosted_lists: usize,
    /// The hosted App's installation covers all of acme's repositories.
    hosted_everywhere: bool,
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
            ("GET", ["api", "user", "installations"]) => {
                self.hosted_lists += 1;
                let installations = match self.hosted_lists > self.hosted_after {
                    true => json!([{
                        "id": 99, "app_slug": "perseid-sdks", "account": { "login": "acme" },
                        "repository_selection": if self.hosted_everywhere { "all" } else { "selected" },
                    }]),
                    false => json!([]),
                };
                (200, json!({ "installations": installations }))
            }
            ("GET", ["api", "user", "installations", "99", "repositories"]) => {
                let listed: Vec<Value> = self
                    .hosted
                    .iter()
                    .map(|r| json!({ "full_name": r }))
                    .collect();
                (200, json!({ "repositories": listed }))
            }
            ("GET", ["api", "users", login]) => (
                200,
                json!({ "login": login, "type": "Organization", "id": id_of(login) }),
            ),
            ("GET", ["api", "orgs", _, "installations"]) => {
                let installations = match self.app_created {
                    true => json!([{ "app_id": 42, "repository_selection": "selected" }]),
                    false => json!([]),
                };
                (200, json!({ "installations": installations }))
            }
            ("GET", ["api", "app"]) => {
                let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
                validation.set_required_spec_claims(&["exp"]);
                let key = jsonwebtoken::DecodingKey::from_rsa_pem(PUBLIC_PEM.as_bytes()).unwrap();
                match jsonwebtoken::decode::<Value>(auth.unwrap(), &key, &validation) {
                    Ok(jwt) if jwt.claims["iss"] == 42 => {
                        (200, json!({ "id": 42, "slug": "petstore-sdk-bot" }))
                    }
                    _ => (
                        401,
                        json!({ "message": "A JSON web token could not be decoded" }),
                    ),
                }
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
                        "id": id_of(name),
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
            ("PUT", ["contents", path @ ..]) => {
                let repo = &self.repos[name];
                let default = repo.default_branch.clone().unwrap_or_else(|| "main".into());
                let branch = body["branch"].as_str().unwrap_or(&default).to_owned();
                if repo.protected && branch == default {
                    return (
                        409,
                        json!({ "message": "Changes must be made through a pull request." }),
                    );
                }
                let mut tree = match repo.refs.contains_key(&branch) {
                    true => self.files(name, &branch),
                    false if repo.refs.is_empty() => BTreeMap::new(),
                    false => return (404, json!({ "message": "Branch not found" })),
                };
                let path = path.join("/");
                assert_eq!(
                    body["sha"].is_string(),
                    tree.contains_key(&path),
                    "a sha updates an existing file only"
                );
                let content = BASE64.decode(body["content"].as_str().unwrap()).unwrap();
                tree.insert(path, String::from_utf8(content).unwrap());
                let tree = tree
                    .into_iter()
                    .map(|(path, text)| (path, self.blob(text.as_bytes())))
                    .collect();
                let commit = self.commit(tree, body["message"].as_str().unwrap());
                self.repos
                    .get_mut(name)
                    .unwrap()
                    .refs
                    .insert(branch, commit);
                (201, json!({}))
            }
            ("POST", ["git", "refs"]) => {
                let branch = body["ref"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("refs/heads/")
                    .unwrap();
                let refs = &mut self.repos.get_mut(name).unwrap().refs;
                if refs.contains_key(branch) {
                    return (422, json!({ "message": "Reference already exists" }));
                }
                refs.insert(branch.into(), body["sha"].as_str().unwrap().into());
                (201, json!({}))
            }
            ("PATCH", ["git", "refs", "heads", branch @ ..]) => {
                let refs = &mut self.repos.get_mut(name).unwrap().refs;
                refs.insert(branch.join("/"), body["sha"].as_str().unwrap().into());
                (200, json!({}))
            }
            ("GET", ["contents", path @ ..]) => {
                let branch = param("ref").unwrap();
                match self.repos[name].refs.get(&branch) {
                    None => (404, json!({ "message": "No commit found for the ref" })),
                    Some(_) => match self.files(name, &branch).get(&path.join("/")) {
                        Some(text) if raw => (200, Value::String(text.clone())),
                        Some(text) => (
                            200,
                            json!({ "content": BASE64.encode(text), "sha": digest(text) }),
                        ),
                        None => (404, json!({ "message": "Not Found" })),
                    },
                }
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
            ("GET", ["commits"]) => match self.repos[name].refs.get("main") {
                Some(sha) => (
                    200,
                    json!([{ "sha": sha, "commit": { "committer": { "date": "2026-09-30T10:00:00Z" } } }]),
                ),
                None => (409, json!({ "message": "Git Repository is empty." })),
            },
            ("GET", ["releases"]) => match self.repos[name].released {
                true => (200, json!([{ "tag_name": "v1.0.0" }])),
                false => (200, json!([])),
            },
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

/// A stable numeric id for a repository or an account.
fn id_of(name: &str) -> u64 {
    u64::from_str_radix(&digest(name.to_lowercase())[..8], 16).unwrap()
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

/// Commits everything in the checkout `dir` and pushes it to the default branch of `repo`, as
/// its user does with what `perseid init` wrote.
fn push_checkout(server: &Arc<Mutex<GitHub>>, repo: &str, dir: &Path) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &["commit", "--quiet", "--allow-empty", "-m", "perseid init"],
    );
    let listed = git(dir, &["ls-files"]);
    let mut github = server.lock().unwrap();
    let mut tree = github.files(repo, "main");
    for path in listed.lines() {
        tree.insert(path.to_owned(), fs::read_to_string(dir.join(path)).unwrap());
    }
    let tree = tree
        .into_iter()
        .map(|(path, text)| (path, github.blob(text.as_bytes())))
        .collect();
    let commit = github.commit(tree, "perseid init");
    github
        .repos
        .get_mut(repo)
        .unwrap()
        .refs
        .insert("main".into(), commit);
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
fn app_opens_the_pull_requests_of_one_repository_per_language() {
    let mut fake = GitHub::with_spec_repo();
    fake.add_repo(
        "acme/petstore-typescript",
        &[("README.md", "# TypeScript\n")],
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
    for line in [
        "+ perseid.toml\n+ .github/workflows/sdks.yml\n",
        "Packages: npm petstore, PyPI petstore, Go github.com/acme/petstore-go",
        "Create the SDK repositories: `gh repo create acme/petstore-go --private`, `gh repo create acme/petstore-python --private`, `gh repo create acme/petstore-typescript --private`",
        "Run `perseid sync`: it installs the perseid App on acme/petstore, acme/petstore-go, acme/petstore-python, acme/petstore-typescript",
        "`perseid app` sets up a GitHub App of your own instead",
        "PyPI: add acme/petstore-python as the pending publisher of petstore",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]),
        "?? .github/\n?? openapi.yaml\n?? perseid.toml\n",
        "init writes perseid.toml and sdks.yml, and no release files for SDKs living elsewhere"
    );
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/sdks.yml")).unwrap();
    keep("per-language-sdks.yml", &workflow);
    assert!(
        workflow
            .contains("paths: [\"openapi.yaml\",\"perseid.toml\",\".github/workflows/sdks.yml\"]"),
        "{workflow}"
    );
    assert!(
        workflow.contains("token: ${{ secrets.SDK_GITHUB_TOKEN }}\n")
            && workflow.contains("app-id: ${{ vars.SDK_APP_ID }}\n")
            && !workflow.contains("create-github-app-token"),
        "the action mints the App token: {workflow}"
    );

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    for line in [
        "! create the SDK repositories perseid.toml names: `gh repo create acme/petstore-go --private`, `gh repo create acme/petstore-python --private`",
        "    + perseid App installed on petstore, petstore-typescript, in your browser",
        "    + acme/petstore-typescript: .github/workflows/sdk-release.yml, committed with your credentials",
        "→ run `perseid sync`",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }

    {
        let mut github = server.lock().unwrap();
        github.add_repo("acme/petstore-python", &[("README.md", "# Python\n")]);
        github.add_repo("acme/petstore-go", &[("README.md", "# Go\n")]);
    }
    let before = server.lock().unwrap().calls.len();
    let app = ["app", "--no-browser"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &app, "\n\n", &[]);
    assert_eq!(code, 0, "{out}");
    for line in [
        "and enter the code ABCD-1234",
        "✓ Signed in to GitHub as octo (browser)",
        "acme/petstore ──PRs──▶ acme/petstore-typescript, acme/petstore-python, acme/petstore-go",
        "  + GitHub App petstore-sdk-bot on acme, created in your browser",
        "  + acme/petstore-go: .github/workflows/sdk-release.yml, committed with your credentials",
        "? Apply these changes? [Y/n]",
        "✓ acme/petstore-go: committed .github/workflows/sdk-release.yml to `main`",
        "sdks.yml and sdk-release.yml now open their pull requests as your App",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let github = server.lock().unwrap();
    for repo in [
        "acme/petstore",
        "acme/petstore-go",
        "acme/petstore-typescript",
        "acme/petstore-python",
    ] {
        let repo = &github.repos[repo];
        assert_eq!(repo.variables["SDK_APP_ID"], "42");
        assert_eq!(
            repo.secrets["SDK_APP_PRIVATE_KEY"], PEM,
            "decrypted with the repository key"
        );
        assert_eq!(repo.refs.keys().collect::<Vec<_>>(), ["main"]);
    }
    for repo in [
        "acme/petstore-go",
        "acme/petstore-typescript",
        "acme/petstore-python",
    ] {
        let workflow = &github.files(repo, "main")[".github/workflows/sdk-release.yml"];
        assert!(
            workflow.contains("uses: meteroid-oss/perseid/release@v0\n"),
            "{workflow}"
        );
    }
    assert!(
        !github
            .files("acme/petstore", "main")
            .contains_key(".github/workflows/sdk-release.yml"),
        "no SDK lives in the repository holding perseid.toml"
    );
    assert_eq!(
        github.repos["acme/petstore"].variables["SDK_APP_SLUG"],
        "petstore-sdk-bot"
    );
    assert!(github.installation_checks > 3, "polled until installed");
    assert!(github.pulls.is_empty());
    assert!(
        github.writes(before).iter().all(|w| w.contains("/actions/")
            || w.contains("app-manifests")
            || w.contains("/login/")
            || w.ends_with("/contents/.github/workflows/sdk-release.yml")),
        "{:?}",
        github.writes(before)
    );
    drop(github);

    let before = server.lock().unwrap().calls.len();
    let (code, out) = perseid(dir.path(), bin.path(), port, &["app", "--yes"], "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ In sync: nothing to change"), "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());

    server
        .lock()
        .unwrap()
        .repos
        .get_mut("acme/petstore-go")
        .unwrap()
        .secrets
        .clear();
    let home = tempfile::tempdir().unwrap();
    let downloads = home.path().join("Downloads");
    fs::create_dir(&downloads).unwrap();
    let downloaded = downloads.join("petstore-sdk-bot.2026-10-05.private-key.pem");
    fs::write(&downloaded, PEM).unwrap();
    let garbage = home.path().join("notes.txt");
    fs::write(&garbage, "not a key").unwrap();
    let answers = format!("\n{}\n\n\n", garbage.display());
    let env = [TOKEN[0], ("HOME", home.path().to_str().unwrap())];
    let (code, out) = perseid(dir.path(), bin.path(), port, &app, &answers, &env);
    assert_eq!(code, 0, "{out}");
    for line in [
        "  + SDK_APP_PRIVATE_KEY on acme/petstore-go: a new key of the App, generated in your browser",
        "→ Generate a private key of the App, under \"Private keys\": http://127.0.0.1",
        "/web/organizations/acme/settings/apps/petstore-sdk-bot",
        "notes.txt: not an RSA private key",
        "✓ SDK_APP_PRIVATE_KEY is set on acme/petstore-go",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert_eq!(
        server.lock().unwrap().repos["acme/petstore-go"].secrets["SDK_APP_PRIVATE_KEY"],
        PEM
    );
    assert!(!downloaded.exists(), "the downloaded key is deleted");

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "sdks.yml isn't pushed yet: {out}");
    push_checkout(&server, "acme/petstore", dir.path());
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
fn app_without_credentials_says_how_to_get_them() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server);
    let (dir, bin) = api_checkout(false);
    let env = [("PERSEID_GITHUB_CLIENT_ID", "")];
    let args = ["init", "--sdks", "rust"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &args, "", &env);
    assert_eq!(code, 0, "{out}");
    let setup = ["app", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &setup, "", &env);
    assert_ne!(code, 0);
    assert!(
        out.contains("set GH_TOKEN") && out.contains("gh auth login"),
        "{out}"
    );

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &[]);
    assert_eq!(code, 0, "{out}");
    for line in [
        "acme/petstore ──PRs──▶ acme/petstore (rust/)",
        "✓ perseid.toml is valid",
        "✓ .github/workflows/sdks.yml is here",
        "! Not signed in to GitHub: only local checks ran",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }

    fs::write(dir.path().join("perseid.toml"), "sdks = [\"cobol\"]\n").unwrap();
    let (code, out) = perseid(dir.path(), bin.path(), port, &setup, "", &env);
    assert_ne!(code, 0);
    assert!(
        out.contains("cobol") && !out.contains("gh auth login"),
        "perseid.toml is checked before signing in: {out}"
    );
}

/// A clone of the new SDKs repository acme/petstore-sdks, `init` run with `args`, and pushed.
fn set_up_sdks_repository(
    server: &Arc<Mutex<GitHub>>,
    port: u16,
    args: &[&str],
) -> (tempfile::TempDir, tempfile::TempDir) {
    server
        .lock()
        .unwrap()
        .add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let (dir, bin) = checkout("acme/petstore-sdks");
    let (code, out) = perseid(dir.path(), bin.path(), port, args, "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Spec: openapi.json, once `perseid connect` pushes it here")
            && out.contains("`npx perseid connect acme/petstore-sdks`"),
        "{out}"
    );
    push_checkout(server, "acme/petstore-sdks", dir.path());
    (dir, bin)
}

#[test]
fn connect_pushes_the_spec_to_a_repository_holding_every_sdk() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let args = ["init", "--sdks", "typescript,python"];
    let (sdks, sdks_bin) = set_up_sdks_repository(&server, port, &args);
    let (code, out) = perseid(sdks.path(), sdks_bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    for line in [
        "acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (typescript/, python/)",
        "    + perseid App installed on petstore-sdks, in your browser",
        "→ run `perseid sync`",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!out.contains("GitHub App petstore"), "{out}");
    server
        .lock()
        .unwrap()
        .repos
        .get_mut("acme/petstore-sdks")
        .unwrap()
        .secrets
        .insert("SDK_GITHUB_TOKEN".into(), "github_pat_x".into());
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
            workflow.contains("token: ${{ secrets.SDK_GITHUB_TOKEN }}\n")
                && workflow.contains("app-private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}\n"),
            "{workflow}"
        );
        assert!(!files.contains_key("typescript/package.json"), "{files:?}");
        let api = &github.repos["acme/petstore"];
        assert!(
            api.secrets.is_empty() && api.variables.is_empty(),
            "init never touches the repository holding the spec"
        );
    }

    let (dir, bin) = api_checkout(true);
    let before = server.lock().unwrap().calls.len();
    let connect = [
        "connect",
        "acme/petstore-sdks",
        "--dry-run",
        "--auth",
        "token",
    ];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 2, "changes pending: {out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks (openapi.json)",
        "→ perseid-push.yml pushes with a fine-grained token, the SDK_GITHUB_TOKEN secret of acme/petstore.",
        "  + acme/petstore: SDK_GITHUB_TOKEN secret, a fine-grained token you paste, with Contents read and write on acme/petstore-sdks",
        "  + .github/workflows/perseid-push.yml, written here for you to commit",
        "! `main` of acme/petstore doesn't hold this .github/workflows/perseid-push.yml yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }

    let connect = ["connect", "acme/petstore-sdks", "--yes", "--auth", "token"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_ne!(code, 0);
    assert!(
        out.contains("--yes takes the token from SDK_GITHUB_TOKEN"),
        "{out}"
    );
    let pasted = [TOKEN[0], ("SDK_GITHUB_TOKEN", "github_pat_api")];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &pasted);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("✓ SDK_GITHUB_TOKEN is set on acme/petstore"),
        "{out}"
    );
    assert!(
        out.contains("1. Review .github/workflows/perseid-push.yml, then commit and push it"),
        "{out}"
    );
    assert!(
        out.contains("on each change"),
        "no release, so each change: {out}"
    );
    let github = server.lock().unwrap();
    let api = &github.repos["acme/petstore"];
    assert!(api.variables.is_empty(), "{:?}", api.variables);
    assert_eq!(api.secrets["SDK_GITHUB_TOKEN"], "github_pat_api");
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push.yml", push);
    assert!(
        push.starts_with("# Written by `perseid connect acme/petstore-sdks`"),
        "{push}"
    );
    for line in [
        "    paths: [\"openapi.yaml\",\".github/workflows/perseid-push.yml\"]\n",
        &format!(
            "      - uses: {}\n        with:\n          spec: \"openapi.yaml\"\n          to: acme/petstore-sdks\n          token: ${{{{ secrets.SDK_GITHUB_TOKEN }}}}\n",
            perseid::github::uses("push")
        ),
    ] {
        assert!(push.contains(line), "{line}\n{push}");
    }
    assert_eq!(
        github.repos["acme/petstore"]
            .refs
            .keys()
            .collect::<Vec<_>>(),
        ["main"],
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

    {
        let mut github = server.lock().unwrap();
        let api = github.repos.get_mut("acme/petstore").unwrap();
        api.secrets.remove("SDK_GITHUB_TOKEN");
    }
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("! 1 change pending:\n    + acme/petstore: SDK_GITHUB_TOKEN secret"),
        "{out}"
    );
    assert!(
        out.contains("→ run `perseid connect acme/petstore-sdks`"),
        "{out}"
    );
}

#[test]
fn the_perseid_app_needs_no_secret_anywhere() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
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
    let (sdks, sdks_bin) = set_up_sdks_repository(&server, port, &args);
    {
        let mut github = server.lock().unwrap();
        github.add_repo(
            "acme/petstore-typescript",
            &[("README.md", "# TypeScript\n")],
        );
        github.add_repo("acme/petstore-python", &[("README.md", "# Python\n")]);
        github
            .repos
            .get_mut("acme/petstore-python")
            .unwrap()
            .protected = true;
        github.hosted = [
            "acme/petstore-sdks",
            "acme/petstore-python",
            "acme/petstore-typescript",
        ]
        .map(str::to_owned)
        .to_vec();
        github.hosted_after = 2;
    }

    let sync = ["sync", "--no-browser"];
    let (code, out) = perseid(sdks.path(), sdks_bin.path(), port, &sync, "\n\n", &TOKEN);
    assert_eq!(code, 0, "{out}");
    for line in [
        "  + perseid App installed on petstore-sdks, petstore-python, petstore-typescript, in your browser",
        "  + acme/petstore-python: .github/workflows/sdk-release.yml, committed with your credentials",
        "→ Install the perseid App on acme, choosing \"Only select repositories\": petstore-sdks, petstore-python, petstore-typescript",
        &format!(
            "/web/apps/perseid-sdks/installations/new/permissions?suggested_target_id={}&repository_ids[]={}",
            id_of("acme"),
            id_of("acme/petstore-sdks")
        ),
        "✓ The perseid App is installed on petstore-sdks, petstore-python, petstore-typescript",
        "✓ acme/petstore-typescript: committed .github/workflows/sdk-release.yml to `main`",
        "✓ acme/petstore-python: .github/workflows/sdk-release.yml is in a pull request, to merge: https://github.com/acme/petstore-python/pull/1",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    {
        let github = server.lock().unwrap();
        for repo in github.repos.values() {
            assert!(repo.secrets.is_empty() && repo.variables.is_empty());
        }
        let release = ".github/workflows/sdk-release.yml";
        assert!(
            github
                .files("acme/petstore-typescript", "main")
                .contains_key(release)
        );
        assert!(
            !github
                .files("acme/petstore-python", "main")
                .contains_key(release)
        );
        let proposed = &github.files("acme/petstore-python", "perseid/release-workflow")[release];
        assert!(proposed.contains("      id-token: write\n"), "{proposed}");
        assert_eq!(github.pulls[0]["base"], "main");
    }
    let (code, out) = perseid(
        sdks.path(),
        sdks_bin.path(),
        port,
        &["sync", "--yes"],
        "",
        &TOKEN,
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "  = perseid App installed on petstore-sdks, petstore-python, petstore-typescript"
        ) && out.contains(
            "is in a pull request, to merge: https://github.com/acme/petstore-python/pull/1"
        ),
        "the pull request is updated, not opened again: {out}"
    );
    assert_eq!(server.lock().unwrap().pulls.len(), 1);

    server.lock().unwrap().hosted_everywhere = true;
    let (code, out) = perseid(sdks.path(), sdks_bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(
            "! the perseid App is installed on all of acme's repositories, which it refuses"
        ),
        "{out}"
    );
    server.lock().unwrap().hosted_everywhere = false;

    let (dir, bin) = api_checkout(true);
    let connect = ["connect", "acme/petstore-sdks", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    let line = format!(
        "source = {{ repo = \"acme/petstore\", id = {}, target_id = {} }}",
        id_of("acme/petstore"),
        id_of("acme/petstore-sdks")
    );
    for expected in [
        "→ perseid-push.yml pushes as the perseid App, installed on acme/petstore-sdks, whose perseid.toml names acme/petstore as its source.",
        &format!("  + acme/petstore-sdks/perseid.toml: `{line}`, through a pull request"),
        "✓ acme/petstore-sdks: perseid.toml is in a pull request, to merge: https://github.com/acme/petstore-sdks/pull/2",
        "2. Merge the pull request adding `source` to the perseid.toml of acme/petstore-sdks",
    ] {
        assert!(out.contains(expected), "{expected}\n{out}");
    }
    {
        let mut github = server.lock().unwrap();
        let api = &github.repos["acme/petstore"];
        assert!(api.secrets.is_empty() && api.variables.is_empty());
        let config = &github.files("acme/petstore-sdks", "perseid/source")["perseid.toml"];
        assert!(config.contains(&format!("{line}\n")), "{config}");
        assert!(
            perseid::config::Config::parse(config, "perseid.toml").is_ok(),
            "{config}"
        );
        let merged = github.repos["acme/petstore-sdks"].refs["perseid/source"].clone();
        github
            .repos
            .get_mut("acme/petstore-sdks")
            .unwrap()
            .refs
            .insert("main".into(), merged);
    }
    let push = fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-perseid-app.yml", &push);
    assert!(
        push.contains("  id-token: write\n")
            && !push.contains("          token:")
            && !push.contains("secrets."),
        "{push}"
    );
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_eq!(code, 0, "{out}");
    for expected in [
        "  = acme/petstore-sdks/perseid.toml names acme/petstore as its source",
        "✓ In sync: nothing to change",
    ] {
        assert!(out.contains(expected), "{expected}\n{out}");
    }
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
    set_up_sdks_repository(&server, port, &args);
    {
        let mut github = server.lock().unwrap();
        let variables = &mut github
            .repos
            .get_mut("acme/petstore-sdks")
            .unwrap()
            .variables;
        variables.insert("SDK_APP_ID".into(), "42".into());
        variables.insert("SDK_APP_SLUG".into(), "petstore-sdk-bot".into());
        let files = github.files("acme/petstore-sdks", "main");
        let workflow = &files[".github/workflows/sdks.yml"];
        keep("sdks-repo-split-sdks.yml", workflow);
        assert!(
            workflow.contains("app-id: ${{ vars.SDK_APP_ID }}\n") && !workflow.contains("owner:"),
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
    let home = tempfile::tempdir().unwrap();
    let downloaded = home
        .path()
        .join("Downloads/petstore-sdk-bot.2026-10-05.private-key.pem");
    fs::create_dir(downloaded.parent().unwrap()).unwrap();
    fs::write(&downloaded, PEM).unwrap();
    let env = [TOKEN[0], ("HOME", home.path().to_str().unwrap())];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &env);
    assert_eq!(code, 0, "{out}");
    for line in [
        "→ perseid-push.yml pushes as the GitHub App of acme/petstore-sdks",
        "  + acme/petstore: SDK_APP_ID variable, the ID of the GitHub App petstore-sdk-bot",
        "  + acme/petstore: SDK_APP_PRIVATE_KEY secret, a new key of the App, generated in your browser",
        "/web/organizations/acme/settings/apps/petstore-sdk-bot",
        "✓ SDK_APP_PRIVATE_KEY is set on acme/petstore",
        "on each published release",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert!(!downloaded.exists());
    let github = server.lock().unwrap();
    let api = &github.repos["acme/petstore"];
    assert_eq!(api.variables["SDK_APP_ID"], "42");
    assert_eq!(api.secrets["SDK_APP_PRIVATE_KEY"], PEM);
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-release-connect.yml", push);
    for line in [
        "\n  release:\n    types: [published]\n  workflow_dispatch:\n",
        "      - name: Write the spec\n        run: |\n          make openapi.yaml\n",
        "          private: true\n",
        "          app-id: ${{ vars.SDK_APP_ID }}\n          private-key: ${{ secrets.SDK_APP_PRIVATE_KEY }}\n          owner: acme\n          repositories: petstore-sdks\n          permission-contents: write\n",
        "          token: ${{ steps.app.outputs.token }}\n",
    ] {
        assert!(push.contains(line), "{line}\n{push}");
    }
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
    for line in [
        "  ~ .github/workflows/perseid-push.yml, written here for you to commit",
        "  = acme/petstore: SDK_APP_PRIVATE_KEY secret",
    ] {
        assert!(out.contains(line), "the App is kept: {line}\n{out}");
    }
    let push = &fs::read_to_string(dir.path().join(".github/workflows/perseid-push.yml")).unwrap();
    keep("perseid-push-tag-connect.yml", push);
    assert!(push.contains("\n    tags: [\"api-v*\"]\n"), "{push}");
    assert!(
        push.contains("make openapi.yaml") && push.contains("private: true"),
        "the other settings are kept: {push}"
    );
}

#[test]
fn connect_declined_writes_the_workflow_only_and_says_how_to_add_the_token() {
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
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "n\n", &TOKEN);
    assert_eq!(code, 0, "{out}");
    assert_eq!(server.lock().unwrap().writes(before), Vec::<&String>::new());
    for line in [
        "? Store the credentials on acme/petstore? [Y/n]",
        "Nothing changed on GitHub. To do it yourself:",
        "create a fine-grained token with Contents read and write on acme/petstore-sdks at https://github.com/settings/personal-access-tokens/new, then `gh secret set SDK_GITHUB_TOKEN -R acme/petstore`",
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
fn connect_with_the_app_needs_it_set_up_in_the_sdks_repository() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    set_up_sdks_repository(&server, port, &["init", "--sdks", "go"]);
    let (dir, bin) = api_checkout(true);
    let connect = ["connect", "acme/petstore-sdks", "--auth", "app", "--yes"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &connect, "", &TOKEN);
    assert_ne!(code, 0);
    assert!(
        out.contains("acme/petstore-sdks has no GitHub App yet: run `perseid app` in a clone of acme/petstore-sdks first, or pass --auth token"),
        "{out}"
    );
}

#[test]
fn connect_warns_until_the_sdks_repository_is_set_up_and_needs_admin_rights_here() {
    let mut fake = GitHub::with_spec_repo();
    fake.add_repo("acme/petstore-sdks", &[("README.md", "# SDKs\n")]);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    let (dir, bin) = api_checkout(true);
    let dry = ["connect", "acme/petstore-sdks", "--dry-run"];
    let (code, out) = perseid(dir.path(), bin.path(), port, &dry, "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    for line in [
        "acme/petstore ──spec──▶ acme/petstore-sdks\n",
        "! acme/petstore-sdks has no perseid.toml on `main` yet: pushes skip until it does. Run `npx perseid init` there",
        "! acme/petstore-sdks doesn't regenerate its SDKs yet",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let connect = ["connect", "acme/petstore-sdks", "--yes", "--auth", "token"];
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
        out.contains("octo isn't an admin of acme/petstore, which stores the credentials")
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
fn init_completes_an_existing_sdks_repository_receiving_the_spec() {
    let mut fake = GitHub::with_spec_repo();
    let (dir, bin) = sdks_checkout(&mut fake);
    let server = Arc::new(Mutex::new(fake));
    let port = serve(server.clone());
    edit(dir.path(), |t| {
        t.replace("https://api.example.com/openapi.yaml", "openapi.yaml")
    });
    fs::write(dir.path().join("openapi.yaml"), SPEC).unwrap();

    let (code, out) = perseid(dir.path(), bin.path(), port, &["init"], "", &[]);
    assert_eq!(code, 0, "{out}");
    for line in [
        "+ .github/workflows/sdks.yml",
        "~ release-please-config.json",
        "+ .release-please-manifest.json",
        "! .github/workflows/sdk-release.yml is yours: check it runs release-please on `main`",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let release: Value = serde_json::from_str(&read("release-please-config.json")).unwrap();
    assert_eq!(
        release["packages"]["docs"]["release-type"], "simple",
        "kept"
    );
    assert_eq!(release["packages"]["ts"]["release-type"], "node", "added");
    assert_eq!(
        read(".github/workflows/sdk-release.yml"),
        "name: Releases\n"
    );
    assert!(
        read("perseid.toml").contains("[typescript]\npath = \"ts\"\n"),
        "user settings kept"
    );

    let (code, out) = perseid(dir.path(), bin.path(), port, &["init"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("the workflows match perseid.toml"), "{out}");

    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    for line in [
        "acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (ts/)",
        "! acme/petstore-sdks: perseid would overwrite files it didn't generate, and stops instead: ts/src/index.ts",
        "! .github/workflows/sdk-release.yml is yours: check it runs release-please on `main`",
        "! .github/workflows/sdks.yml is not on `main` of acme/petstore-sdks yet: commit and push",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
    assert_eq!(
        read("ts/src/index.ts"),
        "export const mine = 1;\n",
        "never overwritten"
    );

    edit(dir.path(), |t| format!("release = false\n{t}"));
    let (code, out) = perseid(dir.path(), bin.path(), port, &["init"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        !out.contains("sdk-release.yml") && !out.contains(".release-please-manifest.json"),
        "{out}"
    );
}

#[test]
fn init_polls_a_url_spec_daily() {
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
    let workflow = fs::read_to_string(dir.path().join(".github/workflows/sdks.yml")).unwrap();
    keep("url-sdks.yml", &workflow);
    assert!(
        workflow.contains("paths: [\"perseid.toml\"]\n  schedule:\n    - cron: '"),
        "{workflow}"
    );
    let (code, out) = perseid(dir.path(), bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains(&format!(
            "{url} ──spec──▶ acme/petstore-sdks ──PRs──▶ acme/petstore-sdks (go/)"
        )),
        "{out}"
    );
}

#[test]
fn init_writes_the_release_files_at_the_root_of_a_repository_holding_perseid_toml_in_a_folder() {
    let config = "spec = \"openapi.yaml\"\nname = \"Petstore\"\nsdks = [\"typescript\", \"go\"]\n";
    let outdated = "# Written by `perseid init`, yours to edit.\nname: SDK Release\n";
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
    git(dir.path(), &["branch", "-m", "trunk"]);
    let api = dir.path().join("api");
    fs::create_dir_all(&api).unwrap();
    fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
    fs::write(api.join("openapi.yaml"), SPEC).unwrap();
    fs::write(api.join("perseid.toml"), config).unwrap();
    fs::write(
        dir.path().join(".github/workflows/sdk-release.yml"),
        outdated,
    )
    .unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "--quiet", "-m", "spec"]);

    let (code, out) = perseid(&api, bin.path(), port, &["init"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("~ .github/workflows/sdk-release.yml"),
        "perseid wrote it, so it is rewritten: {out}"
    );
    assert!(!api.join(".github").exists() && !api.join("release-please-config.json").exists());
    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let release: Value = serde_json::from_str(&read("release-please-config.json")).unwrap();
    let packages: Vec<&String> = release["packages"].as_object().unwrap().keys().collect();
    assert_eq!(packages, ["api/typescript", "api/go"]);
    assert_eq!(release["packages"]["api/go"]["component"], "api/go");
    let workflow = read(".github/workflows/sdk-release.yml");
    keep("subfolder-sdk-release.yml", &workflow);
    assert!(workflow.contains("branches: [\"trunk\"]\n"), "{workflow}");
    assert!(
        read(".github/workflows/sdks.yml").contains("working-directory: api\n"),
        "sdks.yml runs perseid in api/"
    );

    let (code, out) = perseid(&api, bin.path(), port, &["status"], "", &TOKEN);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("are not on `trunk` of acme/petstore yet: commit and push"),
        "{out}"
    );
}
