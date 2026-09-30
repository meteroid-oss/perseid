//! `perseid init --github` against a fake GitHub serving the REST API, OAuth and App pages.

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

#[derive(Default)]
struct Repo {
    private: bool,
    refs: BTreeMap<String, String>,
    variables: BTreeMap<String, String>,
    /// Secrets as decrypted with the repository key.
    secrets: BTreeMap<String, String>,
    key: Option<SecretKey>,
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
}

fn digest(value: impl Hash) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl GitHub {
    fn with_spec_repo() -> Self {
        let mut github = Self::default();
        let tree = BTreeMap::from([("README.md".to_owned(), github.blob(b"# Petstore\n"))]);
        let commit = github.commit(tree, "init");
        let repo = Repo {
            private: true,
            refs: BTreeMap::from([("main".to_owned(), commit)]),
            ..Repo::default()
        };
        github.repos.insert("acme/petstore".into(), repo);
        github
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

    fn handle(
        &mut self,
        method: &str,
        target: &str,
        auth: Option<&str>,
        body: &Value,
    ) -> (u16, Value) {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        self.calls.push(format!("{method} {path}"));
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (method, segments.as_slice()) {
            ("POST", ["web", "login", "device", "code"]) => (
                200,
                json!({
                    "device_code": "device-1", "user_code": "ABCD-1234",
                    "verification_uri": format!("{}/web/login/device", "http://github.test"),
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
                        json!({
                            "access_token": "user-token", "token_type": "bearer", "scope": "repo,workflow",
                            "expires_in": 28800, "refresh_token": "refresh", "refresh_token_expires_in": 15897600,
                        }),
                    ),
                }
            }
            (_, ["api", ..]) if auth.is_none() && segments[1] != "app-manifests" => {
                (401, json!({ "message": "Requires authentication" }))
            }
            ("GET", ["api", "user"]) => (200, json!({ "login": "octo" })),
            ("GET", ["api", "users", login]) => {
                (200, json!({ "login": login, "type": "Organization" }))
            }
            ("POST", ["api", "orgs", org, "repos"]) => {
                let name = format!("{org}/{}", body["name"].as_str().unwrap());
                let repo = Repo {
                    private: body["private"] == true,
                    ..Repo::default()
                };
                assert_eq!(body["visibility"], "private");
                self.repos.insert(name.clone(), repo);
                (201, json!({ "full_name": name, "default_branch": "main" }))
            }
            ("POST", ["api", "app-manifests", code, "conversions"]) => {
                assert_eq!(*code, "manifest-code");
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
                self.repo(method, &repo, rest, query, auth.unwrap(), body)
            }
            _ => (
                404,
                json!({ "message": format!("fake GitHub has no {method} {path}") }),
            ),
        }
    }

    fn repo(
        &mut self,
        method: &str,
        name: &str,
        rest: &[&str],
        query: &str,
        auth: &str,
        body: &Value,
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
                    json!({ "full_name": name, "private": repo.private, "visibility": visibility, "default_branch": "main" }),
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
                    None => (404, json!({ "message": "This repository is empty." })),
                    Some(_) => match self.files(name, &branch).get(&path.join("/")) {
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
    let (mut length, mut auth, mut form) = (0, None, false);
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
            _ => {}
        }
    }
    let mut raw = vec![0; length];
    reader.read_exact(&mut raw).unwrap();
    let body = match form {
        true => Value::Object(
            String::from_utf8(raw)
                .unwrap()
                .split('&')
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_owned(), Value::from(v)))
                .collect(),
        ),
        false => serde_json::from_slice(&raw).unwrap_or(Value::Null),
    };
    let (status, reply) = github
        .lock()
        .unwrap()
        .handle(&method, &target, auth.as_deref(), &body);
    let text = if reply.is_null() {
        String::new()
    } else {
        reply.to_string()
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

/// A spec repository checkout whose origin is acme/petstore, and a PATH offering git but not gh.
fn project() -> (tempfile::TempDir, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    git(dir.path(), &["init", "--quiet", "--initial-branch", "main"]);
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/petstore.git",
        ],
    );
    let bin = tempfile::tempdir().unwrap();
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(real.trim(), bin.path().join("git")).unwrap();
    (dir, bin)
}

/// Runs perseid with `answers` on stdin, acting as the browser when it asks to create the App.
fn perseid(
    dir: &Path,
    bin: &Path,
    port: u16,
    args: &[&str],
    answers: &str,
    env: &[(&str, &str)],
) -> (bool, String) {
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
    let ok = child.wait().unwrap().success();
    (ok, out + &stderr)
}

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
    assert_eq!(manifest["url"], "https://github.com/acme/petstore");
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

#[test]
fn init_github_sets_up_repositories_app_secrets_and_workflow() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let (dir, bin) = project();
    let args = [
        "init",
        "typescript",
        "python",
        "go",
        "--github",
        "--no-browser",
    ];
    let (ok, out) = perseid(dir.path(), bin.path(), port, &args, "y\n\n", &[]);
    eprintln!("{out}");
    assert!(ok, "{out}");
    assert!(out.contains("and enter the code ABCD-1234"), "{out}");
    assert!(
        out.contains("✓ Signed in to GitHub as octo (browser)"),
        "{out}"
    );

    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(
        config.contains("[typescript]\nrepo = \"acme/petstore-node\""),
        "{config}"
    );
    assert!(
        config.contains(
            "[go]\nrepo = \"acme/petstore-go\"\nmodule = \"github.com/acme/petstore-go\""
        ),
        "{config}"
    );
    assert!(
        !dir.path().join("typescript").exists(),
        "SDKs in their own repositories aren't scaffolded here"
    );

    let github = server.lock().unwrap();
    assert_eq!(github.token_polls, 3);
    let sdk_repos = [
        "acme/petstore-go",
        "acme/petstore-node",
        "acme/petstore-python",
    ];
    for repo in sdk_repos {
        assert!(
            github.repos[repo].private,
            "{repo} matches the spec repository"
        );
        let files = github.files(repo, "main");
        assert!(
            files.contains_key(".github/workflows/sdk-release.yml"),
            "{repo}: {files:?}"
        );
        let config: Value = serde_json::from_str(&files["release-please-config.json"]).unwrap();
        assert_eq!(config["packages"]["."]["include-component-in-tag"], false);
    }
    let node: Value = serde_json::from_str(
        &github.files("acme/petstore-node", "main")["release-please-config.json"],
    )
    .unwrap();
    assert_eq!(node["packages"]["."]["release-type"], "node");

    for repo in ["acme/petstore"].iter().chain(&sdk_repos) {
        let repo = &github.repos[*repo];
        assert_eq!(repo.variables["SDK_APP_ID"], "42");
        assert_eq!(
            repo.secrets["SDK_APP_PRIVATE_KEY"], PEM,
            "decrypted with the repository key"
        );
    }
    assert_eq!(
        github.repos["acme/petstore"].variables["SDK_APP_SLUG"],
        "petstore-sdk-bot"
    );
    assert!(github.installation_checks > 4, "polled until installed");

    let files = github.files("acme/petstore", "perseid/setup");
    let workflow = &files[".github/workflows/sdks.yml"];
    fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("sdks.yml"),
        workflow,
    )
    .unwrap();
    assert!(
        workflow.contains("paths: [\"openapi.yaml\",\"perseid.toml\"]"),
        "{workflow}"
    );
    assert!(
        workflow.contains("repositories: petstore,petstore-go,petstore-node,petstore-python\n"),
        "{workflow}"
    );
    assert_eq!(files["perseid.toml"], config);
    assert!(files.contains_key("openapi.yaml") && files.contains_key("README.md"));
    assert_eq!(github.pulls.len(), 1);
    assert_eq!(github.pulls[0]["base"], "main");
    assert!(
        out.contains(
            "✓ Opened https://github.com/acme/petstore/pull/1 with sdks.yml and 2 more files"
        ),
        "{out}"
    );
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "openapi.yaml\nperseid.toml\n");
    assert!(
        out.contains("PyPI: add a pending publisher for petstore"),
        "{out}"
    );
    drop(github);

    let before = server.lock().unwrap().calls.len();
    let args = ["init", "--github", "--no-browser", "--yes"];
    let (ok, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &args,
        "",
        &[("GH_TOKEN", "user-token")],
    );
    eprintln!("{out}");
    assert!(ok, "{out}");
    assert!(
        out.contains("✓ Signed in to GitHub as octo (GH_TOKEN)"),
        "{out}"
    );
    let github = server.lock().unwrap();
    let writes: Vec<_> = github.calls[before..]
        .iter()
        .filter(|c: &&String| {
            !c.starts_with("GET") && !c.contains("/git/blobs") && !c.contains("/git/trees")
        })
        .collect();
    assert!(
        writes.is_empty(),
        "a second run changes nothing: {writes:?}"
    );
    assert!(
        out.contains("✓ https://github.com/acme/petstore/pull/1 holds sdks.yml"),
        "{out}"
    );
}

#[test]
fn init_github_without_credentials_says_how_to_get_them() {
    let github = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(github);
    let (dir, bin) = project();
    let args = ["init", "rust", "--github", "--no-browser", "--yes"];
    let (ok, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &args,
        "",
        &[("PERSEID_GITHUB_CLIENT_ID", "")],
    );
    assert!(!ok);
    assert!(
        out.contains("set GH_TOKEN") && out.contains("gh auth login"),
        "{out}"
    );
    assert!(
        dir.path().join("rust/Cargo.toml").exists(),
        "--yes keeps SDKs in this repository"
    );
}

#[test]
fn init_github_push_commits_sdks_kept_in_the_spec_repository() {
    let server = Arc::new(Mutex::new(GitHub::with_spec_repo()));
    let port = serve(server.clone());
    let (dir, bin) = project();
    let args = [
        "init",
        "rust",
        "--github",
        "--no-browser",
        "--yes",
        "--push",
    ];
    let (ok, out) = perseid(
        dir.path(),
        bin.path(),
        port,
        &args,
        "",
        &[("GH_TOKEN", "user-token")],
    );
    assert!(ok, "{out}");
    let github = server.lock().unwrap();
    assert_eq!(github.repos.len(), 1, "no SDK repository to create");
    assert!(github.pulls.is_empty());
    let files = github.files("acme/petstore", "main");
    let workflow = &files[".github/workflows/sdks.yml"];
    assert!(workflow.contains("repositories: petstore\n"), "{workflow}");
    for path in [
        "rust/Cargo.toml",
        "release-please-config.json",
        ".github/workflows/sdk-release.yml",
    ] {
        assert!(
            files.contains_key(path),
            "{path} is committed with the setup: {files:?}"
        );
    }
    assert_eq!(
        github.repos["acme/petstore"].secrets["SDK_APP_PRIVATE_KEY"],
        PEM
    );
    assert!(out.contains("✓ Pushed sdks.yml and"), "{out}");
}
