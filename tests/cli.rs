use std::{fs, path::Path, process::Command};

fn perseid(dir: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

fn project() -> tempfile::TempDir {
    project_from("petstore.yaml", &["rust", "go"])
}

fn project_from(fixture: &str, languages: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        Path::new("tests/fixtures").join(fixture),
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", &languages.join(",")]);
    assert!(ok, "{out}");
    let tables: String = languages.iter().map(|l| format!("\n[{l}]\n")).collect();
    let config = dir.path().join("perseid.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text + &tables).unwrap();
    dir
}

#[test]
fn init_derives_names_from_the_spec() {
    let dir = project();
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("name = \"Petstore\""), "{config}");
    assert!(
        config
            .contains("spec = \"openapi.yaml\"\nname = \"Petstore\"\nsdks = [\"rust\", \"go\"]\n"),
        "{config}"
    );
    assert!(
        config.contains("base_url = \"https://petstore.example.com\""),
        "{config}"
    );
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(dir.path().join("rust/src/error.rs").exists());
    assert!(dir.path().join("go/go.mod").exists());
}

#[test]
fn init_writes_perseid_toml_and_the_workflows_and_asks_for_the_sdks() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("spec/api/v1");
    fs::create_dir_all(&nested).unwrap();
    fs::copy("tests/fixtures/petstore.yaml", nested.join("openapi.yaml")).unwrap();
    fs::write(dir.path().join("notes.yaml"), "openapi: not a spec name\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["init"]);
    assert!(!ok && out.contains("pass --sdks"), "{out}");
    let (ok, out) = perseid(
        dir.path(),
        &[
            "init",
            "--sdks",
            "go,typescript",
            "--repo",
            "acme/petstore-{lang}",
        ],
    );
    assert!(ok, "{out}");
    assert!(
        out.contains("acme/petstore-typescript, acme/petstore-go"),
        "{out}"
    );
    let mut entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, [".github", "notes.yaml", "perseid.toml", "spec"]);
    assert!(
        out.contains("+ .github/workflows/sdks.yml")
            && out.contains("Packages: npm petstore, Go github.com/acme/petstore-go"),
        "{out}"
    );
    assert!(!dir.path().join("release-please-config.json").exists());
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(
        config.contains("spec = \"spec/api/v1/openapi.yaml\"\nname = \"Petstore\"\nsdks = [\"typescript\", \"go\"]\nrepo = \"acme/petstore-{lang}\"\n"),
        "{config}"
    );
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "go"]);
    assert!(!ok && out.contains("perseid.toml already exists"), "{out}");
    let (ok, out) = perseid(dir.path(), &["init"]);
    assert!(
        ok && out.contains("the workflows match perseid.toml"),
        "{out}"
    );
    let workflow = dir.path().join(".github/workflows/sdks.yml");
    fs::write(&workflow, "# Written by `perseid init`: stale\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["init"]);
    assert!(ok && out.contains("~ .github/workflows/sdks.yml"), "{out}");
    fs::write(&workflow, "name: mine\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["init"]);
    assert!(
        ok && out.contains("! .github/workflows/sdks.yml is yours"),
        "{out}"
    );
    assert_eq!(fs::read_to_string(&workflow).unwrap(), "name: mine\n");
}

#[test]
fn without_a_spec_init_points_to_connect_and_generate_previews() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "go", "--name", "Petstore"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("perseid connect") && out.contains("--spec"),
        "{out}"
    );
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("spec = \"openapi.json\"\n"), "{config}");
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(
        !ok && out.contains("openapi.json doesn't exist yet"),
        "{out}"
    );
    assert!(
        out.contains("--spec") && out.contains("perseid connect"),
        "{out}"
    );
    let spec = std::path::absolute("tests/fixtures/petstore.yaml").unwrap();
    let args = ["generate", "--spec", spec.to_str().unwrap(), "--no-format"];
    let (ok, out) = perseid(dir.path(), &args);
    assert!(ok, "{out}");
    assert!(dir.path().join("go/go.mod").exists());
    assert!(dir.path().join("go/client.go").exists());
}

#[test]
fn generate_is_idempotent_and_check_detects_drift() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let models = dir.path().join("rust/src/models");
    assert!(models.join("pet.rs").exists());
    assert!(dir.path().join("rust/src/request.rs").exists());

    let (ok, out) = perseid(dir.path(), &["generate", "--check", "--no-format"]);
    assert!(ok, "{out}");
    assert!(out.contains("rust (rust): up to date"), "{out}");

    fs::write(models.join("stale.rs"), "// this file is @generated\n").unwrap();
    fs::write(models.join("mine.rs"), "// handwritten\n").unwrap();
    fs::write(models.join("pet.rs"), "// this file is @generated\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--check", "--no-format"]);
    assert!(!ok);
    assert!(
        out.contains("- src/models/stale.rs") && out.contains("~ src/models/pet.rs"),
        "{out}"
    );
    assert!(models.join("stale.rs").exists(), "--check must not write");

    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!models.join("stale.rs").exists());
    assert!(models.join("mine.rs").exists());
}

#[test]
fn ejected_templates_override_the_built_ins() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["eject", "go"]);
    assert!(ok, "{out}");
    let template = dir
        .path()
        .join(".perseid/templates/go/api_summary.go.jinja");
    let source = fs::read_to_string(&template).unwrap();
    fs::write(
        &template,
        source.replacen("package ", "// customized\npackage ", 1),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(ok, "{out}");
    let client = fs::read_to_string(dir.path().join("go/client.go")).unwrap();
    assert!(client.contains("// customized"), "{client}");
}

#[test]
fn stale_generated_files_are_removed_from_directories_without_output() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let marker = "// this file is @generated\n";
    let write = |path: &str, content: &str| {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write("rust/src/retired/old.rs", marker);
    write("rust/src/retired/mine.rs", "// handwritten\n");
    write("rust/target/debug/build.rs", marker);
    write("go/retired/mine.go", marker);
    write(
        "go/retired/old.go",
        "// Code generated by perseid. DO NOT EDIT.\n",
    );
    let client = fs::read_to_string(dir.path().join("go/client.go")).unwrap();
    assert!(client.starts_with("// Code generated by perseid. DO NOT EDIT.\n"));

    let (ok, out) = perseid(dir.path(), &["generate", "--check", "--no-format"]);
    assert!(!ok);
    assert!(out.contains("- src/retired/old.rs"), "{out}");
    assert!(out.contains("- retired/old.go"), "{out}");
    assert!(!out.contains("target"), "{out}");

    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!dir.path().join("rust/src/retired/old.rs").exists());
    assert!(!dir.path().join("go/retired/old.go").exists());
    assert!(dir.path().join("go/retired/mine.go").exists());
    assert!(dir.path().join("rust/src/retired/mine.rs").exists());
    assert!(dir.path().join("rust/target/debug/build.rs").exists());
}

#[test]
fn extension_snippets_are_included_in_resource_classes() {
    let dir = project();
    let extensions = dir.path().join(".perseid/templates/rust/extensions");
    fs::create_dir_all(&extensions).unwrap();
    fs::write(
        extensions.join("pets.rs"),
        "pub fn custom_method(&self) -> usize {\n    42\n}\n",
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok, "{out}");
    let pets = fs::read_to_string(dir.path().join("rust/src/api/pets.rs")).unwrap();
    assert!(pets.contains("pub fn custom_method(&self)"), "{pets}");
}

#[test]
fn webhooks_verifier_is_opt_in() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!dir.path().join("rust/src/webhooks.rs").exists());
    assert!(!dir.path().join("go/webhooks.go").exists());

    let config = dir.path().join("perseid.toml");
    let toml = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        toml.replacen("\n[rust]", "webhooks = true\n\n[rust]", 1),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let go = fs::read_to_string(dir.path().join("go/webhooks.go")).unwrap();
    assert!(
        go.contains("package petstore") && go.contains("func NewWebhook"),
        "{go}"
    );
    assert!(dir.path().join("rust/src/webhooks.rs").exists());

    let toml = fs::read_to_string(&config)
        .unwrap()
        .replacen("[go]", "[go]\nwebhooks = false", 1);
    fs::write(&config, toml).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        !dir.path().join("go/webhooks.go").exists(),
        "target setting wins, stale file removed"
    );
    assert!(dir.path().join("rust/src/webhooks.rs").exists());
}

#[test]
fn handwritten_files_are_never_overwritten() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let path = dir.path().join("rust/src/request.rs");
    fs::write(&path, "// handwritten\n").unwrap();

    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(!ok);
    assert!(out.contains("src/request.rs"), "{out}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "// handwritten\n");
}

fn files(dir: &Path) -> Vec<(String, String)> {
    let mut all = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = fs::read_to_string(&path) {
                let name = path.strip_prefix(dir).unwrap().display().to_string();
                all.push((name, text));
            }
        }
    }
    all.sort();
    all
}

#[test]
fn openapi_3_0_generates_the_same_sdks_as_the_3_1_equivalent() {
    let langs = ["rust", "typescript", "python", "go", "java", "csharp"];
    let old = project_from("petstore-30.yaml", &langs);
    let new = project_from("petstore-nullable-31.yaml", &langs);
    for dir in [&old, &new] {
        let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
        assert!(ok, "{out}");
    }
    let pet = fs::read_to_string(old.path().join("rust/src/models/pet.rs")).unwrap();
    let nickname = pet.split("pub nickname").nth(1).unwrap();
    assert!(
        nickname
            .trim_start_matches([' ', '\n'])
            .starts_with(": Option<String>"),
        "{pet}"
    );
    for lang in langs {
        assert_eq!(
            files(&old.path().join(lang)),
            files(&new.path().join(lang)),
            "{lang}"
        );
    }
}

#[test]
fn swagger_2_is_rejected_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("openapi.yaml"), "swagger: '2.0'\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "rust"]);
    assert!(!ok);
    assert!(out.contains("swagger2openapi"), "{out}");
}

#[test]
fn csharp_init_and_generate_lay_out_a_dotnet_project() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "csharp"]);
    assert!(ok, "{out}");
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let sdk = dir.path().join("csharp");
    let project = fs::read_to_string(sdk.join("Petstore/Petstore.csproj")).unwrap();
    assert!(project.contains("<Version>0.1.0</Version>"), "{project}");
    assert!(sdk.join("Petstore.sln").exists());
    for file in [
        "PetstoreClient.cs",
        "Api/PetsApi.cs",
        "Api/PetsListOptions.cs",
        "Models/Pet.cs",
        "Models/PetstoreJsonContext.cs",
        "ApiTransport.cs",
    ] {
        assert!(sdk.join("Petstore").join(file).exists(), "{file}: {out}");
    }
    let api = fs::read_to_string(sdk.join("Petstore/Api/PetsApi.cs")).unwrap();
    assert!(api.contains("public Task<Pet> RetrieveAsync("), "{api}");
    assert!(api.contains("public Task<Pet> CreateAsync("), "{api}");
}

#[test]
fn csharp_generates_streaming_auth_and_pagination() {
    let dir = project_from("features.yaml", &["csharp"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("csharp/Features").join(path)).unwrap();
    let streaming = read("Api/StreamingApi.cs");
    assert!(
        streaming.contains("public Task<EventStream> RetrieveEventsStreamAsync("),
        "{streaming}"
    );
    assert!(
        streaming.contains("public sealed class StreamingCreateFileBody"),
        "{streaming}"
    );
    assert!(streaming.contains("Upload body,"), "{streaming}");
    let widgets = read("Api/WidgetsApi.cs");
    assert!(
        widgets.contains("public IAsyncEnumerable<Widget> ListIterAsync("),
        "{widgets}"
    );
    let client = read("FeaturesClient.cs");
    assert!(
        client.contains("public FeaturesApiKeys ApiKeys"),
        "{client}"
    );
    assert!(read("Api/RecordsApi.cs").contains(r#"request.Security = [["api_key_query"]];"#));
}

#[test]
fn csharp_extension_snippets_are_included_in_resource_classes() {
    let dir = project_from("petstore.yaml", &["csharp"]);
    let extensions = dir.path().join(".perseid/templates/csharp/extensions");
    fs::create_dir_all(&extensions).unwrap();
    fs::write(
        extensions.join("pets.cs"),
        "public int CustomMethod() => 42;\n",
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let pets = fs::read_to_string(dir.path().join("csharp/Petstore/Api/PetsApi.cs")).unwrap();
    let class = pets.find("public sealed partial class PetsApi").unwrap();
    let method = pets.find("public int CustomMethod() => 42;").unwrap();
    assert!(
        class < method && method < pets.rfind('}').unwrap(),
        "{pets}"
    );
}

#[test]
fn bump_needs_a_pull_request() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--bump", "major"]);
    assert!(!ok && out.contains("--pr"), "{out}");
}

fn git_in(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A project committed and pushed to a local `origin`, with a fake `gh` answering `pr list`
/// with `listed` and logging its calls to `gh.log`.
#[cfg(unix)]
fn pull_request_project(listed: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    let dir = project();
    let git = |args: &[&str]| git_in(dir.path(), args);
    git(&["init", "--quiet", "--initial-branch", "main"]);
    git(&["init", "--quiet", "--bare", "origin.git"]);
    git(&["remote", "add", "origin", "origin.git"]);
    fs::write(dir.path().join(".gitignore"), "origin.git\nbin\nhome\n").unwrap();
    git(&["add", "--all"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    git(&["push", "--quiet", "origin", "main"]);

    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"$GH_LOG\"\ncase \"$2\" in\n  list) printf '{listed}' ;;\n  create) echo https://pr/2 ;;\nesac\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(dir.path().join("home")).unwrap();
    dir
}

/// Runs `generate rust --pr` with the fake `gh` and no git identity configured.
#[cfg(unix)]
fn generate_pr(dir: &Path, bump: &str) -> String {
    generate_pr_with(dir, &["--bump", bump])
}

#[cfg(unix)]
fn generate_pr_with(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["generate", "rust", "--pr", "--no-format"])
        .args(args)
        .current_dir(dir)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("GH_LOG", dir.join("gh.log"))
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("GITHUB_EVENT_BEFORE")
        .env_remove("GITHUB_ACTIONS")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    fs::read_to_string(dir.join("gh.log")).unwrap()
}

#[cfg(unix)]
fn executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn auto_bumps_are_sized_by_oasdiff_against_the_previous_commit() {
    let dir = pull_request_project("");
    let spec = dir.path().join("openapi.yaml");
    let changed = fs::read_to_string(&spec)
        .unwrap()
        .replace("title:", "description: Pets\n  title:");
    fs::write(&spec, changed).unwrap();
    let commit = [
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qam",
    ];
    git_in(dir.path(), &[&commit[..], &["spec"]].concat());
    executable(
        &dir.path().join("bin/oasdiff"),
        "#!/bin/sh\necho \"$@\" >> \"$GH_LOG.oasdiff\"\ncase \"$*\" in\n  breaking*) exit 1 ;;\n  *markdown*) echo '- removed GET /pets/{id}' ;;\nesac\n",
    );

    let calls = generate_pr_with(dir.path(), &[]);
    assert!(
        calls.contains("pr create --head perseid/update --title feat(api)!: update SDKs to"),
        "{calls}"
    );
    assert!(
        calls.contains("### API changes\n\n- removed GET /pets/{id}"),
        "{calls}"
    );
    let runs = fs::read_to_string(dir.path().join("gh.log.oasdiff")).unwrap();
    let first = runs.lines().next().unwrap();
    assert!(
        first.starts_with("breaking --fail-on ERR --severity-levels ")
            && first.ends_with("/openapi.yaml"),
        "{runs}"
    );
}

#[cfg(unix)]
#[test]
fn auto_bumps_without_a_previous_spec_are_minor() {
    let dir = pull_request_project("");
    let calls = generate_pr_with(dir.path(), &[]);
    assert!(
        calls.contains("pr create --head perseid/update --title feat(api): update SDKs to"),
        "{calls}"
    );
    assert!(!calls.contains("API changes"), "{calls}");
}

#[cfg(unix)]
#[test]
fn pull_requests_can_auto_merge_and_dispatch_ci() {
    let dir = pull_request_project("");
    let calls = generate_pr_with(
        dir.path(),
        &[
            "--bump",
            "patch",
            "--auto-merge",
            "--dispatch",
            "ci.yml lint.yml",
        ],
    );
    for call in [
        "label create perseid:auto-release --force --color 6f42c1 --description Auto-merge the release PR this change leads to\n",
        "pr edit https://pr/2 --add-label perseid:auto-release\n",
        "pr merge https://pr/2 --auto --squash\n",
        "workflow run ci.yml --ref perseid/update\n",
        "workflow run lint.yml --ref perseid/update\n",
    ] {
        assert!(calls.contains(call), "{call} not in {calls}");
    }
}

#[test]
fn tools_list_what_the_sdks_need_and_the_app_token_scope() {
    let dir = project_from("petstore.yaml", &["go", "python", "typescript"]);
    edit_config(dir.path(), |config| {
        config.replace("[python]\n", "[python]\nrepo = \"acme/petstore-python\"\n")
    });
    let output_file = dir.path().join("github-output");
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["tools", "list", "--github-output"])
        .current_dir(dir.path())
        .env("GITHUB_OUTPUT", &output_file)
        .env("GITHUB_REPOSITORY", "acme/petstore")
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{listed}");
    let names: Vec<_> = listed
        .lines()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert_eq!(names, ["biome", "ruff", "oasdiff"], "{listed}");
    assert_eq!(
        fs::read_to_string(output_file).unwrap(),
        "languages=typescript python go\nowner=acme\nrepositories=petstore,petstore-python\n"
    );
}

/// Serves `files` by path over HTTP, as GitHub release downloads, until the test ends.
#[cfg(unix)]
fn serve(files: Vec<(String, Vec<u8>)>) -> u16 {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            let path = request.split(' ').nth(1).unwrap_or_default();
            let (status, body) = match files.iter().find(|(p, _)| p == path) {
                Some((_, body)) => ("200 OK", body.as_slice()),
                None => ("404 Not Found", &b""[..]),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    port
}

#[cfg(unix)]
#[test]
fn tools_install_downloads_the_pinned_tools_and_checks_they_run() {
    let dir = project_from("petstore.yaml", &["typescript"]);
    let (ok, listed) = perseid(dir.path(), &["tools", "list"]);
    assert!(ok, "{listed}");
    let version = |tool: &str| {
        let line = listed.lines().find(|l| l.starts_with(tool)).unwrap();
        line.split(' ').nth(1).unwrap().to_owned()
    };
    let (biome, oasdiff) = (version("biome"), version("oasdiff"));
    let release = dir.path().join("release");
    fs::create_dir(&release).unwrap();
    executable(
        &release.join("oasdiff"),
        &format!("#!/bin/sh\necho oasdiff version {oasdiff}\n"),
    );
    let archive = release.join("oasdiff.tar.gz");
    let tar = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&release)
        .arg("oasdiff")
        .status()
        .unwrap();
    assert!(tar.success());
    let (os, arch, platform) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "x86_64") => ("darwin", "x64", "darwin_all"),
        ("macos", _) => ("darwin", "arm64", "darwin_all"),
        (_, "x86_64") => ("linux", "x64", "linux_amd64"),
        _ => ("linux", "arm64", "linux_arm64"),
    };
    let port = serve(vec![
        (
            format!(
                "/biomejs/biome/releases/download/%40biomejs%2Fbiome%40{biome}/biome-{os}-{arch}"
            ),
            format!("#!/bin/sh\necho Version: {biome}\n").into_bytes(),
        ),
        (
            format!(
                "/oasdiff/oasdiff/releases/download/v{oasdiff}/oasdiff_{oasdiff}_{platform}.tar.gz"
            ),
            fs::read(&archive).unwrap(),
        ),
    ]);
    let install = || {
        let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
            .args(["tools", "install", "--dir", "tools"])
            .current_dir(dir.path())
            .env("PERSEID_GITHUB_WEB", format!("http://127.0.0.1:{port}"))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{text}");
        text
    };
    let text = install();
    assert!(text.contains(&format!("✓ biome {biome}\n")), "{text}");
    let ran = Command::new(dir.path().join("tools/oasdiff"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        format!("oasdiff version {oasdiff}\n")
    );
    let text = install();
    assert!(
        text.contains(&format!("✓ oasdiff {oasdiff} (already installed)")),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn open_pull_requests_keep_their_largest_bump() {
    let dir = pull_request_project("https://pr/1\\tfeat(api)!: update SDKs to Petstore 1\\n");
    let calls = generate_pr(dir.path(), "patch");
    assert!(
        calls.contains("pr edit perseid/update --title feat(api)!: update SDKs to"),
        "{calls}"
    );
}

#[cfg(unix)]
#[test]
fn pull_requests_leave_the_checkout_alone() {
    let dir = pull_request_project("");
    let git = |args: &[&str]| git_in(dir.path(), args);
    let origin = |args: &[&str]| git_in(&dir.path().join("origin.git"), args);
    fs::write(dir.path().join("unpushed.txt"), "local").unwrap();
    git(&["add", "unpushed.txt"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "unpushed",
    ]);
    fs::create_dir_all(dir.path().join("rust")).unwrap();
    fs::write(dir.path().join("rust/NOTES.md"), "draft").unwrap();
    fs::write(dir.path().join("scratch.txt"), "draft").unwrap();

    let calls = generate_pr(dir.path(), "minor");
    assert!(
        calls.contains("pr create --head perseid/update --title feat(api): update SDKs to"),
        "{calls}"
    );
    assert!(calls.contains("--base main"), "{calls}");

    assert_eq!(git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git(&["log", "-1", "--format=%s"]), "unpushed");
    let status = git(&["status", "--porcelain"]);
    assert!(status.contains("?? rust/"), "{status}");
    assert!(status.contains("?? scratch.txt"), "{status}");
    assert!(!status.lines().any(|l| !l.starts_with("??")), "{status}");
    assert_eq!(git(&["worktree", "list"]).lines().count(), 1);

    assert_eq!(
        origin(&["rev-parse", "perseid/update^"]),
        origin(&["rev-parse", "main"])
    );
    assert_eq!(
        origin(&["log", "-1", "--format=%an <%ae>", "perseid/update"]),
        "github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com>"
    );
    let files = origin(&["ls-tree", "-r", "--name-only", "perseid/update"]);
    assert!(files.contains("rust/Cargo.toml"), "{files}");
    assert!(files.contains("rust/src/lib.rs"), "{files}");
    for local in ["unpushed.txt", "rust/NOTES.md", "scratch.txt"] {
        assert!(!files.lines().any(|f| f == local), "{local} in {files}");
    }
}

#[cfg(unix)]
#[test]
fn sdk_repository_pull_requests_target_its_default_branch() {
    let dir = pull_request_project("");
    let git = |args: &[&str]| git_in(dir.path(), args);
    let remote = dir.path().join("rust-sdk.git");
    git(&[
        "init",
        "--quiet",
        "--bare",
        "--initial-branch",
        "trunk",
        "rust-sdk.git",
    ]);
    git(&["clone", "--quiet", "rust-sdk.git", "seed"]);
    fs::write(dir.path().join("seed/README.md"), "handwritten").unwrap();
    git(&["-C", "seed", "add", "README.md"]);
    git(&[
        "-C",
        "seed",
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    git(&["-C", "seed", "push", "--quiet", "origin", "HEAD:trunk"]);
    edit_config(dir.path(), |config| {
        config.replace(
            "[rust]\n",
            &format!("[rust]\nrepo = \"file://{}\"\n", remote.display()),
        )
    });

    let calls = generate_pr(dir.path(), "minor");
    assert!(calls.contains("pr create --head perseid/update"), "{calls}");
    assert!(calls.contains("--base trunk"), "{calls}");
    let files = git_in(&remote, &["ls-tree", "-r", "--name-only", "perseid/update"]);
    for file in ["README.md", "Cargo.toml", "src/lib.rs"] {
        assert!(files.lines().any(|f| f == file), "{file} not in {files}");
    }
    assert!(!files.contains("openapi.yaml"), "{files}");
}

#[test]
fn empty_sdk_repositories_get_a_skeleton() {
    let dir = project();
    let remote = dir.path().join("go-sdk.git");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet", "--bare", "go-sdk.git"]);
    git(&["clone", "--quiet", "go-sdk.git", "seed"]);
    git(&[
        "-C",
        "seed",
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    git(&["-C", "seed", "push", "--quiet", "origin", "HEAD"]);
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    fs::write(
        dir.path().join("perseid.toml"),
        config.replace(
            "[go]\n",
            &format!("[go]\nrepo = \"file://{}\"\n", remote.display()),
        ),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(ok, "{out}");
    let relative = Path::new(".perseid/repos")
        .join(dir.path().file_name().unwrap())
        .join("go-sdk");
    assert!(
        out.contains(&format!("go ({}): ", relative.display())),
        "{out}"
    );
    let checkout = dir.path().join(&relative);
    assert!(checkout.join("go.mod").exists());
    assert!(checkout.join("client.go").exists());
    assert!(
        !checkout.join("release-please-config.json").exists(),
        "`perseid setup` adds the release files"
    );

    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(ok, "{out}");
    let (_, out) = perseid(dir.path(), &["generate", "go", "--check", "--no-format"]);
    assert!(
        out.contains("+ client.go") && !out.contains("local changes"),
        "checks the generated files against the repository, empty: {out}"
    );
    fs::write(checkout.join("client.go"), "// my edit\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(
        !ok && out.contains(&format!("{} has local changes", checkout.display())),
        "{out}"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("client.go")).unwrap(),
        "// my edit\n"
    );
}

#[test]
fn sdks_in_the_repository_holding_perseid_toml_are_generated_in_place() {
    let dir = project();
    git_in(dir.path(), &["init", "--quiet"]);
    let origin = "https://github.com/Acme/Petstore-SDKs";
    git_in(dir.path(), &["remote", "add", "origin", origin]);
    edit_config(dir.path(), |c| {
        c.replace("[go]\n", "[go]\nrepo = \"acme/petstore-sdks\"\n")
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("go (go): ") && out.contains("rust (rust): "),
        "{out}"
    );
    assert!(dir.path().join("go/go.mod").exists());
    assert!(!dir.path().join(".perseid/repos").exists());
}

#[test]
fn out_previews_sdks_whose_repository_doesnt_exist_yet() {
    let dir = project();
    edit_config(dir.path(), |c| {
        c.replace(
            "[go]\n",
            "[go]\nrepo = \"file:///nonexistent/acme/petstore-go\"\n",
        )
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(!ok);
    assert!(
        out.contains(
            "file:///nonexistent/acme/petstore-go doesn't exist yet: `perseid setup-github` creates it; \
             preview with `perseid generate --out <dir>`"
        ),
        "{out}"
    );
    assert!(!dir.path().join(".perseid/repos/acme").exists());

    let (ok, out) = perseid(dir.path(), &["generate", "--out", "preview", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("go (preview/go): ") && out.contains("rust (preview/rust): "),
        "{out}"
    );
    assert!(dir.path().join("preview/go/go.mod").exists());
    assert!(dir.path().join("preview/rust/Cargo.toml").exists());
    assert!(!dir.path().join("rust").exists());

    let check = ["generate", "--out", "preview", "--check", "--no-format"];
    let (ok, out) = perseid(dir.path(), &check);
    assert!(ok && out.contains("go (preview/go): up to date"), "{out}");
    fs::remove_file(dir.path().join("preview/go/client.go")).unwrap();
    let (ok, out) = perseid(dir.path(), &check);
    assert!(!ok && out.contains("+ client.go"), "{out}");

    let (ok, out) = perseid(dir.path(), &["generate", "--out", "preview", "--pr"]);
    assert!(!ok && out.contains("--pr"), "{out}");
}

#[test]
fn csharp_releases_bump_the_csproj_version() {
    let dir = project_from("petstore.yaml", &["csharp"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let csproj = fs::read_to_string(dir.path().join("csharp/Petstore/Petstore.csproj")).unwrap();
    assert!(csproj.contains("x-release-please-version"), "{csproj}");
}

const LIST_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Shop, version: "1" }
paths:
  /items:
    get:
      operationId: list_items
      parameters:
        - { name: page, in: query, schema: { type: integer } }
      responses:
        '200': { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/ItemPage' } } } }
  /items/{id}:
    get:
      operationId: get_item
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        '200': { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/Item' } } } }
components:
  schemas:
    Item: { type: object, required: [id], properties: { id: { type: string } } }
    ItemPage:
      type: object
      required: [data, meta]
      properties:
        data: { type: array, items: { $ref: '#/components/schemas/Item' } }
        meta: { $ref: '#/components/schemas/Meta' }
    Meta: { type: object, required: [pages], properties: { pages: { type: integer } } }
"##;

fn inspect_with(pagination: &str) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("openapi.yaml"), LIST_SPEC).unwrap();
    let config =
        format!("spec = \"openapi.yaml\"\nname = \"Shop\"\nsdks = [\"go\"]\n{pagination}\n");
    fs::write(dir.path().join("perseid.toml"), config).unwrap();
    perseid(dir.path(), &["inspect"])
}

#[test]
fn pagination_rules_apply_to_matching_operations() {
    let (ok, out) = inspect_with("[pagination]\npage = \"page\"\nfirst_page = 0\n");
    assert!(ok, "{out}");
    let api: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ops = &api["resources"][0]["operations"];
    let list = ops
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "list_items");
    let pagination = &list.unwrap()["pagination"];
    assert_eq!(pagination["style"], "page");
    assert_eq!(pagination["item_schema"], "Item");
    assert_eq!(pagination["first_page"], 0);
    assert!(!out.contains("\"total_pages\""), "{out}");

    let (ok, out) = inspect_with("[pagination]\npage = \"page\"\noperations = [\"get_item\"]\n");
    assert!(!ok && out.contains("no `page` query parameter"), "{out}");
    let (ok, out) = inspect_with("[pagination]\npage = \"page\"\ntotal_pages = \"meta.nope\"\n");
    assert!(ok && !out.contains("\"pagination\""), "{out}");
    let (ok, out) = inspect_with("[pagination]\ncursor = \"page\"\n");
    assert!(ok && !out.contains("\"pagination\""), "{out}");
}

fn operation<'a>(model: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    model["resources"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["operations"].as_array().unwrap())
        .find(|op| op["id"] == id)
        .unwrap_or_else(|| panic!("no operation `{id}`"))
}

fn field_names(model: &serde_json::Value, ty: &str) -> Vec<String> {
    let fields = model["types"][ty]["fields"].as_array().unwrap();
    fields
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn real_world_constructs_generate_every_language() {
    let langs = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("realworld.yaml", &langs);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for warning in [
        "schema `Timezone`: `Etc/GMT-0` and `Etc/GMT0` both become the identifier `EtcGmt0`, so \
         the enum is typed as a string",
        "schema `PaymentMethodData`: inline schema in oneOf must have discriminator enum value, \
         so the schema is typed as an untyped JSON value",
    ] {
        assert!(out.contains(warning), "missing `{warning}` in:\n{out}");
    }
    let charge = fs::read_to_string(dir.path().join("go/charge.go")).unwrap();
    assert!(
        charge.contains("u.Customer != nil && u.Customer.ID != nil"),
        "a read-only id is optional in requests, so a pointer: {charge}"
    );

    let (ok, model) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{model}");
    let model: serde_json::Value = serde_json::from_str(&model).unwrap();
    let list = operation(&model, "GetCustomers");
    let params = &list["query_params"];
    assert_eq!(params[0]["name"], "created");
    assert_eq!(params[0]["deep_object"], true);
    assert_eq!(params[2]["name"], "metadata");
    assert_eq!(params[2]["structured"], true);
    assert_eq!(params[2]["deep_object"], false);
    let create = operation(&model, "PostCustomers");
    assert_eq!(
        create["form_deep_object"],
        serde_json::json!(["address", "expand", "metadata"])
    );
    let beta = operation(&model, "beta_createResponse");
    assert_eq!(beta["path"], "/responses?beta=true");
    assert_eq!(beta["request_body_schema_name"], "Options");
    let raw = operation(&model, "markdown/render-raw");
    assert_eq!(raw["request_body_content_type"], "text/plain");
    let customer = &model["types"]
        .as_object()
        .unwrap()
        .values()
        .find(|t| t["name"] == "Customer")
        .unwrap()["fields"];
    let id = customer
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "id");
    assert_eq!(
        id.unwrap()["required"],
        false,
        "read-only in a request body"
    );

    let csharp = dir.path().join("csharp/RealWorld/Models/OptionsModel.cs");
    assert!(
        csharp.is_file(),
        "`Options` names the JSON context options in C#"
    );
    let rust = fs::read_to_string(dir.path().join("rust/src/models/options.rs")).unwrap();
    assert!(rust.contains("pub struct Options"), "{rust}");
}

#[test]
fn unions_of_objects_follow_untagged_unions_and_x_perseid_union() {
    let dir = project_from("torture.yaml", &["rust"]);
    let generate = || {
        let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
        assert!(ok, "{out}");
        out
    };
    let read = |model: &str| {
        let path = dir.path().join(format!("rust/src/models/{model}.rs"));
        fs::read_to_string(path)
            .unwrap()
            .split_whitespace()
            .collect::<String>()
    };
    let out = generate();
    assert!(
        out.contains("decoded as their best-matching variant: 1"),
        "{out}"
    );
    assert!(out.contains("left as untyped JSON: 1"), "{out}");
    let unions = read("object_unions");
    for text in [
        "pubenumObjectUnionsAccount",
        "has(&value,\"deleted\",Some(serde_json::json!(true)))",
        "pubenumObjectUnionsDocument",
        "pubraw_account:Option<serde_json::Value>",
    ] {
        assert!(unions.contains(text), "no `{text}` in {unions}");
    }
    assert!(read("document").contains("pubtypeDocument=serde_json::Value;"));

    edit_config(dir.path(), |text| {
        format!("untagged_unions = \"best-match\"\n{text}")
    });
    let out = generate();
    assert!(
        out.contains("decoded as their best-matching variant: 2"),
        "{out}"
    );
    assert!(!out.contains("left as untyped JSON"), "{out}");
    assert!(read("document").contains("best_match(&value,"));

    edit_config(dir.path(), |text| {
        text.replace("[rust]\n", "[rust]\nuntagged_unions = \"json\"\n")
    });
    assert!(generate().contains("left as untyped JSON: 1"));

    edit_config(dir.path(), |text| text.replace("\"json\"", "\"guess\""));
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(!ok && out.contains("guess"), "{out}");
}

#[test]
fn torture_fixture_generates_every_language() {
    let langs = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("torture.yaml", &langs);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert_eq!(
        out.matches("schema `MapOrAddress`: `oneOf`").count(),
        1,
        "warnings are printed once for all languages: {out}"
    );

    let (ok, model) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{model}");
    let model: serde_json::Value = serde_json::from_str(&model).unwrap();
    let list = operation(&model, "list_widgets");
    assert_eq!(list["response_body_schema_name"], "Widget");
    assert_eq!(list["response_body_is_list"], true);
    let derived = operation(&model, "get_v2_widgets_by_widget_id");
    assert_eq!(derived["path_params"], serde_json::json!(["widget_id"]));
    assert_eq!(derived["response_body_schema_name"], "Widget");
    let update = operation(&model, "update_widget");
    assert_eq!(update["request_body_schema_name"], "WidgetUpdate");
    assert_eq!(update["response_body_schema_name"], "Widget");
    let create = operation(&model, "create_widget");
    assert_eq!(create["request_body_schema_name"], "CreateWidgetRequest");
    assert_eq!(create["response_body_schema_name"], "CreateWidgetResponse");
    assert_eq!(
        operation(&model, "bulk_create_widgets")["request_body_is_list"],
        true
    );
    assert_eq!(
        operation(&model, "count_widgets")["response_body_json_type"]["id"],
        "Map"
    );
    assert_eq!(
        field_names(&model, "Composed"),
        ["__flatten_base", "extra", "sibling_prop"]
    );
    assert_eq!(field_names(&model, "Nested"), ["__flatten_base", "depth"]);
    assert_eq!(field_names(&model, "InlineEvent"), ["at"]);
    assert_eq!(
        field_names(&model, "CreateWidgetRequest"),
        ["dimensions", "name", "parts"]
    );
    let variants: Vec<_> = model["types"]["Activity"]["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["name"].as_str().unwrap(),
                v["schema_ref"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        variants,
        [
            ("opened", "Opened"),
            ("reopened", "Opened"),
            ("closed", "ActivityClosedVariant")
        ]
    );
    for ty in ["Problem", "WidgetReactions", "CreateWidgetRequestPartsItem"] {
        assert!(model["types"].get(ty).is_some(), "{ty} was not promoted");
    }

    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let rust = read("rust/src/api/widgets.rs");
    assert!(rust.contains("Vec<crate::models::Widget>"), "{rust}");
    assert!(rust.contains("date_created_lt"), "{rust}");
    let reactions = read("rust/src/models/widget_reactions.rs");
    assert!(
        reactions.contains("pub plus_1") && reactions.contains("pub minus_1"),
        "{reactions}"
    );
    assert!(read("typescript/src/api/widgets.ts").contains("Promise<Widget[]>"));
    assert!(read("python/torture/api/widgets.py").contains("-> t.List[Widget]:"));
    let tree = read("python/torture/models/tree_node.py");
    assert!(!tree.contains("import TreeNode"), "{tree}");
    assert!(read("python/torture/api/class_.py").contains("class Class(ApiBaseSync)"));
    assert!(read("go/widgets.go").contains("([]Widget, error)"));
    assert!(read("java/src/main/java/com/torture/api/Widgets.java").contains("List<Widget>"));
    let activity = read("csharp/Torture/Api/ActivityApi.cs");
    assert!(
        activity.contains("public sealed partial class ActivityApi"),
        "{activity}"
    );
    assert!(
        read("csharp/Torture/TortureClient.cs").contains("public ActivityApi ActivityApi { get; }")
    );
}

#[test]
fn java_types_string_aliases_as_strings() {
    let dir = tempfile::tempdir().unwrap();
    let spec = r##"
openapi: 3.1.0
info: { title: Edge, version: "1" }
paths:
  /token:
    post:
      operationId: rotate_token
      x-perseid-name: rotate
      tags: [tokens]
      parameters: [{ name: previous, in: query, schema: { $ref: "#/components/schemas/Token" } }]
      requestBody: { content: { application/json: { schema: { $ref: "#/components/schemas/Token" } } } }
      responses:
        "200":
          description: ok
          content: { application/json: { schema: { $ref: "#/components/schemas/Token" } } }
components:
  schemas:
    Token: { type: string, format: uuid }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "java"]);
    assert!(ok, "{out}");
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let tokens = fs::read_to_string(
        dir.path()
            .join("java/src/main/java/com/edge/api/Tokens.java"),
    )
    .unwrap();
    assert!(tokens.contains("public String rotate("), "{tokens}");
    assert!(!tokens.contains("com.edge.models"), "{tokens}");
    let options = dir
        .path()
        .join("java/src/main/java/com/edge/api/TokensRotateOptions.java");
    let options = fs::read_to_string(options).unwrap();
    assert!(options.contains("String previous"), "{options}");
    assert!(!options.contains("com.edge.models"), "{options}");
}

#[test]
fn unsupported_constructs_are_listed_with_their_operation_or_schema() {
    let dir = tempfile::tempdir().unwrap();
    let spec = r##"
openapi: 3.1.0
info: { title: Edge, version: "1" }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - { name: ids, in: query, style: pipeDelimited, schema: { type: array, items: { type: string } } }
        - { name: session, in: cookie, schema: { type: string } }
      responses: { "204": { description: ok } }
  /render/{spec}:
    post:
      parameters:
        - { name: spec, in: path, required: true, schema: { type: object } }
      responses: { "204": { description: ok } }
  /clash:
    get:
      responses:
        "200":
          description: ok
          content: { application/json: { schema: { $ref: "#/components/schemas/Clash" } } }
components:
  schemas:
    Clash:
      type: object
      properties: { type: { type: string }, "@type": { type: string } }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "rust"]);
    assert!(ok, "{out}");
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(!ok);
    assert!(!out.contains("panicked"), "{out}");
    for line in [
        "operation `search` (GET /search): query parameter `ids`: style \"pipeDelimited\" is not supported",
        "operation `post_render_by_spec` (POST /render/{spec}): path parameter `spec`: only scalar values are supported, not type `object`",
        "schema `Clash`: `@type` and `type` both become the identifier `type`",
    ] {
        assert!(out.contains(line), "missing `{line}` in:\n{out}");
    }
}

#[test]
fn rust_models_box_recursion_and_use_chrono_dates() {
    let dir = project_from("torture.yaml", &["rust"]);
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok, "{out}");
    // Unformatted output, so compare without whitespace.
    let read = |path: &str| {
        let text = fs::read_to_string(dir.path().join("rust/src/models").join(path)).unwrap();
        text.split_whitespace().collect::<String>()
    };
    let tree = read("tree_node.rs");
    assert!(tree.contains("Option<Box<TreeNode>>"), "{tree}");
    assert!(tree.contains("children:Vec<TreeNode>"), "{tree}");
    assert!(read("thing.rs").contains("chrono::DateTime<chrono::Utc>"));
    let patch = read("thing_patch.rs");
    assert!(patch.contains("Option<Option<String>>"), "{patch}");
    assert!(read("kind.rs").contains("Unknown(String)"));
}

#[test]
fn typescript_int64_is_opt_in() {
    let dir = project_from("torture.yaml", &["typescript", "go"]);
    let config = dir.path().join("perseid.toml");
    let original = fs::read_to_string(&config).unwrap();
    let thing = || fs::read_to_string(dir.path().join("typescript/src/models/thing.ts")).unwrap();

    let (ok, out) = perseid(dir.path(), &["generate", "typescript", "--no-format"]);
    assert!(ok, "{out}");
    assert!(thing().contains("count: number;"), "{}", thing());

    fs::write(
        &config,
        original.replace("[typescript]", "[typescript]\nint64 = \"bigint\""),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "typescript", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        thing().contains("count: bigint;") && thing().contains("BigInt(json[\"count\"])"),
        "{}",
        thing()
    );

    fs::write(
        &config,
        original.replace("[typescript]", "[typescript]\nint64 = \"long\""),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(!ok && out.contains("unknown variant `long`"), "{out}");

    fs::write(
        &config,
        original.replace("[go]", "[go]\nint64 = \"string\""),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(
        !ok && out.contains("only supported in [typescript]"),
        "{out}"
    );
}

fn edit_config(dir: &Path, edit: impl FnOnce(String) -> String) {
    let path = dir.join("perseid.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(path, edit(text)).unwrap();
}

fn inspect(dir: &Path) -> serde_json::Value {
    let (ok, out) = perseid(dir, &["inspect"]);
    assert!(ok, "{out}");
    serde_json::from_str(&out).unwrap()
}

#[test]
fn init_names_methods_after_their_resource_path() {
    let dir = project_from("realworld.yaml", &["rust", "typescript"]);
    let model = inspect(dir.path());
    for (id, name) in [
        ("GetCharges", "list"),
        ("PostCharges", "create"),
        ("GetChargesSearch", "search"),
        ("GetChargesCharge", "retrieve"),
        ("PostChargesCharge", "update"),
        ("PostChargesChargeCapture", "capture"),
        ("GetChargesChargeRefunds", "refunds"),
        ("PutCustomer", "update"),
    ] {
        let op = operation(&model, id);
        assert_eq!(op["name"], name, "{id}");
    }

    edit_config(dir.path(), |c| {
        c.replacen(
            "[rust]\n",
            "[rust]\nmethods = { GetChargesSearch = \"find\" }\n",
            1,
        ) + "\n[methods]\nPostChargesChargeCapture = \"capture_payment\"\n"
    });
    assert_eq!(
        operation(&inspect(dir.path()), "PostChargesChargeCapture")["name"],
        "capture_payment"
    );
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let rust = fs::read_to_string(dir.path().join("rust/src/api/charges.rs")).unwrap();
    for method in [
        "fn find(",
        "fn capture_payment(",
        "fn retrieve(",
        "fn refunds(",
    ] {
        assert!(rust.contains(method), "no `{method}` in {rust}");
    }
    let ts = fs::read_to_string(dir.path().join("typescript/src/api/charges.ts")).unwrap();
    assert!(
        ts.contains("public search(") && ts.contains("public capturePayment("),
        "{ts}"
    );

    edit_config(dir.path(), |c| c + "GetCharges = \"capture_payment\"\n");
    let (ok, out) = perseid(dir.path(), &["inspect"]);
    assert!(
        !ok && out.contains("are both named `capture_payment`"),
        "{out}"
    );
}

#[test]
fn error_responses_are_typed_per_operation() {
    let dir = project_from("realworld.yaml", &["rust"]);
    let model = inspect(dir.path());
    assert_eq!(model["error_schemas"], serde_json::json!(["Error"]));
    assert_eq!(model["default_error"], "Error");
    assert_eq!(
        operation(&model, "GetChargesCharge")["errors"],
        serde_json::json!({ "404": "Error", "default": "Error" })
    );
    assert_eq!(
        operation(&model, "PutCustomer")["errors"],
        serde_json::json!({})
    );
}

#[test]
fn primitive_or_object_unions_are_modelled() {
    let dir = project_from("realworld.yaml", &["rust"]);
    let model = inspect(dir.path());
    let fields = model["types"]["Charge"]["fields"].as_array().unwrap();
    let field = |name: &str| &fields.iter().find(|f| f["name"] == name).unwrap()["type"];
    let variants = |name: &str| -> Vec<(String, String)> {
        field(name)["variants"]
            .as_array()
            .unwrap_or_else(|| panic!("`{name}` is not a union"))
            .iter()
            .map(|v| {
                (
                    v["name"].as_str().unwrap().into(),
                    v["json_type"].as_str().unwrap().into(),
                )
            })
            .collect()
    };
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(
        variants("customer"),
        [pair("string", "string"), pair("customer", "object")]
    );
    assert_eq!(
        variants("shipping"),
        [pair("charge_shipping", "object"), pair("empty", "string")]
    );
    assert_eq!(
        variants("amount"),
        [pair("integer", "integer"), pair("empty", "string")]
    );
    assert_eq!(
        variants("source"),
        [
            pair("string", "string"),
            pair("customer", "object"),
            pair("upload", "object")
        ]
    );
    assert_eq!(field("source")["mode"], "rules");
    assert_eq!(
        field("source")["variants"][1]["when"],
        serde_json::json!([{"property": "object", "value": "customer"}])
    );
    assert!(model["types"]["ChargeShipping"].is_object());
}

#[test]
fn html_descriptions_become_markdown_doc_comments() {
    let dir = project_from("realworld.yaml", &["rust"]);
    let model = inspect(dir.path());
    assert_eq!(
        model["types"]["Charge"]["description"],
        "A `Charge` moves money from a card. See [charges](https://docs.example.com/charges)."
    );
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let api = fs::read_to_string(dir.path().join("rust/src/api/charges.rs")).unwrap();
    assert!(api.contains("/// Returns a list of charges, the most recent first. Use `created` & [expand](https://docs.example.com/expand)."), "{api}");
    assert!(
        api.contains("/// - Paginated") && !api.contains("<p>"),
        "{api}"
    );
}

#[test]
fn union_variants_default_their_discriminator() {
    let dir = project_from("torture.yaml", &["rust"]);
    let model = inspect(dir.path());
    assert_eq!(
        model["types"]["Circle"]["discriminator_defaults"],
        serde_json::json!({ "type": "circle" })
    );
    assert!(
        model["types"]["Widget"]
            .get("discriminator_defaults")
            .is_none()
    );
}

#[test]
fn init_fills_package_metadata_from_the_spec() {
    let dir = tempfile::tempdir().unwrap();
    let spec = fs::read_to_string("tests/fixtures/petstore.yaml").unwrap().replacen(
        "info:\n",
        "info:\n  description: The Petstore API. Manage pets.\n  license: { name: MIT }\n  contact: { name: Pet Team, email: pets@example.com, url: https://pets.example.com }\n",
        1,
    );
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet"]);
    git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:acme/petstore.git",
    ]);
    let (ok, out) = perseid(
        dir.path(),
        &["init", "--sdks", "rust,typescript,python,java,csharp"],
    );
    assert!(ok, "{out}");
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let config = read("perseid.toml");
    assert!(
        config.starts_with(
            "#:schema https://raw.githubusercontent.com/meteroid-oss/perseid/main/perseid.schema.json\n"
        ),
        "{config}"
    );
    let table: toml::Table = config.parse().unwrap();
    assert_eq!(
        table["metadata"]["license"].as_str(),
        Some("MIT"),
        "{config}"
    );
    assert!(!config.contains("repository"), "{config}");
    for line in [
        "description = \"The Petstore API.\"",
        "license = \"MIT\"",
        "homepage = \"https://pets.example.com\"",
        "authors = [\"Pet Team <pets@example.com>\"]",
    ] {
        assert!(config.contains(line), "no `{line}` in {config}");
    }
    let cargo = read("rust/Cargo.toml");
    assert!(
        cargo.contains("license = \"MIT\"")
            && cargo.contains("authors = [\"Pet Team <pets@example.com>\"]"),
        "{cargo}"
    );
    assert!(
        cargo.contains("repository = \"https://github.com/acme/petstore\"")
            && !cargo.contains("@@"),
        "{cargo}"
    );
    let package: serde_json::Value =
        serde_json::from_str(&read("typescript/package.json")).unwrap();
    assert_eq!(
        (package["license"].as_str(), package["author"].as_str()),
        (Some("MIT"), Some("Pet Team <pets@example.com>"))
    );
    assert_eq!(
        package["repository"]["url"],
        "git+https://github.com/acme/petstore.git"
    );
    let pyproject = read("python/pyproject.toml");
    assert!(
        pyproject.contains("authors = [{ name = \"Pet Team\", email = \"pets@example.com\" }]"),
        "{pyproject}"
    );
    let properties = read("java/gradle.properties");
    assert!(
        properties.contains("POM_LICENSE_URL=https://spdx.org/licenses/MIT.html")
            && properties.contains("POM_SCM_URL=https://github.com/acme/petstore\n"),
        "{properties}"
    );
    let csproj = read("csharp/Petstore/Petstore.csproj");
    assert!(
        csproj.contains("<PackageLicenseExpression>MIT</PackageLicenseExpression>")
            && csproj.contains("<Authors>Pet Team</Authors>"),
        "{csproj}"
    );
}

#[test]
fn rust_unions_and_tag_defaults_are_typed() {
    let dir = project_from("realworld.yaml", &["rust"]);
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("\n[rust]\n"), "{config}");
    assert!(!config.contains("untagged_unions"), "{config}");
    let charge = || {
        let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
        assert!(ok, "{out}");
        let path = dir.path().join("rust/src/models/charge.rs");
        fs::read_to_string(path)
            .unwrap()
            .split_whitespace()
            .collect::<String>()
    };
    let typed = charge();
    assert!(typed.contains("customer:Option<ChargeCustomer>"), "{typed}");
    assert!(
        typed.contains("pubenumChargeCustomer{String(String),Customer(Box<Customer>),"),
        "{typed}"
    );
    assert!(
        typed.contains("pubenumChargeAmount{Integer(i64),///Theemptystring.Empty,"),
        "{typed}"
    );
    let models = fs::read_to_string(dir.path().join("rust/src/models/mod.rs")).unwrap();
    assert!(
        models.contains("pub fn id(&self) -> Option<&str>"),
        "{models}"
    );
}

#[test]
fn rust_timeout_stream_and_error_docs_follow_the_config() {
    let dir = project_from("torture.yaml", &["rust"]);
    let read = |path: &str| {
        let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
        assert!(ok, "{out}");
        fs::read_to_string(dir.path().join("rust/src").join(path)).unwrap()
    };
    assert!(read("api/client.rs").contains("Some(Duration::from_secs(60))"));
    let api = read("api/mod.rs");
    assert!(api.contains("futures_core::Stream for Paginator"), "{api}");
    assert!(
        api.contains("pub type ErrorBody = serde_json::Value;"),
        "{api}"
    );
    let things = read("api/things.rs");
    assert!(
        things.contains("[`ValidationError`](crate::models::ValidationError) (422)"),
        "{things}"
    );
    let circle = read("models/circle.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(circle.contains("pubfnnew(radius:f64,)"), "{circle}");

    edit_config(dir.path(), |text| format!("timeout = 15\n{text}"));
    assert!(read("api/client.rs").contains("Some(Duration::from_secs(15))"));
}

#[test]
fn typescript_types_unions_errors_and_the_default_timeout() {
    let dir = project_from("realworld.yaml", &["typescript"]);
    edit_config(dir.path(), |text| {
        text.replace(
            "[typescript]",
            "timeout = 15\nwebhooks = true\n[typescript]",
        )
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("typescript/src").join(path)).unwrap();
    let charge = read("models/charge.ts");
    for decl in [
        "customer: string | Customer | null;",
        "amount?: number | \"\";",
        "shipping?: ChargeShipping | \"\";",
        "source?: string | Customer | UploadModel;",
    ] {
        assert!(charge.contains(decl), "no `{decl}` in {charge}");
    }
    assert!(
        charge.contains(
            "isJsonObject(json[\"customer\"]) ? CustomerSerializer.parse(json[\"customer\"])"
        ),
        "{charge}"
    );
    let index = read("index.ts");
    for text in [
        "const DEFAULT_TIMEOUT_MS = 15000;",
        "parseError: ErrorSerializer.parse,",
        "export type RealWorldErrorBody =",
        "NotFoundError,",
        "static readonly NotFoundError = NotFoundError;",
    ] {
        assert!(index.contains(text), "no `{text}` in {index}");
    }
    assert!(read("api/charges.ts").contains("public retrieve("));
    assert!(!read("webhook.ts").contains("from \"node:"));
}

#[test]
fn python_types_errors_unions_and_discriminator_defaults() {
    let dir = project_from("realworld.yaml", &["python"]);
    fs::write(
        dir.path().join("perseid.toml"),
        fs::read_to_string(dir.path().join("perseid.toml")).unwrap() + "timeout = 15\n",
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let errors = read("python/real_world/api/_errors.py");
    assert!(
        errors.contains("ErrorBody: t.TypeAlias = _models.Error\n"),
        "{errors}"
    );
    assert!(read("python/real_world/api/client.py").contains("class RealWorld:"));
    let charge = read("python/real_world/models/charge.py");
    for decl in [
        "customer: str | Customer | None",
        "shipping: ChargeShipping | t.Literal[\"\"] | None = None",
        "from .customer import Customer",
    ] {
        assert!(charge.contains(decl), "no `{decl}` in {charge}");
    }
    let common = read("python/real_world/api/common.py");
    assert!(common.contains("DEFAULT_TIMEOUT: float = 15\n"), "{common}");

    let dir = project_from("torture.yaml", &["python"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read = |path: &str| fs::read_to_string(dir.path().join(path)).unwrap();
    let circle = read("python/torture/models/circle.py");
    assert!(circle.contains("type: str = \"circle\""), "{circle}");
    let things = read("python/torture/api/things.py");
    assert!(
        things.contains("\"422\": _models.ValidationError,") && things.contains("def create("),
        "{things}"
    );
}

#[test]
fn go_types_unions_initialisms_and_error_bodies() {
    let dir = project_from("realworld.yaml", &["go"]);
    let config = dir.path().join("perseid.toml");
    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("warning: the Go module path is `real-world`"),
        "{out}"
    );
    let read = |path: &str| fs::read_to_string(dir.path().join("go").join(path)).unwrap();
    let charge = read("charge.go");
    assert!(
        charge.contains("Customer *ChargeCustomer `json:\"customer\"`")
            && charge.contains("func NewChargeCustomerFromCustomer(value Customer) ChargeCustomer"),
        "{charge}"
    );
    let client = read("client.go");
    assert!(
        client.contains("func (e *APIError) Detail() *Error {")
            && client.contains("const DefaultTimeout = 60 * time.Second")
            && client.contains("//\tresult, err := client.Charges().List(ctx)"),
        "{client}"
    );
    assert!(read("README.md").contains("go get real-world\n"));

    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("[go]\n", "[go]\ntimeout = 15\n")).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "go", "--no-format"]);
    assert!(ok, "{out}");
    assert!(read("client.go").contains("const DefaultTimeout = 15 * time.Second"));
}

#[test]
fn java_is_unchecked_and_hides_plumbing() {
    let dir = project_from("petstore.yaml", &["java"]);
    let (ok, out) = perseid(dir.path(), &["generate"]);
    assert!(ok, "{out}");
    let root = dir.path().join("java/src/main/java/com/petstore");
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    assert!(read("internal/Utils.java").starts_with("// this file is @generated"));
    assert!(read("internal/PetstoreHttpClient.java").contains("package com.petstore.internal;"));
    let pets = read("api/Pets.java");
    assert!(!pets.contains("throws"), "{pets}");
    assert!(
        pets.contains("final RequestOptions requestOptions)"),
        "{pets}"
    );
    assert!(read("exceptions/ApiException.java").contains("extends RuntimeException"));
    assert!(read("models/PetStatus.java").contains("public final class PetStatus"));
}

#[test]
fn removed_keys_fail_with_a_hint() {
    let dir = project_from("petstore.yaml", &["java"]);
    let config = dir.path().join("perseid.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("[java]\n", "[java]\nedition = 2\n")).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate"]);
    assert!(!ok && out.contains("[java] `edition` was removed"), "{out}");
    fs::write(&config, format!("typed_unions = true\n{text}")).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate"]);
    assert!(!ok && out.contains("`typed_unions` was removed"), "{out}");
}

#[test]
fn java_enums_keep_unknown_values_and_docs_are_html() {
    let dir = project_from("realworld.yaml", &["java"]);
    let (ok, out) = perseid(dir.path(), &["generate"]);
    assert!(ok, "{out}");
    let models = dir.path().join("java/src/main/java/com/realworld/models");
    let status = fs::read_to_string(models.join("StateReason.java")).unwrap();
    for part in [
        "public final class StateReason",
        "public static StateReason of(String value)",
        "public boolean isKnown()",
        "public enum Known {",
    ] {
        assert!(status.contains(part), "no `{part}` in {status}");
    }
    let api = fs::read_to_string(
        dir.path()
            .join("java/src/main/java/com/realworld/api/Charges.java"),
    )
    .unwrap();
    assert!(api.contains("<code>created</code> &amp;"), "{api}");
}

#[test]
fn csharp_types_unions_errors_and_timeout() {
    let dir = project_from("realworld.yaml", &["csharp"]);
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    fs::write(
        dir.path().join("perseid.toml"),
        config.replacen("\n[csharp]", "timeout = 15\n\n[csharp]", 1),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("csharp/RealWorld").join(path)).unwrap();
    let charge = read("Models/Charge.cs");
    for expected in [
        "public required ChargeCustomer? Customer { get; set; }",
        "public sealed record Customer(global::RealWorld.Models.Customer Value) : ChargeCustomer;",
        "public static implicit operator ChargeCustomer(string value) => new String(value);",
        "public sealed record Empty() : ChargeAmount;",
        "/// A <c>Charge</c> moves money from a card.",
    ] {
        assert!(charge.contains(expected), "no `{expected}` in {charge}");
    }
    assert!(
        read("Models/RealWorldJsonContext.cs")
            .contains("ChargeCustomer.Customer variant => variant.Value.Id,")
    );
    let charges = read("Api/ChargesApi.cs");
    assert!(charges.contains("RetrieveAsync("), "{charges}");
    assert!(
        charges.contains("/// <exception cref=\"NotFoundException\">404: <c>GetError()</c> reads the <see cref=\"Models.Error\"/> body.</exception>"),
        "{charges}"
    );
    assert!(
        read("RealWorldClient.cs")
            .contains("public static Models.Error? GetError(this ApiException exception)")
    );
    assert!(read("RealWorldClientOptions.cs").contains("TimeSpan.FromSeconds(15)"));
}
