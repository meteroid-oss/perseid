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

#[test]
fn csharp_generates_streaming_auth_and_pagination() {
    let dir = project_from("features.yaml", &["csharp"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("csharp/Features").join(path)).unwrap();
    let streaming = read("Api/StreamingApi.cs");
    assert!(
        streaming.contains("public Task<EventStream> StreamEventsAsync("),
        "{streaming}"
    );
    assert!(
        streaming.contains("public sealed class StreamingUploadFileBody"),
        "{streaming}"
    );
    assert!(streaming.contains("Upload body,"), "{streaming}");
    let widgets = read("Api/WidgetsApi.cs");
    assert!(
        widgets.contains("public IAsyncEnumerable<Widget> ListWidgetsIterAsync("),
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
fn torture_fixture_generates_every_language() {
    let langs = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("torture.yaml", &langs);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert_eq!(
        out.matches("schema `StringOrInt`: `oneOf`").count(),
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
    assert!(read("python/torture/api/widgets.py").contains("-> list[Widget]:"));
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
      operationId: rotate
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
    let (ok, out) = perseid(dir.path(), &["init", "java"]);
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
    let (ok, out) = perseid(dir.path(), &["init", "rust"]);
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
fn rust_models_box_recursion_and_follow_the_chrono_dependency() {
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

    let manifest = dir.path().join("rust/Cargo.toml");
    let without_chrono: String = fs::read_to_string(&manifest)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("chrono"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&manifest, without_chrono).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok, "{out}");
    let thing = read("thing.rs");
    assert!(!thing.contains("chrono"), "{thing}");
    assert!(thing.contains("pubcreated_at:String"), "{thing}");
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
    assert!(!ok && out.contains("`int64` must be"), "{out}");

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
