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
