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
    let args = [&["init"], languages].concat();
    let (ok, out) = perseid(dir.path(), &args);
    assert!(ok, "{out}");
    dir
}

#[test]
fn init_derives_names_from_the_spec() {
    let dir = project();
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("name = \"Petstore\""), "{config}");
    assert!(
        config.contains("base_url = \"https://petstore.example.com\""),
        "{config}"
    );
    assert!(dir.path().join("rust/src/error.rs").exists());
    assert!(dir.path().join("go/go.mod").exists());
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
    assert!(out.contains("rust: up to date"), "{out}");

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
    write("go/retired/old.go", marker);

    let (ok, out) = perseid(dir.path(), &["generate", "--check", "--no-format"]);
    assert!(!ok);
    assert!(out.contains("- src/retired/old.rs"), "{out}");
    assert!(out.contains("- retired/old.go"), "{out}");
    assert!(!out.contains("target"), "{out}");

    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!dir.path().join("rust/src/retired/old.rs").exists());
    assert!(!dir.path().join("go/retired/old.go").exists());
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
    let langs = ["rust", "typescript", "python", "go", "java"];
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
    let (ok, out) = perseid(dir.path(), &["init", "rust"]);
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
    let (ok, out) = perseid(dir.path(), &["init", "csharp"]);
    assert!(ok, "{out}");
    let sdk = dir.path().join("csharp");
    let project = fs::read_to_string(sdk.join("Petstore/Petstore.csproj")).unwrap();
    assert!(project.contains("<Version>0.1.0</Version>"), "{project}");
    assert!(sdk.join("Petstore.sln").exists());

    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for file in [
        "PetstoreClient.cs",
        "Api/PetsApi.cs",
        "Api/PetsListPetsOptions.cs",
        "Models/Pet.cs",
        "Models/PetstoreJsonContext.cs",
        "ApiTransport.cs",
    ] {
        assert!(sdk.join("Petstore").join(file).exists(), "{file}: {out}");
    }
    let api = fs::read_to_string(sdk.join("Petstore/Api/PetsApi.cs")).unwrap();
    assert!(api.contains("public Task<Pet> GetPetAsync("), "{api}");
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn init_configures_release_please_per_sdk() {
    let dir = project();
    let config = json(&dir.path().join("release-please-config.json"));
    assert_eq!(config["packages"]["rust"]["release-type"], "rust");
    assert_eq!(config["packages"]["go"]["version-file"], "version.go");
    assert_eq!(config["tag-separator"], "/");
    assert_eq!(
        json(&dir.path().join(".release-please-manifest.json")),
        serde_json::json!({}),
        "never released SDKs start at initial-version"
    );
    assert!(
        dir.path()
            .join(".github/workflows/sdk-release.yml")
            .exists()
    );

    fs::create_dir(dir.path().join("python")).unwrap();
    fs::write(
        dir.path().join("python/pyproject.toml"),
        "[project]\nname = \"petstore\"\nversion = \"1.4.0\"\n",
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "python", "java"]);
    assert!(ok, "{out}");
    let config = json(&dir.path().join("release-please-config.json"));
    assert_eq!(config["packages"]["python"]["release-type"], "python");
    assert_eq!(
        config["packages"]["java"]["extra-files"][1],
        "src/main/java/com/petstore/Version.java"
    );
    assert_eq!(
        json(&dir.path().join(".release-please-manifest.json")),
        serde_json::json!({ "python": "1.4.0" })
    );
    let properties = fs::read_to_string(dir.path().join("java/gradle.properties")).unwrap();
    assert!(properties.contains("VERSION_NAME=0.1.0"), "{properties}");
}

#[test]
fn bump_needs_a_pull_request() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--bump", "major"]);
    assert!(!ok && out.contains("--pr"), "{out}");
}

#[cfg(unix)]
#[test]
fn open_pull_requests_keep_their_largest_bump() {
    use std::os::unix::fs::PermissionsExt;

    let dir = project();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet", "--initial-branch", "main"]);
    git(&["init", "--quiet", "--bare", "origin.git"]);
    git(&["remote", "add", "origin", "origin.git"]);
    fs::write(dir.path().join(".gitignore"), "origin.git\nbin\n").unwrap();
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

    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\necho \"$@\" >> \"$GH_LOG\"\ncase \"$2\" in\n  list) printf 'https://pr/1\\tfeat(api)!: update SDKs to Petstore 1\\n' ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.path().join("gh.log");
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["generate", "rust", "--pr", "--bump", "patch", "--no-format"])
        .current_dir(dir.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("GH_LOG", &log)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("pr edit perseid/update --title feat(api)!: update SDKs to"),
        "{calls}"
    );
}

#[test]
fn empty_sdk_repositories_get_a_skeleton_releasing_from_their_root() {
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
    let checkout = dir
        .path()
        .join(".perseid/repos")
        .join(dir.path().file_name().unwrap())
        .join("go-sdk");
    assert!(checkout.join("go.mod").exists());
    assert!(checkout.join("client.go").exists());
    let release = json(&checkout.join("release-please-config.json"));
    assert_eq!(release["packages"]["."]["include-component-in-tag"], false);
    assert!(checkout.join(".github/workflows/sdk-release.yml").exists());
}

#[test]
fn init_no_release_skips_release_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "rust", "--no-release"]);
    assert!(ok, "{out}");
    assert!(dir.path().join("rust/Cargo.toml").exists());
    assert!(!dir.path().join("release-please-config.json").exists());
    assert!(!dir.path().join(".release-please-manifest.json").exists());
    assert!(!dir.path().join(".github").exists());
}

#[test]
fn csharp_releases_bump_the_csproj_version() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "csharp"]);
    assert!(ok, "{out}");
    let release = json(&dir.path().join("release-please-config.json"));
    assert_eq!(
        release["packages"]["csharp"]["extra-files"][0],
        "Petstore/Petstore.csproj"
    );
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
    let config = format!("spec = \"openapi.yaml\"\nname = \"Shop\"\n{pagination}\n[go]\n");
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
