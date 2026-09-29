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
