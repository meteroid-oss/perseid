#[cfg(unix)]
use std::sync::{Arc, Mutex};
use std::{fs, path::Path, process::Command};

#[cfg(unix)]
use serde_json::{Value, json};

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
fn init_takes_the_name_in_any_case_and_writes_nothing_for_an_invalid_one() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "go", "--name", "2fa"]);
    assert!(!ok && out.contains("must start with a letter"), "{out}");
    assert!(!dir.path().join("perseid.toml").exists());
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "go", "--name", "acme pay"]);
    assert!(ok && out.contains("AcmePay SDKs: go/ here"), "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("name = \"AcmePay\"\n"), "{config}");
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
fn init_from_stainless_maps_targets_pagination_and_method_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/stainless-openapi.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    fs::copy(
        "tests/fixtures/stainless.yml",
        dir.path().join("stainless.yml"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--from", "openapi.yaml"]);
    assert!(!ok && out.contains("pass it as --spec"), "{out}");
    let (ok, out) = perseid(dir.path(), &["init", "--from", "stainless.yml"]);
    assert!(ok, "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    for expected in [
        "spec = \"openapi.yaml\"\nname = \"Knock\"\nsdks = [\"typescript\", \"python\", \"go\"]\nbase_url = \"https://api.knock.app\"\nidempotency_keys = true\nexclude = [\"notify\"]\n",
        "license = \"Apache-2.0\"\nhomepage = \"https://docs.knock.app\"\nauthors = [\"knock <support@knock.app>\"]\n",
        "[methods]\naddAudienceMembers = \"add_members\"\narchiveMessage = \"mark_as_archived\"\ngetUser = \"get\"\ngetUserFeed = \"list_items\"\n\n",
        "[resources]\naddAudienceMembers = \"audiences\"\ngetUserFeed = \"users.feeds\"\nlistAudienceMembers = \"audiences\"\n\n",
        "# entries_cursor\ncursor = \"after\"\nitems = \"entries\"\nnext_cursor = \"page_info.after\"\n",
        "# items_cursor\ncursor = \"after\"\nitems = \"items\"\nnext_cursor = \"page_info.after\"\n",
        "[typescript]\npackage = \"@knocklabs/node\"\nrepo = \"knocklabs/knock-node\"\n",
        "[python]\npackage = \"knockapi\"\nrepo = \"knocklabs/knock-python\"\nexclude = [\"listAudienceMembers\"]\n",
    ] {
        assert!(config.contains(expected), "{expected}\n---\n{config}");
    }
    assert!(
        !config.contains("env_prefix") && !config.contains("timeout"),
        "{config}"
    );
    assert!(
        out.contains(
            "Knock SDKs: knocklabs/knock-node, knocklabs/knock-python, knocklabs/knock-go"
        ),
        "{out}"
    );
    assert!(
        out.contains("3 resource placements, 4 method names")
            && !out.contains("resources.users.feeds"),
        "{out}"
    );
    for skipped in [
        "targets.ruby: perseid generates no Ruby SDK",
        "targets.{go,python,typescript}.staging_repo: ",
        "client_settings.opts.branch: perseid clients take no custom options: send `X-Knock-Branch`",
        "pagination.entries_cursor.request.before: previous_cursor_param",
        "pagination.slack_channels_cursor: `query_options.cursor` is a nested parameter",
        "resources.*.models: perseid names types after their schema, without resource namespaces: MessageSchedule is Schedule",
        "resources.users.list_schedules: `paginated: false`",
        "readme: ",
    ] {
        assert!(
            out.contains(&format!("  - {skipped}")),
            "{skipped}\n---\n{out}"
        );
    }
    let args = [
        "generate",
        "--no-format",
        "--out",
        "sdks",
        "typescript",
        "python",
    ];
    let (ok, out) = perseid(dir.path(), &args);
    assert!(ok, "{out}");
    let api = fs::read_to_string(dir.path().join("sdks/typescript/api.md")).unwrap();
    assert!(
        api.contains("client.users.get(userId: string)")
            && api.contains("client.messages.markAsArchived(")
            && api.contains("client.users.list(options?: UsersListOptions): PagePromise<")
            && !api.contains("/v1/notify"),
        "{api}"
    );
    let audiences =
        fs::read_to_string(dir.path().join("sdks/python/knockapi/api/audiences.py")).unwrap();
    assert!(
        audiences.contains("def add_members(") && !audiences.contains("def list_members("),
        "{audiences}"
    );
}

#[test]
fn init_from_a_stainless_config_written_for_another_spec_warns_and_keeps_its_operations() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/stainless-openapi.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let config = fs::read_to_string("tests/fixtures/stainless.yml").unwrap();
    let stale = config
        .replace(" /v1/", " /v2/")
        .replace("post /v2/notify", "post /v1/notify");
    fs::write(dir.path().join("stainless.yml"), stale).unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--from", "stainless.yml"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("! only 0 of the 13 endpoints of stainless.yml are operations of the spec"),
        "{out}"
    );
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(config.contains("exclude = [\"notify\"]\n"), "{config}");
}

#[test]
fn init_from_a_stainless_config_without_its_spec_lists_what_needs_one() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/stainless-legacy.yml",
        dir.path().join("openapi.stainless.yml"),
    )
    .unwrap();
    let args = [
        "init",
        "--from",
        "openapi.stainless.yml",
        "--sdks",
        "typescript,go,java",
    ];
    let (ok, out) = perseid(dir.path(), &args);
    assert!(ok, "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    for expected in [
        "spec = \"openapi.json\"\nname = \"OnebusawaySDK\"\nsdks = [\"typescript\", \"go\", \"java\"]\nbase_url = \"https://api.pugetsound.onebusaway.org\"\ntimeout = 120\n",
        "[context]\nenv_prefix = \"ONEBUSAWAY\"\n",
        "# page\npage = \"page\"\nitems = \"list\"\ntotal_pages = \"total_pages\"\n",
        "[typescript]\npackage = \"onebusaway-sdk\"\nrepo = \"OneBusAway/js-sdk\"\n",
        "[go]\npackage = \"onebusaway\"\nrepo = \"OneBusAway/go-sdk\"\nmodule = \"github.com/OneBusAway/go-sdk/v2\"\n",
        "[java]\npackage = \"org.onebusaway\"\n",
    ] {
        assert!(config.contains(expected), "{expected}\n---\n{config}");
    }
    for skipped in [
        "targets.python: left out by --sdks",
        "targets.kotlin: perseid generates no Kotlin SDK",
        "client_settings.default_retries.max_retries: perseid retries twice",
        "environments.sandbox: perseid clients have one default base URL",
        "query_settings.array_format: ",
        "pagination.next_url: perseid has no pagination by next-page URL",
        "resources: method names and the operations stainless.yml leaves out need the spec",
        "security_schemes: perseid reads them from the spec",
    ] {
        assert!(
            out.contains(&format!("  - {skipped}")),
            "{skipped}\n---\n{out}"
        );
    }
    let (ok, out) = perseid(dir.path(), &["init", "--from", "openapi.stainless.yml"]);
    assert!(!ok && out.contains("perseid.toml already exists"), "{out}");
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

    let marker = dir.path().join("rust/.perseid/generation.json");
    let recorded = fs::read_to_string(&marker).unwrap();
    assert!(
        recorded.starts_with(
            "{\n  \"generated\": \"this file is @generated by perseid\",\n  \"files\": \"sha256:"
        ),
        "{recorded}"
    );
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!models.join("stale.rs").exists());
    assert!(models.join("mine.rs").exists());
    assert_eq!(fs::read_to_string(&marker).unwrap(), recorded);

    let spec = dir.path().join("openapi.yaml");
    let text = fs::read_to_string(&spec).unwrap();
    let summarized = "operationId: list_pets\n      summary: Lists the pets\n";
    fs::write(&spec, text.replace("operationId: list_pets\n", summarized)).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "rust", "--no-format"]);
    assert!(ok && out.contains("~ .perseid/generation.json"), "{out}");
    assert_ne!(fs::read_to_string(&marker).unwrap(), recorded);
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
fn eject_lists_and_copies_single_files() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["eject", "go", "--list"]);
    assert!(
        ok && out.contains("templates/api_summary.go.jinja\n"),
        "{out}"
    );
    let (ok, out) = perseid(
        dir.path(),
        &["eject", "go", "templates/api_summary.go.jinja"],
    );
    assert!(ok, "{out}");
    let ejected = dir.path().join(".perseid/templates/go");
    assert_eq!(fs::read_dir(&ejected).unwrap().count(), 1, "{out}");
    let (ok, out) = perseid(dir.path(), &["eject", "go", "templates/nope.jinja"]);
    assert!(
        !ok && out.contains("`perseid eject go --list` lists them"),
        "{out}"
    );
}

#[test]
fn commands_find_perseid_toml_from_a_subfolder_and_inspect_one_language() {
    let dir = project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let (ok, out) = perseid(&dir.path().join("go"), &["inspect", "go"]);
    assert!(ok, "{out}");
    let api: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        api["resources"].is_object() || api["resources"].is_array(),
        "{out}"
    );
    let empty = tempfile::tempdir().unwrap();
    let (ok, out) = perseid(empty.path(), &["generate"]);
    assert!(
        !ok && out.contains("no perseid.toml in") && out.contains("its parents"),
        "{out}"
    );
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
    assert!(
        dir.path().join("rust/src/webhooks.rs").exists(),
        "the `webhooks` cargo feature is the opt-in in Rust"
    );
    assert!(!dir.path().join("go/webhooks.go").exists());

    let config = dir.path().join("perseid.toml");
    let toml = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        toml.replacen("\n[metadata]", "webhooks = true\n\n[metadata]", 1),
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
fn tests_are_generated_unless_left_out() {
    let dir = project_from("petstore.yaml", &["go", "java", "csharp"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for path in [
        "go/pets_test.go",
        "go/perseid_mock_test.go",
        "java/src/test/java/com/petstore/api/PetsTest.java",
        "csharp/Petstore.Tests/PetsTests.cs",
    ] {
        assert!(dir.path().join(path).exists(), "{path}");
    }
    let go = fs::read_to_string(dir.path().join("go/pets_test.go")).unwrap();
    assert!(
        go.contains(r#"expect(t, requests, "DELETE /pets/pet_id")"#),
        "{go}"
    );

    edit_config(dir.path(), |text| {
        text.replacen("[go]", "[go]\ntests = false", 1)
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        !dir.path().join("go/pets_test.go").exists(),
        "stale tests are removed"
    );
    assert!(
        dir.path()
            .join("csharp/Petstore.Tests/PetsTests.cs")
            .exists()
    );
}

#[test]
fn round_trips_are_opt_in() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let round_trips = [
        ("rust/tests/round_trips.rs", "rust/tests/round_trips.json"),
        (
            "typescript/tests/api/roundTrips.test.ts",
            "typescript/tests/api/round_trips.json",
        ),
        (
            "python/tests/test_round_trips.py",
            "python/tests/round_trips.json",
        ),
        ("go/round_trips_test.go", "go/testdata/round_trips.json"),
        (
            "java/src/test/java/com/petstore/api/RoundTripsTest.java",
            "java/src/test/resources/com/petstore/api/round_trips.json",
        ),
        (
            "csharp/Petstore.Tests/RoundTripsTests.cs",
            "csharp/Petstore.Tests/round_trips.json",
        ),
    ];
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for (test, data) in round_trips {
        assert!(!dir.path().join(test).exists(), "{test}");
        assert!(!dir.path().join(data).exists(), "{data}");
    }

    edit_config(dir.path(), |text| {
        text.replacen("\n[metadata]", "round_trips = true\n\n[metadata]", 1)
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for (test, data) in round_trips {
        assert!(dir.path().join(test).exists(), "{test}");
        let data = fs::read_to_string(dir.path().join(data)).unwrap();
        let samples: Value = serde_json::from_str(&data).unwrap();
        assert!(
            samples["models"]["Pet"][0]["json"]["name"].is_string(),
            "{data}"
        );
    }

    edit_config(dir.path(), |text| {
        text.replacen("[go]", "[go]\nround_trips = false", 1)
            .replacen("[java]", "[java]\ntests = false", 1)
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(out.contains("`round_trips = true` needs `tests`"), "{out}");
    for (test, data) in &round_trips[3..5] {
        assert!(!dir.path().join(test).exists(), "stale {test} is removed");
        assert!(!dir.path().join(data).exists(), "stale {data} is removed");
    }
    assert!(dir.path().join(round_trips[0].1).exists());
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
fn swagger_2_specs_are_converted_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore-swagger2.json",
        dir.path().join("swagger.json"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "typescript,python"]);
    assert!(ok, "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(
        config.contains("spec = \"swagger.json\"")
            && config.contains("base_url = \"https://petstore.swagger.io/v2\""),
        "{config}"
    );
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("note: swagger.json is Swagger 2.0, converted to OpenAPI 3 in memory"),
        "{out}"
    );
    let pets = fs::read_to_string(dir.path().join("typescript/src/api/petApi.ts")).unwrap();
    for call in [
        "setMultipartBody(form)",
        "setFormBody(",
        "setBody(PetSerializer.serialize(pet))",
        "setExplodedQueryParam(\"status\"",
    ] {
        assert!(pets.contains(call), "{call}: {pets}");
    }
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
    assert!(api.contains("public interface IPetsApi"), "{api}");
    assert!(
        api.contains("public async Task<Pet> RetrieveAsync("),
        "{api}"
    );
    assert!(
        api.contains("public Task<ApiResponse<Pet>> CreateAsync("),
        "{api}"
    );
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
        streaming.contains("public async Task<EventStream> RetrieveEventsStreamAsync("),
        "{streaming}"
    );
    assert!(
        streaming.contains("Task<EventStream<CompletionChunk>> CreateCompletionStreamAsync("),
        "{streaming}"
    );
    assert!(
        streaming.contains("(completionRequest with { Stream = true })"),
        "{streaming}"
    );
    assert!(
        streaming.contains("public sealed class StreamingUploadFileBody"),
        "{streaming}"
    );
    assert!(streaming.contains("Upload body,"), "{streaming}");
    let widgets = read("Api/WidgetsApi.cs");
    assert!(
        widgets.contains("public AsyncPager<WidgetsListPage, Widget> ListAsync("),
        "{widgets}"
    );
    assert!(
        widgets.contains("public string? NextCursor => Body.NextCursor;"),
        "{widgets}"
    );
    assert!(
        widgets.contains("(body, _) => body.NextCursor"),
        "{widgets}"
    );
    let client = read("FeaturesClient.cs");
    assert!(
        client.contains("public FeaturesApiKeys ApiKeys"),
        "{client}"
    );
    assert!(
        read("Api/RecordsApi.cs").contains(
            "request.Security =\n        [\n            [\"api_key_query\"],\n        ];"
        )
    );
    for entry in fs::read_dir(dir.path().join("csharp/Features/Api")).unwrap() {
        let code = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(
            !code.contains("\n\n\n") && !code.contains("{\n\n") && !code.contains("\n\n    }"),
            "stray blank lines before CSharpier:\n{code}"
        );
        assert!(
            !code.contains("\nTask") && !code.contains("\npublic async") && code.ends_with("}\n"),
            "broken indentation before CSharpier:\n{code}"
        );
    }
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

/// A project committed and pushed to a local `origin`, with a `bin` of the commands
/// `generate --pr` gets: git, logging the extra headers of its pushes to `push.log`.
#[cfg(unix)]
fn pull_request_project() -> tempfile::TempDir {
    let dir = project();
    let git = |args: &[&str]| git_in(dir.path(), args);
    git(&["init", "--quiet", "--initial-branch", "main"]);
    git(&["init", "--quiet", "--bare", "origin.git"]);
    let origin = dir.path().join("origin.git");
    git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    fs::write(
        dir.path().join(".gitignore"),
        "origin.git\nbin\nhome\n*.log\n",
    )
    .unwrap();
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
    let real = format!("{}/git", git(&["--exec-path"]));
    let headers = "$GIT_CONFIG_KEY_0=$GIT_CONFIG_VALUE_0|$GIT_CONFIG_KEY_1=$GIT_CONFIG_VALUE_1";
    executable(
        &bin.join("git"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in push) echo \"{headers}\" >> \"$LOG_DIR/push.log\" ;; esac\nexec {real} \"$@\"\n"
        ),
    );
    fs::create_dir(dir.path().join("home")).unwrap();
    dir
}

/// Pushes a generated Rust SDK to `main`, so pull requests update it rather than create it.
#[cfg(unix)]
fn with_generated_sdk(dir: &Path) {
    fs::create_dir_all(dir.join("rust/src")).unwrap();
    fs::write(
        dir.join("rust/src/lib.rs"),
        "// This file is @generated by perseid.\n",
    )
    .unwrap();
    let git = |args: &[&str]| git_in(dir, args);
    git(&["add", "rust"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "sdk",
    ]);
    git(&["push", "--quiet", "origin", "main"]);
}

/// What the fake GitHub API answers, besides creating pull requests.
#[cfg(unix)]
#[derive(Default)]
struct Answers {
    /// The open pull requests of the update branches: those with a `head` only for that
    /// branch, the others for any.
    listed: Vec<Value>,
    /// The `github-authentication-token-expiration` header.
    expiration: Option<String>,
    /// The error enabling auto-merge answers.
    merge_error: Option<&'static str>,
}

/// A fake GitHub API, accepting the token `test-token`, which logs each request as
/// `METHOD /path?query body`.
#[cfg(unix)]
fn fake_github(answers: Answers, log: Arc<Mutex<Vec<String>>>) -> u16 {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let target = parts.next().unwrap_or_default().to_owned();
            let (mut length, mut authorized) = (0, false);
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let Some((name, value)) = header.trim_end().split_once(':') else {
                    break;
                };
                match name.to_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "authorization" => authorized = value.trim() == "Bearer test-token",
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            log.lock()
                .unwrap()
                .push(format!("{method} {target} {body}").trim_end().to_owned());
            let path = target.split('?').next().unwrap();
            let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let pull = |number: &str| {
                let url = format!(
                    "https://github.com/{}/{}/pull/{number}",
                    segments[1], segments[2]
                );
                json!({ "number": number.parse::<u64>().unwrap(), "html_url": url, "node_id": format!("PR_{number}") })
            };
            let (status, reply) = match (method.as_str(), &segments[..]) {
                _ if !authorized => (401, json!({ "message": "Bad credentials" })),
                ("GET", ["repos", _, _, "pulls"]) => {
                    let head = target
                        .split_once("head=")
                        .map(|(_, h)| h.split('&').next().unwrap());
                    let listed: Vec<&Value> = answers
                        .listed
                        .iter()
                        .filter(|p| match (p["head"]["ref"].as_str(), head) {
                            (Some(branch), Some(head)) => head.ends_with(&format!(":{branch}")),
                            _ => true,
                        })
                        .collect();
                    (200, json!(listed))
                }
                ("POST", ["repos", _, _, "pulls"]) => (201, pull("2")),
                ("PATCH", ["repos", _, _, "pulls", number]) => (200, pull(number)),
                ("POST", ["repos", _, _, "labels"]) => {
                    (422, json!({ "message": "Validation Failed" }))
                }
                ("POST", ["repos", _, _, "issues", _, "labels"]) => (200, json!([])),
                ("POST", ["repos", _, _, "issues", _, "comments"]) => (201, json!({})),
                ("DELETE", ["repos", _, _, "git", "refs", "heads", ..]) => (204, Value::Null),
                ("PUT", ["repos", _, _, "pulls", _, "merge"]) => (200, json!({ "merged": true })),
                ("POST", ["repos", _, _, "actions", "workflows", _, "dispatches"]) => {
                    (204, Value::Null)
                }
                ("POST", ["graphql"]) => match answers.merge_error {
                    Some(error) => (200, json!({ "errors": [{ "message": error }] })),
                    None => (200, json!({ "data": {} })),
                },
                _ => (404, json!({ "message": "Not Found" })),
            };
            let text = match reply {
                Value::Null => String::new(),
                reply => reply.to_string(),
            };
            let expiration = match &answers.expiration {
                Some(date) => format!("github-authentication-token-expiration: {date}\r\n"),
                None => String::new(),
            };
            let _ = write!(
                &stream,
                "HTTP/1.1 {status} X\r\n{expiration}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    port
}

/// What `generate rust --pr` printed and asked of the fake GitHub API.
#[cfg(unix)]
struct PullRequestRun {
    ok: bool,
    output: String,
    calls: Vec<String>,
}

#[cfg(unix)]
impl PullRequestRun {
    /// The JSON bodies of the `method` requests to paths ending with `path`.
    fn requests(&self, method: &str, path: &str) -> Vec<Value> {
        self.calls
            .iter()
            .filter_map(|call| {
                let (verb, rest) = call.split_once(' ')?;
                let (target, body) = rest.split_once(' ').unwrap_or((rest, ""));
                let matches = verb == method && target.split('?').next()?.ends_with(path);
                matches.then(|| serde_json::from_str(body).unwrap_or(Value::Null))
            })
            .collect()
    }

    fn request(&self, method: &str, path: &str) -> Value {
        let found = self.requests(method, path);
        assert_eq!(found.len(), 1, "{method} …{path} in {:#?}", self.calls);
        found.into_iter().next().unwrap()
    }
}

/// Runs `generate rust --pr` against a fake GitHub API answering `answers`, with no `gh` on the
/// PATH, no git identity configured, GH_TOKEN set to `test-token`, and `env`.
#[cfg(unix)]
fn run_pr(dir: &Path, args: &[&str], answers: Answers, env: &[(&str, &str)]) -> PullRequestRun {
    let calls = Arc::default();
    let port = fake_github(answers, Arc::clone(&calls));
    let output = Command::new(env!("CARGO_BIN_EXE_perseid"))
        .args(["generate", "rust", "--pr", "--no-format"])
        .args(args)
        .current_dir(dir)
        .env("PATH", dir.join("bin"))
        .env("PERSEID_GITHUB_API", format!("http://127.0.0.1:{port}"))
        .env("GH_TOKEN", "test-token")
        .env("LOG_DIR", dir)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("GITHUB_EVENT_BEFORE")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("CI")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    PullRequestRun {
        ok: output.status.success(),
        output: String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr),
        calls: calls.lock().unwrap().clone(),
    }
}

#[cfg(unix)]
fn generate_pr(dir: &Path, bump: &str) -> PullRequestRun {
    generate_pr_with(dir, &["--bump", bump])
}

#[cfg(unix)]
fn generate_pr_with(dir: &Path, args: &[&str]) -> PullRequestRun {
    let run = run_pr(dir, args, Answers::default(), &[]);
    assert!(run.ok, "{}", run.output);
    run
}

/// The title of the pull request `run` opened.
#[cfg(unix)]
fn opened_title(run: &PullRequestRun) -> String {
    let created = run.request("POST", "/pulls");
    assert_eq!(created["head"], "perseid/update", "{created}");
    created["title"].as_str().unwrap().to_owned()
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
    let dir = pull_request_project();
    with_generated_sdk(dir.path());
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
        "#!/bin/sh\necho \"$@\" >> \"$LOG_DIR/oasdiff.log\"\necho '[{\"id\": \"api-path-removed-without-deprecation\", \"text\": \"api path removed without deprecation\", \"level\": 3, \"operation\": \"DELETE\", \"path\": \"/pets/{id}\"}, {\"id\": \"endpoint-added\", \"text\": \"endpoint added\", \"level\": 1, \"operation\": \"GET\", \"path\": \"/toys\"}]'\n",
    );

    let run = generate_pr_with(dir.path(), &[]);
    assert!(
        opened_title(&run).starts_with("feat(api)!: update SDKs to"),
        "{:#?}",
        run.calls
    );
    let body = &run.request("POST", "/pulls")["body"];
    let entry = "`DELETE /pets/{id}`: api path removed without deprecation";
    assert!(
        body.as_str().unwrap().contains(&format!(
            "### Changelog\n\n- ⚠️ {entry}\n- add `GET /toys`\n"
        )),
        "{body}"
    );
    let message = git_in(
        &dir.path().join("origin.git"),
        &["log", "-1", "--format=%B", "perseid/update"],
    );
    assert!(
        message.starts_with("feat(api)!: update SDKs to")
            && message.ends_with(&format!(
                "\n\nBEGIN_NESTED_COMMIT\nfeat(api)!: {entry}\nEND_NESTED_COMMIT\n\
                 BEGIN_NESTED_COMMIT\nfeat(api): add `GET /toys`\nEND_NESTED_COMMIT"
            )),
        "{message}"
    );
    let runs = fs::read_to_string(dir.path().join("oasdiff.log")).unwrap();
    assert_eq!(runs.lines().count(), 1, "{runs}");
    assert!(
        runs.starts_with("changelog --format json --severity-levels ")
            && runs.trim_end().ends_with("/openapi.yaml"),
        "{runs}"
    );
}

#[cfg(unix)]
#[test]
fn auto_bumps_without_a_previous_spec_are_minor() {
    let dir = pull_request_project();
    let run = generate_pr_with(dir.path(), &[]);
    assert!(opened_title(&run).starts_with("feat(api): update SDKs to"));
    let body = run.request("POST", "/pulls")["body"].to_string();
    assert!(!body.contains("API changes"), "{body}");
}

#[cfg(unix)]
#[test]
fn pull_requests_can_auto_merge_and_dispatch_ci() {
    let dir = pull_request_project();
    let run = generate_pr_with(
        dir.path(),
        &[
            "--bump",
            "patch",
            "--auto-merge",
            "--dispatch",
            "ci.yml lint.yml",
        ],
    );
    assert!(run.output.contains("/pull/2\n"), "{}", run.output);
    assert_eq!(
        run.request("POST", "/origin/labels"),
        json!({
            "name": "perseid:auto-release",
            "color": "6f42c1",
            "description": "Auto-merge the release PR this change leads to",
        })
    );
    assert_eq!(
        run.request("POST", "/issues/2/labels"),
        json!({ "labels": ["perseid:auto-release"] })
    );
    let merge = run.request("POST", "/graphql");
    assert_eq!(merge["variables"], json!({ "id": "PR_2" }));
    let query = merge["query"].as_str().unwrap();
    assert!(
        query.contains("enablePullRequestAutoMerge") && query.contains("mergeMethod: SQUASH"),
        "{query}"
    );
    for workflow in ["ci.yml", "lint.yml"] {
        assert_eq!(
            run.request("POST", &format!("/actions/workflows/{workflow}/dispatches")),
            json!({ "ref": "perseid/update" })
        );
    }
}

#[cfg(unix)]
#[test]
fn auto_merge_warns_when_the_repository_disallows_it_and_merges_clean_pull_requests() {
    let dir = pull_request_project();
    let answers = Answers {
        merge_error: Some("Pull request Auto merge is not allowed for this repository"),
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &["--auto-merge"], answers, &[]);
    assert!(run.ok, "{}", run.output);
    assert!(
        run.output.contains("doesn't allow auto-merge") && run.output.contains("Allow auto-merge"),
        "{}",
        run.output
    );
    assert!(run.requests("PUT", "/merge").is_empty());

    let answers = Answers {
        merge_error: Some("Pull request Pull request is in clean status"),
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &["--auto-merge"], answers, &[]);
    assert!(run.ok, "{}", run.output);
    assert_eq!(
        run.request("PUT", "/pulls/2/merge"),
        json!({ "merge_method": "squash" })
    );
}

/// `days` from now, as GitHub writes token expirations.
#[cfg(unix)]
fn expiration_in(days: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (days, time) = ((now / 86400 + days) as i64 + 719_468, now % 86400);
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year}-{month:02}-{day:02} {:02}:{:02}:00 UTC",
        time / 3600,
        time % 3600 / 60
    )
}

#[cfg(unix)]
#[test]
fn tokens_expiring_within_30_days_are_warned_about() {
    let dir = pull_request_project();
    let soon = expiration_in(12);
    let answers = Answers {
        expiration: Some(soon.clone()),
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &[], answers, &[("GITHUB_ACTIONS", "true")]);
    assert!(run.ok, "{}", run.output);
    let day = soon.split(' ').next().unwrap();
    let warning =
        format!("::warning::the GitHub token expires on {day}: renew the SDK_GITHUB_TOKEN");
    assert_eq!(run.output.matches(&warning).count(), 1, "{}", run.output);

    let answers = Answers {
        expiration: Some(expiration_in(45)),
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &[], answers, &[]);
    assert!(run.ok && !run.output.contains("expires"), "{}", run.output);
}

#[cfg(unix)]
#[test]
fn pull_requests_in_actions_need_a_token() {
    let dir = pull_request_project();
    let env = [("GITHUB_ACTIONS", "true"), ("GH_TOKEN", "")];
    let run = run_pr(dir.path(), &["--bump", "patch"], Answers::default(), &env);
    assert!(!run.ok, "{}", run.output);
    assert!(
        run.output.contains("add the SDK_GITHUB_TOKEN secret"),
        "{}",
        run.output
    );
    assert!(run.calls.is_empty(), "{:#?}", run.calls);
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
    let dir = pull_request_project();
    with_generated_sdk(dir.path());
    let answers = Answers {
        listed: vec![json!({
            "number": 1,
            "html_url": "https://github.com/acme/petstore-sdks/pull/1",
            "node_id": "PR_1",
            "title": "feat(api)!: update SDKs to Petstore 1",
            "body": "<!-- perseid:api-changes {\"changes\": [{\"breaking\": false, \"operation\": \"GET /toys\", \"id\": \"endpoint-added\", \"text\": \"endpoint added\"}], \"omitted\": 0} -->",
        })],
        expiration: Some("2999-01-01 00:00:00 UTC".into()),
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &["--bump", "patch"], answers, &[]);
    assert!(run.ok, "{}", run.output);
    assert!(!run.output.contains("expires"), "{}", run.output);
    let owner = dir.path().file_name().unwrap().to_str().unwrap();
    let listed = &run.calls[0];
    assert!(
        listed.starts_with(&format!(
            "GET /repos/{owner}/origin/pulls?head={owner}:perseid/update&state=open"
        )),
        "{listed}"
    );
    let edited = run.request("PATCH", "/pulls/1");
    assert!(
        edited["title"]
            .as_str()
            .unwrap()
            .starts_with("feat(api)!: update SDKs to"),
        "{edited}"
    );
    let body = edited["body"].as_str().unwrap();
    assert!(
        body.contains("### Changelog\n\n- add `GET /toys`\n"),
        "{body}"
    );
    let message = git_in(
        &dir.path().join("origin.git"),
        &["log", "-1", "--format=%B", "perseid/update"],
    );
    assert!(message.contains("feat(api): add `GET /toys`"), "{message}");
    assert!(run.requests("POST", "/pulls").is_empty());
    assert!(run.output.contains("/pull/1\n"), "{}", run.output);
}

/// The open update pull request of the rust SDK alone, `perseid/update-rust`.
#[cfg(unix)]
fn rust_pull_request() -> Answers {
    Answers {
        listed: vec![json!({
            "number": 1,
            "html_url": "https://github.com/acme/petstore-sdks/pull/1",
            "node_id": "PR_1",
            "title": "feat(api): update the rust SDK to Petstore 1",
            "head": { "ref": "perseid/update-rust" },
        })],
        ..Answers::default()
    }
}

#[cfg(unix)]
#[test]
fn updates_of_several_sdks_share_a_pull_request() {
    let dir = pull_request_project();
    let run = generate_pr_with(dir.path(), &["go", "--bump", "minor"]);
    let created = run.request("POST", "/pulls");
    assert_eq!(created["head"], "perseid/update", "{created}");
    let title = created["title"].as_str().unwrap();
    assert!(title.starts_with("feat(api): update SDKs to"), "{title}");
    let origin = dir.path().join("origin.git");
    let files = git_in(&origin, &["ls-tree", "-r", "--name-only", "perseid/update"]);
    for sdk in ["rust/", "go/"] {
        assert!(
            files.lines().any(|f| f.starts_with(sdk)),
            "{sdk} not in {files}"
        );
    }
}

#[cfg(unix)]
#[test]
fn updates_too_large_for_release_please_get_a_pull_request_per_sdk() {
    let dir = pull_request_project();
    let spec = dir.path().join("openapi.yaml");
    let changed = fs::read_to_string(&spec)
        .unwrap()
        .replace("title:", "description: Pets\n  title:");
    fs::write(&spec, changed).unwrap();
    let answers = Answers {
        listed: vec![json!({
            "number": 1,
            "html_url": "https://github.com/acme/petstore-sdks/pull/1",
            "node_id": "PR_1",
            "title": "feat(api): update SDKs to Petstore 1",
            "head": { "ref": "perseid/update" },
        })],
        ..Answers::default()
    };
    let args = [
        "go",
        "--bump",
        "minor",
        "--auto-merge",
        "--dispatch",
        "ci.yml",
    ];
    let run = run_pr(dir.path(), &args, answers, &[("PERSEID_SPLIT_ABOVE", "10")]);
    assert!(run.ok, "{}", run.output);

    let created = run.requests("POST", "/pulls");
    let heads: Vec<_> = created
        .iter()
        .map(|p| p["head"].as_str().unwrap())
        .collect();
    assert_eq!(
        heads,
        ["perseid/update-rust", "perseid/update-go"],
        "{created:#?}"
    );
    for (pull, name) in created.iter().zip(["rust", "go"]) {
        let title = pull["title"].as_str().unwrap();
        let expected = format!("feat(api): update the {name} SDK to Petstore");
        assert!(title.starts_with(&expected), "{title}");
    }
    assert_eq!(run.requests("POST", "/issues/2/labels").len(), 2);
    let dispatched = run.requests("POST", "/actions/workflows/ci.yml/dispatches");
    let refs = [
        json!({ "ref": "perseid/update-rust" }),
        json!({ "ref": "perseid/update-go" }),
    ];
    assert_eq!(dispatched, refs);

    let origin = |args: &[&str]| git_in(&dir.path().join("origin.git"), args);
    for (name, other) in [("rust", "go"), ("go", "rust")] {
        let branch = format!("perseid/update-{name}");
        let files = origin(&["ls-tree", "-r", "--name-only", &branch]);
        assert!(
            files.lines().any(|f| f.starts_with(&format!("{name}/"))),
            "{files}"
        );
        assert!(
            !files.lines().any(|f| f.starts_with(&format!("{other}/"))),
            "{files}"
        );
        let spec = origin(&["show", &format!("{branch}:openapi.yaml")]);
        assert!(
            spec.contains("description: Pets"),
            "the spec rides along: {spec}"
        );
    }

    let comment = run.request("POST", "/issues/1/comments");
    let text = comment["body"].as_str().unwrap();
    assert!(
        text.starts_with("Replaced by one pull request per SDK"),
        "{text}"
    );
    assert!(text.contains("/pull/2"), "{text}");
    assert_eq!(
        run.request("PATCH", "/pulls/1"),
        json!({ "state": "closed" })
    );
    let deleted = |c: &&String| c.starts_with("DELETE") && c.ends_with("/heads/perseid/update");
    assert!(run.calls.iter().any(|c| deleted(&c)), "{:#?}", run.calls);
}

#[cfg(unix)]
#[test]
fn open_pull_requests_of_an_sdk_keep_the_update_split() {
    let dir = pull_request_project();
    let run = run_pr(
        dir.path(),
        &["go", "--bump", "minor"],
        rust_pull_request(),
        &[],
    );
    assert!(run.ok, "{}", run.output);
    let edited = run.request("PATCH", "/pulls/1");
    let title = edited["title"].as_str().unwrap();
    assert!(
        title.starts_with("feat(api): update the rust SDK to"),
        "{title}"
    );
    let created = run.request("POST", "/pulls");
    assert_eq!(created["head"], "perseid/update-go", "{created}");
}

#[cfg(unix)]
#[test]
fn a_spec_change_leaving_every_sdk_alone_gets_one_pull_request() {
    let dir = pull_request_project();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let git = |args: &[&str]| git_in(dir.path(), args);
    git(&["add", "--all"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "sdks",
    ]);
    git(&["push", "--quiet", "origin", "main"]);
    let spec = dir.path().join("openapi.yaml");
    let commented = format!("# Pets\n{}", fs::read_to_string(&spec).unwrap());
    fs::write(&spec, commented).unwrap();

    // Whether the update is split or not: the pull request of an SDK left alone is closed.
    for (answers, split) in [(Answers::default(), false), (rust_pull_request(), true)] {
        let run = run_pr(dir.path(), &["go", "--bump", "patch"], answers, &[]);
        assert!(run.ok, "{}", run.output);
        let created = run.request("POST", "/pulls");
        assert_eq!(created["head"], "perseid/update", "{created}");
        let title = created["title"].as_str().unwrap();
        assert!(title.starts_with("fix(api): update SDKs to"), "{title}");
        let origin = dir.path().join("origin.git");
        let changed = git_in(&origin, &["diff", "--name-only", "main", "perseid/update"]);
        assert_eq!(changed, "openapi.yaml");
        let deleted = run
            .calls
            .iter()
            .any(|c| c.starts_with("DELETE") && c.contains("/git/refs/heads/perseid/update-rust"));
        assert_eq!(deleted, split, "{:#?}", run.calls);
        if split {
            let comment = run.request("POST", "/issues/1/comments");
            let text = comment["body"].as_str().unwrap();
            assert!(
                text.starts_with("Closed: the base branch already holds"),
                "{text}"
            );
            assert_eq!(
                run.request("PATCH", "/pulls/1"),
                json!({ "state": "closed" })
            );
        } else {
            assert!(
                run.requests("PATCH", "/pulls/1").is_empty(),
                "{:#?}",
                run.calls
            );
            assert!(run.requests("POST", "/issues/1/comments").is_empty());
        }
    }
}

#[cfg(unix)]
#[test]
fn first_generations_list_no_api_changes() {
    let dir = pull_request_project();
    let answers = Answers {
        listed: vec![json!({
            "number": 1,
            "html_url": "https://github.com/acme/petstore-sdks/pull/1",
            "node_id": "PR_1",
            "title": "feat(api): update SDKs to Petstore 1",
            "body": "<!-- perseid:api-changes {\"changes\": [{\"breaking\": false, \"operation\": \"GET /toys\", \"id\": \"endpoint-added\", \"text\": \"endpoint added\"}], \"omitted\": 0} -->",
        })],
        ..Answers::default()
    };
    let run = run_pr(dir.path(), &["--bump", "minor"], answers, &[]);
    assert!(run.ok, "{}", run.output);
    let body = run.request("PATCH", "/pulls/1")["body"].to_string();
    assert!(
        !body.contains("Changelog") && !body.contains("toys"),
        "{body}"
    );
    let message = git_in(
        &dir.path().join("origin.git"),
        &["log", "-1", "--format=%B", "perseid/update"],
    );
    assert!(!message.contains("NESTED"), "{message}");
}

#[cfg(unix)]
#[test]
fn pull_requests_leave_the_checkout_alone() {
    let dir = pull_request_project();
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
    let persisted = "AUTHORIZATION: basic checkout-token";
    git(&["config", "http.https://github.com/.extraheader", persisted]);

    let run = generate_pr(dir.path(), "minor");
    assert!(opened_title(&run).starts_with("feat(api): update SDKs to"));
    assert_eq!(run.request("POST", "/pulls")["base"], "main");
    let pushed = fs::read_to_string(dir.path().join("push.log")).unwrap();
    let header = "http.https://github.com/.extraheader";
    assert_eq!(
        pushed,
        format!("{header}=|{header}=AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg==\n")
    );

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
    let dir = pull_request_project();
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

    let run = generate_pr(dir.path(), "minor");
    assert!(opened_title(&run).starts_with("feat(api): update SDKs to"));
    assert_eq!(run.request("POST", "/rust-sdk/pulls")["base"], "trunk");
    let files = git_in(&remote, &["ls-tree", "-r", "--name-only", "perseid/update"]);
    for file in ["README.md", "Cargo.toml", "src/lib.rs"] {
        assert!(files.lines().any(|f| f == file), "{file} not in {files}");
    }
    assert!(!files.contains("openapi.yaml"), "{files}");

    let spec = dir.path().join("openapi.yaml");
    let changed = fs::read_to_string(&spec)
        .unwrap()
        .replace("title:", "description: Pets\n  title:");
    fs::write(&spec, changed).unwrap();
    let run = generate_pr(dir.path(), "minor");
    assert_eq!(
        run.request("POST", "/rust-sdk/pulls")["base"],
        "trunk",
        "the detached checkout of a second run targets the default branch too"
    );
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
    assert!(checkout.join("release-please-config.json").exists());
    assert!(
        !checkout.join(".github/workflows/sdk-release.yml").exists(),
        "CI tokens can't write workflows: `perseid sync` commits it"
    );
    assert!(
        out.contains("sdk-release.yml is missing: run `perseid sync`"),
        "{out}"
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
            "file:///nonexistent/acme/petstore-go doesn't exist yet: create it on GitHub, or \
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
  /entries:
    get:
      operationId: list_entries
      parameters:
        - { name: offset, in: query, schema: { type: integer } }
      responses:
        '200': { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/EntryPage' } } } }
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
    EntryPage:
      type: object
      required: [entries, labels, total]
      properties:
        entries: { type: array, items: { $ref: '#/components/schemas/Item' } }
        labels: { type: array, items: { type: string } }
        total: { type: integer }
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
    let pagination = &operation(&api, "list_items")["pagination"];
    assert_eq!(pagination["style"], "page");
    assert_eq!(pagination["item_schema"], "Item");
    assert_eq!(pagination["first_page"], 0);
    assert!(!out.contains("\"total_pages\""), "{out}");

    let (ok, out) = inspect_with("[pagination]\npage = \"page\"\noperations = [\"get_item\"]\n");
    assert!(!ok && out.contains("no `page` query parameter"), "{out}");
    let (ok, out) = inspect_with("[pagination]\ncursor = \"page\"\n");
    assert!(ok && !out.contains("\"pagination\""), "{out}");
}

#[test]
fn pagination_rules_use_the_response_values_it_has() {
    let pagination = |out: &str, id: &str| {
        let api: serde_json::Value = serde_json::from_str(out).unwrap();
        operation(&api, id)["pagination"].clone()
    };
    let (ok, out) = inspect_with(
        "[pagination]\noffset = \"offset\"\nhas_more = \"has_more\"\ntotal = \"total\"\n",
    );
    assert!(ok, "{out}");
    let entries = pagination(&out, "list_entries");
    assert_eq!(entries["items"], serde_json::json!(["entries"]));
    assert_eq!(entries["total"], serde_json::json!(["total"]));
    assert!(entries.get("has_more").is_none(), "{entries}");

    let (ok, out) = inspect_with("[pagination]\npage = \"page\"\ntotal_pages = \"meta.nope\"\n");
    assert!(ok, "{out}");
    assert!(
        pagination(&out, "list_items").get("total_pages").is_none(),
        "{out}"
    );

    let scoped = "[pagination]\noffset = \"offset\"\nhas_more = \"has_more\"\noperations = [\"list_entries\"]\n";
    let (ok, out) = inspect_with(scoped);
    assert!(!ok && out.contains("has no `has_more` property"), "{out}");
}

#[test]
fn stripe_style_lists_paginate_without_a_rule() {
    let list = |id: &str, cursor: &str, has_more: bool, extra: &str| {
        let more = if has_more {
            ", has_more: { type: boolean }"
        } else {
            ""
        };
        format!(
            r#"
  /{id}:
    get:
      operationId: {id}
      {extra}
      parameters:
        - {{ name: {cursor}, in: query, schema: {{ type: string }} }}
        - {{ name: limit, in: query, schema: {{ type: integer }} }}
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [data]
                properties: {{ data: {{ type: array, items: {{ $ref: '#/components/schemas/Item' }} }}{more} }}"#
        )
    };
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: Shop, version: \"1\" }}\npaths:{}{}{}{}\ncomponents:\n  \
         schemas:\n    Item: {{ type: object, required: [id], properties: {{ id: {{ type: string }} }} }}\n",
        list("customers", "starting_after", true, ""),
        list("completions", "after", true, ""),
        list("events", "starting_after", false, ""),
        list("invoices", "starting_after", true, "x-pagination: false"),
    );
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let config = "spec = \"openapi.yaml\"\nname = \"Shop\"\nsdks = [\"go\"]\n";
    fs::write(dir.path().join("perseid.toml"), config).unwrap();
    let (ok, out) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{out}");
    let api: serde_json::Value = serde_json::from_str(&out).unwrap();
    let customers = &operation(&api, "customers")["pagination"];
    assert_eq!(customers["style"], "cursor");
    assert_eq!(customers["param"], "starting_after");
    assert_eq!(customers["item_cursor"], "id");
    assert_eq!(customers["has_more"], serde_json::json!(["has_more"]));
    assert!(customers.get("before").is_none(), "{customers}");
    for id in ["completions", "events", "invoices"] {
        assert!(operation(&api, id).get("pagination").is_none(), "{id}");
    }

    let rule = "\n[[pagination]]\ncursor = \"after\"\nitem_cursor = \"id\"\nbefore = \"limit\"\noperations = [\"completions\"]\n";
    fs::write(dir.path().join("perseid.toml"), format!("{config}{rule}")).unwrap();
    let (ok, out) = perseid(dir.path(), &["inspect"]);
    assert!(
        !ok && out.contains("must be optional string query parameters"),
        "{out}"
    );

    let config = format!("detect_pagination = false\n{config}");
    fs::write(dir.path().join("perseid.toml"), config).unwrap();
    let (ok, out) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{out}");
    let api: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(operation(&api, "customers").get("pagination").is_none());
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
    let warning = "schema `PaymentMethodData`: inline schema in oneOf must have discriminator enum value, \
                   so the schema is typed as an untyped JSON value";
    assert!(out.contains(warning), "missing `{warning}` in:\n{out}");
    let charge = fs::read_to_string(dir.path().join("go/charge.go")).unwrap();
    assert!(
        charge.contains("u.OfCustomer != nil && u.OfCustomer.ID != nil"),
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
    assert_eq!(list["pagination"]["param"], "starting_after");
    assert_eq!(list["pagination"]["before"], "ending_before");
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
        out.contains("decoded as their best-matching variant: 3"),
        "{out}"
    );
    assert!(!out.contains("left as untyped JSON"), "{out}");
    let unions = read("object_unions");
    for text in [
        "pubenumObjectUnionsAccount",
        "has(&value,\"deleted\",Some(serde_json::json!(true)))",
        "pubenumObjectUnionsDocument",
        "pubraw_account:Option<serde_json::Value>",
    ] {
        assert!(unions.contains(text), "no `{text}` in {unions}");
    }
    assert!(read("document").contains("ranked(&value,"));

    edit_config(dir.path(), |text| {
        format!("untagged_unions = \"json\"\n{text}")
    });
    let out = generate();
    assert!(
        out.contains("decoded as their best-matching variant: 1"),
        "{out}"
    );
    assert!(out.contains("left as untyped JSON: 2"), "{out}");
    assert!(read("document").contains("pubtypeDocument=serde_json::Value;"));

    edit_config(dir.path(), |text| {
        text.replace("[rust]\n", "[rust]\nuntagged_unions = \"best-match\"\n")
    });
    assert!(generate().contains("decoded as their best-matching variant: 3"));

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
        out.matches("decoded as their best-matching variant")
            .count(),
        1,
        "warnings are printed once for all languages: {out}"
    );
    assert!(
        !out.contains("MapOrAddress"),
        "a map or an object is a typed union: {out}"
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
    assert!(read("python/torture/api/widgets.py").contains("-> builtins.list[Widget]:"));
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
    Token: { type: string }
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
        - { name: ids, in: query, style: pipeDelimited, explode: false, schema: { type: array, items: { type: string } } }
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
    let line = "schema `Clash`: `@type` and `type` both become the identifier `type`";
    assert!(out.contains(line), "missing `{line}` in:\n{out}");
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
        thing().contains("count: bigint;")
            && thing().contains("BigInt(decodeInteger(json[\"count\"], path, \"count\"))"),
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
        ("loginUser", "login"),
        ("markdown/render-raw", "render_raw"),
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
    assert!(
        config.contains("\n[typescript]\npackage = \"petstore\"\n")
            && config.contains("\n[java]\npackage = \"com.example.pets.petstore\"\n"),
        "each SDK names its package: {config}"
    );
    for sdk in ["rust", "typescript", "python", "java", "csharp"] {
        let license = read(&format!("{sdk}/LICENSE"));
        assert!(
            license.starts_with("MIT License\n\nCopyright (c) 20")
                && license.contains(" Pet Team\n"),
            "{license}"
        );
    }
    fs::write(dir.path().join("rust/LICENSE"), "Mine\n").unwrap();
    fs::remove_file(dir.path().join("python/LICENSE")).unwrap();
    fs::write(dir.path().join("python/LICENSE.md"), "Mine\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert_eq!(read("rust/LICENSE"), "Mine\n");
    assert!(!dir.path().join("python/LICENSE").exists());
}

#[test]
fn init_takes_the_license_and_comments_the_metadata_it_lacks() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let init = ["init", "--sdks", "go", "--license", "Apache-2.0"];
    let (ok, out) = perseid(dir.path(), &init);
    assert!(ok, "{out}");
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    assert!(
        config.contains("\n[metadata]\n# description = \"Petstore API client\"\nlicense = \"Apache-2.0\"\n# homepage = \"https://example.com\"\n")
            && config.ends_with("\n[go]\npackage = \"petstore\"\n"),
        "{config}"
    );
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let license = fs::read_to_string(dir.path().join("go/LICENSE")).unwrap();
    assert!(license.contains("Apache License\n"), "{license}");

    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/petstore.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let (ok, out) = perseid(
        dir.path(),
        &["init", "--sdks", "go", "--license", "\"MIT\""],
    );
    assert!(
        !ok && out.contains("isn't an SPDX license expression"),
        "{out}"
    );
    assert!(!dir.path().join("perseid.toml").exists());
    let (ok, out) = perseid(dir.path(), &["init", "--sdks", "go"]);
    assert!(
        ok && out.contains("! no license: the SDKs get no LICENSE"),
        "{out}"
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
    let pagination = read("pagination.rs");
    assert!(
        pagination.contains("futures_core::Stream for Paginator"),
        "{pagination}"
    );
    let api = read("api/mod.rs");
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
        text.replace("[metadata]", "timeout = 15\nwebhooks = true\n[metadata]")
    });
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("typescript/src").join(path)).unwrap();
    let charge = read("models/charge.ts");
    for decl in [
        "customer: string | Customer | null;",
        "amount?: number | \"\" | undefined;",
        "shipping?: ChargeShipping | \"\" | undefined;",
        "source?: string | Customer | UploadModel | undefined;",
    ] {
        assert!(charge.contains(decl), "no `{decl}` in {charge}");
    }
    assert!(
        charge.contains(
            "isJsonObject(json[\"customer\"]) ? CustomerSerializer.parse(json[\"customer\"], decodePath(path, \"customer\"))"
        ),
        "{charge}"
    );
    let index = read("index.ts");
    for text in [
        "const DEFAULT_TIMEOUT_MS = 15000;",
        "parseError: ErrorModelSerializer.parse,",
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
    assert!(
        circle.contains("type: t.Literal[\"circle\"] = \"circle\""),
        "{circle}"
    );
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
    assert!(read("exceptions/PetstoreException.java").contains("extends RuntimeException"));
    assert!(read("exceptions/ApiException.java").contains("extends PetstoreException"));
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

const LANGUAGES: [&str; 6] = ["rust", "typescript", "python", "go", "java", "csharp"];

#[test]
fn readme_examples_build_lists_and_nested_models_in_every_language() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let spec = r##"
openapi: 3.1.0
info: { title: Shop, version: "1" }
servers: [{ url: https://x.example.com }]
paths:
  /orders:
    post:
      operationId: create_order
      tags: [orders]
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: '#/components/schemas/OrderCreate' } } }
      responses:
        '200': { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/OrderCreate' } } } }
components:
  schemas:
    OrderCreate:
      type: object
      required: [tags, lines, shipping]
      properties:
        tags: { type: array, items: { type: string } }
        lines: { type: array, items: { $ref: '#/components/schemas/Line' } }
        shipping: { $ref: '#/components/schemas/Address' }
    Line:
      type: object
      required: [sku, quantity, size]
      properties:
        sku: { type: string }
        quantity: { type: integer, format: int64 }
        size: { type: string, enum: [small, large] }
        parent: { $ref: '#/components/schemas/Line' }
    Address:
      type: object
      required: [city]
      properties:
        city: { type: string, example: Paris }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let calls = [
        (
            "rust",
            "OrderCreate::new(vec![Line::new(1, \"small\".into(), \"sku\")], Address::new(\"Paris\"), vec![\"tags\".to_owned()])",
        ),
        (
            "typescript",
            "{ lines: [{ quantity: 1, size: \"small\", sku: \"sku\" }], shipping: { city: \"Paris\" }, tags: [\"tags\"] }",
        ),
        (
            "python",
            "lines=[Line(quantity=1, size=LineSize(\"small\"), sku=\"sku\")], shipping=Address(city=\"Paris\"), tags=[\"tags\"]",
        ),
        (
            "go",
            "petstore.OrderCreate{Lines: []petstore.Line{{Quantity: 1, Size: petstore.LineSize(\"small\"), Sku: \"sku\"}}, Shipping: petstore.Address{City: \"Paris\"}, Tags: []string{\"tags\"}}",
        ),
        (
            "java",
            "OrderCreate.builder().lines(java.util.List.of(Line.builder().quantity(1L).size(com.petstore.models.LineSize.of(\"small\")).sku(\"sku\").build())).shipping(Address.builder().city(\"Paris\").build()).tags(java.util.List.of(\"tags\")).build()",
        ),
        (
            "csharp",
            "new OrderCreate { Lines = [new Line { Quantity = 1, Size = \"small\", Sku = \"sku\" }], Shipping = new Address { City = \"Paris\" }, Tags = [\"tags\"] }",
        ),
    ];
    let imports = [
        (
            "rust",
            "use petstore::models::{Address, Line, OrderCreate};",
        ),
        (
            "python",
            "from petstore.models import Address, Line, LineSize\n",
        ),
        (
            "java",
            "import com.petstore.models.Address;\nimport com.petstore.models.Line;\n",
        ),
    ];
    for (language, needle) in calls.iter().chain(&imports) {
        let readme = fs::read_to_string(dir.path().join(language).join("README.md")).unwrap();
        assert!(
            readme.contains(needle),
            "no `{needle}` in the {language} {readme}"
        );
    }
}

#[cfg(unix)]
#[test]
fn docs_data_names_and_calls_every_operation_in_every_sdk() {
    let dir = project_from("petstore.yaml", &LANGUAGES);
    let spec = fs::read_to_string(dir.path().join("openapi.yaml")).unwrap();
    let spec = spec.replace(
        "{ name: pet_id, in: path, required: true, schema: { type: string } }",
        "{ name: pet_id, in: path, required: true, schema: { type: string }, example: pet_7n42 }",
    );
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["docs-data", "--out", "docs.json"]);
    assert!(ok, "{out}");
    let data: Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("docs.json")).unwrap()).unwrap();
    assert_eq!(data["name"], "Petstore");
    assert_eq!(data["base_url"], "https://petstore.example.com");
    assert_eq!(
        data["resources"][0]["operations"],
        json!(["list_pets", "create_pet", "get_pet", "delete_pet"])
    );
    let expected = [
        (
            "typescript",
            "npm install petstore",
            "client.pets.retrieve(petId: string): APIPromise<Pet>",
            "const pet = await client.pets.retrieve(\"pet_7n42\");",
            "petId",
        ),
        (
            "python",
            "pip install petstore",
            "client.pets.retrieve(pet_id: str) -> Pet",
            "pet = client.pets.retrieve(\"pet_7n42\")",
            "pet_id",
        ),
        (
            "go",
            "go get petstore",
            "client.Pets().Retrieve(ctx, petID string) (*Pet, error)",
            "pet, err := client.Pets().Retrieve(ctx, \"pet_7n42\")\nif err != nil {\n\treturn err\n}",
            "petID",
        ),
        (
            "java",
            "implementation(\"com.petstore:petstore:latest.release\")",
            "Pet client.pets().retrieve(String petId)",
            "var pet = client.pets().retrieve(\"pet_7n42\");",
            "petId",
        ),
        (
            "csharp",
            "dotnet add package Petstore",
            "Task<Pet> client.Pets.RetrieveAsync(string petId)",
            "var pet = await client.Pets.RetrieveAsync(\"pet_7n42\");",
            "petId",
        ),
        (
            "rust",
            "cargo add petstore",
            "client.pets().retrieve(pet_id: &str) -> Call<Pet>",
            "let pet = client.pets().retrieve(\"pet_7n42\").await?;",
            "pet_id",
        ),
    ];
    for (language, install, signature, sample, param) in expected {
        let sdk = &data["languages"][language];
        let get = &sdk["operations"]["get_pet"];
        assert_eq!(sdk["install"], install, "{language}");
        assert_eq!(get["signature"], signature, "{language}");
        assert!(
            get["sample"].as_str().unwrap().starts_with(sample),
            "{language}: {get}"
        );
        assert_eq!(get["complete"], true, "{language}");
        assert_eq!(get["params"]["pet_id"], param, "{language}");
        let setup = sdk["setup"].as_str().unwrap();
        assert!(
            setup.contains("PETSTORE_API_KEY") || setup.ends_with("Petstore.fromEnv();"),
            "{language}: {setup}"
        );
    }
    let python = &data["languages"]["python"];
    assert_eq!(python["types"]["Pet"]["fields"]["created_at"], "created_at");
    assert_eq!(
        python["operations"]["create_pet"]["sample"],
        "pet = client.pets.create(name=\"name\")"
    );

    let (ok, out) = perseid(dir.path(), &["docs-data", "go"]);
    assert!(ok, "{out}");
    let go: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(go["languages"].as_object().unwrap().len(), 1);
    assert_eq!(
        go["languages"]["go"]["types"]["Pet"]["fields"]["created_at"],
        "CreatedAt"
    );

    let (ok, out) = perseid(dir.path(), &["generate", "typescript", "--no-format"]);
    assert!(ok, "{out}");
    let readme = fs::read_to_string(dir.path().join("typescript/README.md")).unwrap();
    assert!(
        readme.contains("client.pets.retrieve(\"pet_7n42\")"),
        "{readme}"
    );
}

/// `(METHOD, path)` of every operation `language` generates.
fn operations(dir: &Path, language: &str) -> Vec<(String, String)> {
    let (ok, out) = perseid(dir, &["inspect", language]);
    assert!(ok, "{out}");
    let api: serde_json::Value = serde_json::from_str(&out).unwrap();
    let mut operations = vec![];
    for resource in api["resources"].as_array().unwrap() {
        for op in resource["operations"].as_array().unwrap() {
            let method = op["method"].as_str().unwrap().to_uppercase();
            operations.push((method, op["path"].as_str().unwrap().to_owned()));
        }
    }
    operations
}

#[test]
fn api_md_lists_every_operation_and_readmes_call_real_ones() {
    let calls = [
        (
            "petstore.yaml",
            "rust",
            "client.pets().retrieve(\"pet_id\")",
        ),
        (
            "petstore.yaml",
            "typescript",
            "client.pets.retrieve(\"pet_id\")",
        ),
        (
            "petstore.yaml",
            "python",
            "client.pets.retrieve(\"pet_id\")",
        ),
        (
            "petstore.yaml",
            "go",
            "client.Pets().Retrieve(ctx, \"pet_id\")",
        ),
        (
            "petstore.yaml",
            "java",
            "client.pets().retrieve(\"pet_id\")",
        ),
        (
            "petstore.yaml",
            "csharp",
            "client.Pets.RetrieveAsync(\"pet_id\")",
        ),
        (
            "features.yaml",
            "rust",
            "client.errors().list_scenarios_pages(None).items()",
        ),
        (
            "features.yaml",
            "typescript",
            "client.errors.listScenariosPages()",
        ),
        (
            "features.yaml",
            "python",
            "client.errors.list_scenarios_pages()",
        ),
        (
            "features.yaml",
            "go",
            "client.ErrorsAPI().ListScenariosPagesAutoPaging(ctx, nil)",
        ),
        (
            "features.yaml",
            "java",
            "client.errors().listScenariosPages()",
        ),
        (
            "features.yaml",
            "csharp",
            "await foreach (var widget in client.Errors.ListScenariosPagesAsync())",
        ),
        (
            "features.yaml",
            "rust",
            "client.streaming().create_completion_stream(CompletionRequest::new(\"prompt\"))",
        ),
        (
            "features.yaml",
            "typescript",
            "client.streaming.createCompletionStream({ prompt: \"prompt\" })",
        ),
        (
            "features.yaml",
            "python",
            "client.streaming.create_completion_stream(prompt=\"prompt\")",
        ),
        (
            "features.yaml",
            "go",
            "client.Streaming().CreateCompletionStream(ctx, features.CompletionRequest{Prompt: \"prompt\"})",
        ),
        (
            "features.yaml",
            "java",
            "client.streaming().createCompletionStream(CompletionRequest.builder().prompt(\"prompt\").build())",
        ),
        (
            "features.yaml",
            "csharp",
            "client.Streaming.CreateCompletionStreamAsync(new CompletionRequest { Prompt = \"prompt\" })",
        ),
    ];
    for fixture in ["petstore.yaml", "features.yaml"] {
        let dir = project_from(fixture, &LANGUAGES);
        let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
        assert!(ok, "{out}");
        for language in LANGUAGES {
            let read = |path: &str| fs::read_to_string(dir.path().join(language).join(path));
            let readme = read("README.md").unwrap();
            for placeholder in [
                "@@",
                "{{",
                "{%",
                "someResource",
                "some_resource",
                "Things()",
            ] {
                assert!(!readme.contains(placeholder), "{placeholder} in {readme}");
            }
            assert!(readme.contains("[api.md](api.md)"), "{readme}");
            assert!(
                readme.ends_with(
                    "_Generated by [perseid](https://github.com/meteroid-oss/perseid)._\n"
                ),
                "{readme}"
            );
            for (_, _, call) in calls.iter().filter(|c| c.0 == fixture && c.1 == language) {
                assert!(
                    readme.contains(call),
                    "no `{call}` in the {language} {readme}"
                );
            }
            let api = read("api.md").unwrap();
            assert!(api.starts_with("<!-- This file is @generated"), "{api}");
            let operations = operations(dir.path(), language);
            assert!(!operations.is_empty());
            for (method, path) in &operations {
                let row = format!("| `{method} {path}` |");
                let expected = operations.iter().filter(|o| o.0 == *method && o.1 == *path);
                assert!(
                    api.matches(&row).count() >= expected.count(),
                    "{language}: no row for {method} {path} in {api}"
                );
            }
        }
    }

    let dir = project_from("petstore.yaml", &["go"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let api = dir.path().join("go/api.md");
    fs::write(&api, fs::read_to_string(&api).unwrap() + "edited\n").unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--check", "--no-format"]);
    assert!(!ok && out.contains("~ api.md"), "{out}");
}

#[test]
fn csharp_types_unions_errors_and_timeout() {
    let dir = project_from("realworld.yaml", &["csharp"]);
    let config = fs::read_to_string(dir.path().join("perseid.toml")).unwrap();
    fs::write(
        dir.path().join("perseid.toml"),
        config.replacen("\n[metadata]", "timeout = 15\n\n[metadata]", 1),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let read =
        |path: &str| fs::read_to_string(dir.path().join("csharp/RealWorld").join(path)).unwrap();
    let charge = read("Models/Charge.cs");
    for expected in [
        "public required ChargeCustomer? Customer { get; init; }",
        "public sealed record Customer(global::RealWorld.Models.Customer Value) : ChargeCustomer;",
        "public static implicit operator ChargeCustomer(string value) => new StringValue(value);",
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
        charges.contains("/// <exception cref=\"NotFoundException\">404: <c>Error</c> is the <see cref=\"Models.Error\"/> body.</exception>"),
        "{charges}"
    );
    assert!(
        read("RealWorldClient.cs")
            .contains("public static Models.Error? GetError(this ApiException exception)")
    );
    assert!(read("RealWorldClientOptions.cs").contains("TimeSpan.FromSeconds(15)"));
}

#[test]
fn schema_names_that_collide_or_start_with_a_digit_are_renamed() {
    let dir = project_from("petstore.yaml", &["rust", "go", "python", "java", "csharp"]);
    let schema =
        |name: &str| format!("    {name}: {{type: object, properties: {{v: {{type: string}}}}}}\n");
    let names = [
        "3DModel",
        "Self",
        "Codec",
        "UnionRules",
        "Serialize",
        "ApiError",
        "Ptr",
        "ErrorStatus",
        "None",
        "IntEnum",
        "BaseModel",
        "JsonProperty",
        "Short",
        "Deprecated",
        "IStringEnum",
    ];
    let fields: String = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("        f{i}: {{$ref: '#/components/schemas/{n}'}}\n"))
        .collect();
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{title: Names, version: 1.0.0}}\nservers: [{{url: https://x.example.com}}]\n\
         paths:\n  /echo:\n    post:\n      operationId: echo\n      tags: [echo]\n      requestBody:\n        required: true\n        \
         content: {{application/json: {{schema: {{$ref: '#/components/schemas/Holder'}}}}}}\n      responses:\n        '200': {{description: ok}}\n\
         components:\n  schemas:\n{}    Holder:\n      type: object\n      properties:\n{fields}",
        names.iter().map(|n| schema(n)).collect::<String>()
    );
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    for language in ["rust", "go", "python", "java", "csharp"] {
        let (ok, out) = perseid(dir.path(), &["inspect", language]);
        assert!(ok, "{language}: {out}");
        let api: Value = serde_json::from_str(&out).unwrap();
        let types = api["types"].as_object().unwrap();
        assert!(types.contains_key("N3dModel"), "{language}: {out}");
        assert!(!types.contains_key("3DModel"), "{language}: {out}");
        if language == "python" {
            assert!(types.contains_key("NoneModel") && types.contains_key("IntEnumModel"));
        }
        if language == "go" {
            assert!(types.contains_key("ErrorStatusModel") && types.contains_key("PtrModel"));
        }
        if language == "rust" {
            assert!(types.contains_key("SelfModel") && types.contains_key("CodecModel"));
        }
        if language == "java" {
            assert!(types.contains_key("ShortModel") && types.contains_key("DeprecatedModel"));
        }
    }
}

/// Every generated file of `language` under `dir`, concatenated.
fn generated_text(dir: &Path, language: &str) -> String {
    fn walk(path: &Path, out: &mut String) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Ok(text) = fs::read_to_string(&path) {
                out.push_str(&text);
            }
        }
    }
    let mut out = String::new();
    walk(&dir.join(language), &mut out);
    out
}

#[test]
fn nullable_items_values_and_optional_responses_are_typed_in_every_language() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let spec = "openapi: 3.1.0\ninfo: {title: Nullables, version: 1.0.0}\n\
        servers: [{url: https://x.example.com}]\n\
        paths:\n\
        \x20 /echo:\n    post:\n      operationId: echo\n      tags: [echo]\n      requestBody:\n        required: true\n        \
        content: {application/json: {schema: {$ref: '#/components/schemas/Holder'}}}\n      responses:\n        \
        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/Holder'}}}}\n\
        \x20 /maybe:\n    get:\n      operationId: get_maybe\n      tags: [echo]\n      responses:\n        \
        '200': {description: ok, content: {application/json: {schema: {oneOf: [{$ref: '#/components/schemas/Holder'}, {type: 'null'}]}}}}\n\
        \x20 /gone:\n    get:\n      operationId: get_gone\n      tags: [echo]\n      responses:\n        \
        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/Holder'}}}}\n        \
        '204': {description: gone}\n\
        components:\n  schemas:\n    Holder:\n      type: object\n      properties:\n        \
        tags: {type: array, items: {type: [string, 'null']}}\n        \
        labels: {type: object, additionalProperties: {type: [string, 'null']}}\n";
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let expected: [(&str, &[&str]); 6] = [
        (
            "rust",
            &[
                "Vec<Option<String>>",
                "HashMap<String, Option<String>>",
                "Option<crate::models::Holder>",
                "json_or_none(",
            ],
        ),
        (
            "typescript",
            &["(string | null)[]", "sendOptional(", "| undefined>"],
        ),
        (
            "python",
            &[
                "list[str | None]",
                "dict[str, str | None]",
                "Holder | None",
                "(204, 205)",
            ],
        ),
        (
            "go",
            &["[]*string", "map[string]*string", "(*Holder, error)"],
        ),
        ("java", &["Optional<Holder>"]),
        (
            "csharp",
            &[
                "List<string?>",
                "Dictionary<string, string?>",
                "Task<Holder?>",
                "SendJsonOrDefaultAsync<Holder, Holder?>",
            ],
        ),
    ];
    for (language, needles) in expected {
        let text = generated_text(dir.path(), language);
        for needle in needles {
            assert!(text.contains(needle), "{language}: missing `{needle}`");
        }
    }
}

#[test]
fn cookie_parameters_are_sent_in_the_cookie_header_in_every_language() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let spec = "openapi: 3.1.0\ninfo: {title: Cookies, version: 1.0.0}\n\
        servers: [{url: https://x.example.com}]\n\
        paths:\n\
        \x20 /x:\n    get:\n      operationId: get_x\n      tags: [x]\n      parameters:\n        \
        - {name: sid, in: cookie, required: true, schema: {type: string}}\n        \
        - {name: sid, in: query, schema: {type: string}}\n      responses:\n        '204': {description: ok}\n\
        components:\n  securitySchemes:\n    session: {type: apiKey, in: cookie, name: auth}\n\
        security: [{session: []}]\n";
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let expected: [(&str, &str); 6] = [
        ("rust", "with_cookie_param(\"sid\""),
        ("typescript", "setCookieParam(\"sid\""),
        ("python", "cookie_params="),
        ("go", "SetCookie(\"sid\""),
        ("java", "Utils.cookiePair(\"sid\""),
        ("csharp", "SetCookie(\"sid\""),
    ];
    for (language, needle) in expected {
        let text = generated_text(dir.path(), language);
        assert!(text.contains(needle), "{language}: missing `{needle}`");
    }
}

#[test]
fn undeclared_security_schemes_fail_unless_their_operations_are_excluded() {
    let dir = project_from("petstore.yaml", &["typescript"]);
    let spec = "openapi: 3.1.0\ninfo: {title: Keys, version: 1.0.0}\n\
        servers: [{url: https://x.example.com}]\n\
        paths:\n\
        \x20 /x:\n    get:\n      operationId: get_x\n      tags: [x]\n      \
        security: [{api_key: []}]\n      responses:\n        '204': {description: ok}\n\
        \x20 /y:\n    get:\n      operationId: get_y\n      tags: [x]\n      responses:\n        \
        '204': {description: ok}\n\
        components:\n  securitySchemes:\n    bearer: {type: http, scheme: bearer}\n\
        security: [{bearer: []}]\n";
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(
        !ok && out.contains("operation `get_x` requires the security scheme `api_key`"),
        "{out}"
    );
    let config_path = dir.path().join("perseid.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(&config_path, format!("exclude = [\"get_x\"]\n{config}")).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
}

#[test]
fn any_json_body_and_multipart_file_lists_are_generated_in_every_language() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let spec = "openapi: 3.1.0\ninfo: {title: Shapes, version: 1.0.0}\n\
        servers: [{url: https://x.example.com}]\n\
        paths:\n\
        \x20 /any:\n    get:\n      operationId: get_any\n      tags: [x]\n      responses:\n        \
        '200': {description: ok, content: {application/json: {}}}\n\
        \x20 /map:\n    get:\n      operationId: get_map\n      tags: [x]\n      responses:\n        \
        '200': {description: ok, content: {application/json: {schema: {type: object, additionalProperties: {$ref: '#/components/schemas/V'}}}}}\n\
        \x20 /text:\n    get:\n      operationId: get_text\n      tags: [x]\n      responses:\n        \
        '200': {description: ok, content: {text/plain: {schema: {type: string}}}}\n\
        \x20 /upload:\n    post:\n      operationId: upload\n      tags: [x]\n      requestBody:\n        \
        content:\n          multipart/form-data:\n            schema:\n              type: object\n              \
        properties:\n                files: {type: array, items: {type: string, format: binary}}\n            \
        encoding:\n              files: {contentType: image/png}\n      responses:\n        '204': {description: ok}\n\
        components:\n  schemas:\n    V: {type: object, properties: {v: {type: string}}}\n";
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!out.contains("skipping the operation"), "{out}");
    let expected: [(&str, &[&str]); 6] = [
        (
            "rust",
            &["HashMap<String, V>", "Vec<crate::api::Upload>", "file_as("],
        ),
        (
            "typescript",
            &["{ [key: string]: V }", "Upload[]", "\"image/png\""],
        ),
        (
            "python",
            &["dict[str, V]", "list[FileInput]", "\"image/png\""],
        ),
        (
            "go",
            &["map[string]V", "[]Upload", "contentType: \"image/png\""],
        ),
        (
            "java",
            &[
                "Map<String,V>",
                "List<Upload>",
                "\"image/png\"",
                "returningText()",
            ],
        ),
        (
            "csharp",
            &["Dictionary<string, V>", "List<Upload>", "\"image/png\""],
        ),
    ];
    for (language, needles) in expected {
        let text = generated_text(dir.path(), language);
        for needle in needles {
            assert!(text.contains(needle), "{language}: missing `{needle}`");
        }
    }
}

#[test]
fn parameter_styles_and_content_are_generated_in_every_language() {
    let languages = ["rust", "typescript", "python", "go", "java", "csharp"];
    let dir = project_from("petstore.yaml", &languages);
    let spec = r##"
openapi: 3.1.0
info: { title: Styles, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /search:
    get:
      operationId: search
      tags: [x]
      parameters:
        - { name: ids, in: query, style: pipeDelimited, explode: false, schema: { type: array, items: { type: string } } }
        - { name: words, in: query, style: spaceDelimited, explode: false, schema: { type: array, items: { type: string } } }
        - name: filter
          in: query
          content: { application/json: { schema: { type: object, properties: { a: { type: string } } } } }
        - name: X-Filter
          in: header
          content: { application/json: { schema: { type: object, properties: { b: { type: string } } } } }
      responses: { "204": { description: ok } }
  /items/{tags}/{color}/{point}:
    get:
      operationId: get_item
      tags: [x]
      parameters:
        - { name: tags, in: path, required: true, style: label, explode: true, schema: { type: array, items: { type: string } } }
        - { name: color, in: path, required: true, style: matrix, schema: { type: string } }
        - { name: point, in: path, required: true, schema: { type: object, properties: { x: { type: integer } } } }
      responses: { "204": { description: ok } }
  /blobs/{spec}:
    get:
      operationId: get_blob
      tags: [x]
      parameters:
        - name: spec
          in: path
          required: true
          content: { application/json: { schema: { type: object, properties: { a: { type: string } } } } }
      responses: { "204": { description: ok } }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!out.contains("skipping the operation"), "{out}");
    let expected: [(&str, &[&str]); 6] = [
        (
            "rust",
            &[
                "with_delimited_query_param(\"ids\"",
                "with_json_query_param(\"filter\"",
                "with_json_header_param(\"X-Filter\"",
                "with_styled_path_param(\"spec\"",
                "\"matrix\"",
            ],
        ),
        (
            "typescript",
            &[
                "setDelimitedQueryParam(\"ids\"",
                "setJsonQueryParam(\"filter\"",
                "setJsonHeaderParam(\"X-Filter\"",
                "setStyledPathParam(\"tags\"",
            ],
        ),
        (
            "python",
            &[
                "delimited={\"ids\": \"|\"",
                "json_params=(\"filter\"",
                "json_header(",
                "encode_path_param(\"spec\"",
            ],
        ),
        (
            "go",
            &[
                "AddDelimitedQueryParam(\"ids\"",
                "AddJSONQueryParam(\"filter\"",
                "SetJSONHeader(\"X-Filter\"",
                "SetStyledPathParam(\"tags\"",
            ],
        ),
        (
            "java",
            &[
                "addDelimitedQueryParameter(url, \"ids\"",
                "addJsonQueryParameter(url, \"filter\"",
                "Utils.encodePathParam(\"spec\"",
                "addEncodedPathSegment",
            ],
        ),
        (
            "csharp",
            &[
                "AddDelimitedQuery(\"ids\"",
                "AddJsonQuery(\"filter\"",
                "SetJsonHeader(\"X-Filter\"",
                "ApiRequest.EncodePathParam(\"color\"",
            ],
        ),
    ];
    for (language, needles) in expected {
        let text = generated_text(dir.path(), language);
        for needle in needles {
            assert!(text.contains(needle), "{language}: missing `{needle}`");
        }
    }
}

#[test]
fn unsupported_operations_are_skipped_with_a_warning() {
    let dir = project_from("petstore.yaml", &["rust"]);
    let spec = r##"
openapi: 3.1.0
info: { title: Skips, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /ok:
    get:
      operationId: get_ok
      tags: [x]
      responses: { "204": { description: ok } }
  /bad:
    get:
      operationId: get_bad
      tags: [x]
      parameters:
        - name: q
          in: query
          content: { text/csv: { schema: { type: string } } }
      responses: { "204": { description: ok } }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(out.contains("skipping the operation"), "{out}");
    assert!(out.contains("get_bad"), "{out}");
    let text = generated_text(dir.path(), "rust");
    assert!(text.contains("\"/ok\"") && !text.contains("/bad"), "{text}");
}

#[test]
fn models_with_extra_properties_keep_them_in_every_language() {
    let dir = project_from(
        "petstore.yaml",
        &["rust", "typescript", "python", "go", "java", "csharp"],
    );
    let spec = r##"
openapi: 3.1.0
info: { title: Extras, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /things:
    get:
      operationId: get_thing
      tags: [x]
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Thing" }
components:
  schemas:
    Thing:
      type: object
      properties:
        name: { type: string }
      additionalProperties: { type: integer }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!out.contains("dropped when decoding"), "{out}");
    let expected: &[(&str, &[&str])] = &[
        (
            "rust",
            &[
                "#[serde(flatten)]",
                ": std::collections::HashMap<String, i64>,",
            ],
        ),
        (
            "typescript",
            &[
                "...extraProperties(json, [\"name\"], (item: any, key: string) => decodeInteger(item, path, key))",
            ],
        ),
        ("python", &["_EXTRA_FIELDS: t.ClassVar[dict[str, int]]"]),
        ("go", &["ExtraFields map[string]int64", "typedExtraFields("]),
        (
            "java",
            &[
                "@JsonAnyGetter",
                "@JsonAnySetter",
                "Map<String, Long> typedAdditionalProperties()",
            ],
        ),
        (
            "csharp",
            &[
                "[JsonExtensionData]",
                "Dictionary<string, JsonElement>? AdditionalProperties",
            ],
        ),
    ];
    for (language, needles) in expected {
        let text = generated_text(dir.path(), language);
        for needle in *needles {
            assert!(text.contains(needle), "{language}: missing `{needle}`");
        }
    }
}

#[test]
fn unsupported_constructs_warn_with_names_and_stay_typed_where_possible() {
    let dir = project_from(
        "petstore.yaml",
        &["rust", "typescript", "python", "go", "java", "csharp"],
    );
    let spec = r##"
openapi: 3.1.0
info: { title: Polish, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
components:
  securitySchemes:
    digest: { type: http, scheme: digest }
    key: { type: apiKey, in: header, name: X-Key }
  schemas:
    Symbols:
      type: string
      enum: ["-", "*", "a-b", "a_b", "Active", "active"]
    Negated: { not: { type: string } }
    Pair:
      type: array
      prefixItems: [{ type: string }, { type: integer }]
paths:
  /things:
    get:
      operationId: get_thing
      tags: [x]
      security: [{ digest: [] }]
      servers: [{ url: "https://other.example.com" }]
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  symbols: { $ref: "#/components/schemas/Symbols" }
                  negated: { $ref: "#/components/schemas/Negated" }
                  pair: { $ref: "#/components/schemas/Pair" }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for warning in [
        "unsupported http auth scheme `digest`",
        "operation `get_thing`",
        "declares its own `servers`",
        "is only a `not`",
        "tuple array in schema `Pair`",
    ] {
        assert!(out.contains(warning), "missing `{warning}` in:\n{out}");
    }
    assert!(!out.contains("typed as a string"), "{out}");
    let rust = generated_text(dir.path(), "rust");
    for needle in ["Minus", "Star", "\"a_b\" => Self::AB2"] {
        assert!(rust.contains(needle), "missing `{needle}`");
    }
}

fn run_samples(dir: &Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["samples", "--out", "samples.json"];
    args.extend_from_slice(extra);
    let (ok, out) = perseid(dir, &args);
    assert!(ok, "{out}");
    serde_json::from_str(&fs::read_to_string(dir.join("samples.json")).unwrap()).unwrap()
}

fn sample<'a>(model: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    model["samples"]
        .as_array()?
        .iter()
        .find(|s| s["name"] == name)
        .map(|s| &s["json"])
}

fn contains_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(items) => items.iter().any(contains_null),
        serde_json::Value::Object(map) => map.values().any(contains_null),
        _ => false,
    }
}

#[test]
fn samples_cover_every_model_of_the_torture_fixture() {
    let dir = project_from("torture.yaml", &["rust", "typescript", "go"]);
    let samples = run_samples(dir.path(), &[]);
    let (ok, model) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{model}");
    let model: serde_json::Value = serde_json::from_str(&model).unwrap();
    let types = model["types"].as_object().unwrap();
    assert!(!types.is_empty());
    for name in types.keys() {
        let entry = &samples[name];
        assert!(entry.is_object(), "no samples of `{name}`");
        assert!(
            ["struct", "enum", "union", "alias"].contains(&entry["kind"].as_str().unwrap()),
            "{name}: {entry}"
        );
        assert!(entry["type_name"].is_string(), "{name}");
        for language in ["rust", "typescript", "go"] {
            assert!(entry["names"][language].is_string(), "{name} in {language}");
        }
        assert!(sample(entry, "full").is_some(), "{name} has no full sample");
        assert!(
            sample(entry, "minimal").is_some(),
            "{name} has no minimal sample"
        );
    }
    assert_eq!(samples.as_object().unwrap().len(), types.len());
}

#[test]
fn samples_tag_every_variant_of_a_union_with_its_discriminator() {
    let dir = project_from("torture.yaml", &["rust"]);
    let samples = run_samples(dir.path(), &[]);
    for (name, tag) in [("Shape", "type"), ("Pet", "pet_type")] {
        let union = &samples[name];
        assert_eq!(union["kind"], "union", "{name}");
        let variants: Vec<&serde_json::Value> = union["samples"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["name"].as_str().unwrap().starts_with("variant_"))
            .collect();
        assert_eq!(variants.len(), 2, "{name}: {union}");
        for variant in variants {
            let expected = variant["name"]
                .as_str()
                .unwrap()
                .trim_start_matches("variant_");
            assert_eq!(variant["json"][tag], expected, "{name}: {variant}");
        }
    }
    let shape = &samples["Shape"];
    assert_eq!(sample(shape, "variant_circle").unwrap()["type"], "circle");
    assert_eq!(sample(shape, "variant_square").unwrap()["type"], "square");
}

#[test]
fn samples_set_nullable_values_to_null_and_leave_optional_ones_out_of_minimal() {
    let dir = project_from("torture.yaml", &["rust"]);
    let samples = run_samples(dir.path(), &[]);
    let mut with_nulls = 0;
    for (name, entry) in samples.as_object().unwrap() {
        if let Some(nulls) = sample(entry, "nulls") {
            with_nulls += 1;
            assert!(contains_null(nulls), "{name}: {nulls}");
        }
        for s in entry["samples"].as_array().unwrap() {
            let sample_name = s["name"].as_str().unwrap();
            if sample_name == "full" || sample_name == "minimal" {
                continue;
            }
            assert!(
                sample_name.starts_with("variant_")
                    || sample_name.starts_with("value_")
                    || sample_name.starts_with("full_pick_")
                    || sample_name.starts_with("nulls"),
                "{name}: unexpected sample `{sample_name}`"
            );
        }
    }
    assert!(with_nulls > 0, "no model has a nulls sample");
    // Every sample is valid JSON of the model: objects carry their required properties, so
    // `minimal` is never larger than `full`.
    for (name, entry) in samples.as_object().unwrap() {
        let (full, minimal) = (
            sample(entry, "full").unwrap(),
            sample(entry, "minimal").unwrap(),
        );
        if let (Some(full), Some(minimal)) = (full.as_object(), minimal.as_object()) {
            assert!(
                minimal.len() <= full.len(),
                "{name}: {minimal:?} vs {full:?}"
            );
        }
    }
}

#[test]
fn samples_are_deterministic_and_need_no_project_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        "tests/fixtures/torture.yaml",
        dir.path().join("openapi.yaml"),
    )
    .unwrap();
    let first = run_samples(dir.path(), &["--spec", "openapi.yaml"]);
    let text = fs::read_to_string(dir.path().join("samples.json")).unwrap();
    let second = run_samples(dir.path(), &["--spec", "openapi.yaml"]);
    assert_eq!(first, second);
    assert_eq!(
        text,
        fs::read_to_string(dir.path().join("samples.json")).unwrap()
    );
    for language in ["rust", "typescript", "python", "go", "java", "csharp"] {
        assert!(first["Shape"]["names"][language].is_string(), "{language}");
    }
    let (ok, out) = perseid(dir.path(), &["samples", "--out", "x.json"]);
    assert!(!ok, "without a spec or a project file: {out}");
}

#[test]
fn samples_command_is_hidden_from_help() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, out) = perseid(dir.path(), &["--help"]);
    assert!(ok, "{out}");
    assert!(
        !out.lines().any(|l| l.trim_start().starts_with("samples")),
        "{out}"
    );
}

#[test]
fn adjacent_unions_repeating_the_discriminator_generate_and_stay_typed_in_go() {
    let dir = project_from("petstore.yaml", &["python", "go"]);
    let spec = r##"
openapi: 3.1.0
info: { title: Adjacent, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /events:
    get:
      operationId: get_event
      tags: [x]
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Event" }
components:
  schemas:
    Event:
      type: object
      required: [kind]
      properties:
        kind: { type: string }
        at: { type: string }
      oneOf:
        - type: object
          required: [kind, data]
          properties:
            kind: { type: string, enum: [created] }
            data: { type: object, properties: { id: { type: string } } }
        - type: object
          required: [kind]
          properties:
            kind: { type: string, enum: [ping] }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!out.contains("untyped JSON"), "{out}");
    assert!(generated_text(dir.path(), "python").contains("class Event"));
    assert!(generated_text(dir.path(), "go").contains("func NewEventPing() Event"));
}

#[test]
fn operation_ids_colliding_once_snake_cased_are_resolved_by_x_perseid_name() {
    let dir = project_from("petstore.yaml", &["rust"]);
    let spec = r##"
openapi: 3.1.0
info: { title: Collide, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /a/{id}:
    get:
      operationId: getThing
      tags: [things]
      parameters: [{ name: id, in: path, required: true, schema: { type: string } }]
      responses: { "204": { description: ok } }
  /b/{id}:
    get:
      operationId: get_thing
      x-perseid-name: get_other_thing
      tags: [things]
      parameters: [{ name: id, in: path, required: true, schema: { type: string } }]
      responses: { "204": { description: ok } }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let rust = generated_text(dir.path(), "rust");
    assert!(
        rust.contains("fn retrieve(") && rust.contains("fn get_other_thing("),
        "{rust}"
    );
}

#[test]
fn unions_sharing_json_types_and_union_bodies_stay_typed() {
    let dir = project_from("petstore.yaml", &["python"]);
    let spec = r##"
openapi: 3.1.0
info: { title: Unions, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /completions:
    post:
      operationId: createCompletion
      tags: [completions]
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/CompletionRequest" }
      responses: { "204": { description: ok } }
  /transcriptions:
    post:
      operationId: createTranscription
      tags: [transcriptions]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              oneOf:
                - { $ref: "#/components/schemas/Plain" }
                - { type: string }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                oneOf:
                  - { $ref: "#/components/schemas/Plain" }
                  - { $ref: "#/components/schemas/Verbose" }
components:
  schemas:
    CompletionRequest:
      type: object
      required: [prompt]
      properties:
        prompt:
          anyOf:
            - { type: string }
            - { type: array, items: { type: string } }
            - { type: array, items: { type: integer } }
            - { type: array, items: { type: array, items: { type: integer } } }
    Plain:
      type: object
      required: [text]
      properties: { text: { type: string } }
    Verbose:
      type: object
      required: [text, segments]
      properties:
        text: { type: string }
        segments: { type: array, items: { type: string } }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, model) = perseid(dir.path(), &["inspect"]);
    assert!(ok, "{model}");
    let model: serde_json::Value = serde_json::from_str(&model).unwrap();
    let fields = model["types"]["CompletionRequest"]["fields"]
        .as_array()
        .unwrap();
    let prompt = &fields[0]["type"];
    assert_eq!(prompt["id"], "Union");
    assert_eq!(prompt["decode"], "try");
    let names: Vec<_> = prompt["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "string",
            "array_of_strings",
            "array_of_integers",
            "array_of_integer_arrays"
        ]
    );
    assert_eq!(prompt["variants"][2]["items"]["json_type"], "integer");
    assert_eq!(
        prompt["variants"][3]["items"]["items"]["json_type"],
        "integer"
    );

    // Inline body unions are named after the operation, and stay typed unions.
    let transcription = operation(&model, "createTranscription");
    assert_eq!(
        transcription["request_body_schema_name"],
        "CreateTranscriptionRequest"
    );
    let request = &model["types"]["CreateTranscriptionRequest"]["target"];
    assert_eq!(request["id"], "Union");
    assert_eq!(
        transcription["response_body_schema_name"],
        "CreateTranscriptionResponse"
    );
    let response = &model["types"]["CreateTranscriptionResponse"]["target"];
    assert_eq!(response["id"], "Union");
    assert_eq!(response["mode"], "rules");
    assert!(model["types"].get("Verbose").is_some());

    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    let python = generated_text(dir.path(), "python");
    assert!(
        python.contains("decode_response(response, CreateTranscriptionResponse)"),
        "{python}"
    );
}

#[test]
fn oauth2_client_credentials_clients_get_the_credentials_options_and_the_token_url() {
    let dir = project_from("oauth.yaml", &LANGUAGES);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for language in LANGUAGES {
        let sources = files(&dir.path().join(language));
        let all: String = sources.iter().map(|(_, text)| text.as_str()).collect();
        assert!(all.contains("\"oauth/token\""), "{language}: no token URL");
        assert!(
            all.contains("secrets.read secrets.write"),
            "{language}: the scopes of the requirements are not asked for"
        );
        assert!(
            all.contains("VAULT_CLIENT_ID") && all.contains("VAULT_CLIENT_SECRET"),
            "{language}: the client credentials are not read from the environment"
        );
    }
}

#[test]
fn specs_without_a_client_credentials_flow_get_no_credentials_options() {
    let dir = project_from("petstore.yaml", &["typescript", "python", "rust", "csharp"]);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for language in ["typescript", "python", "rust", "csharp"] {
        for (name, text) in files(&dir.path().join(language)) {
            assert!(
                !text.contains("PETSTORE_CLIENT_ID"),
                "{language}/{name} reads a client id"
            );
        }
    }
}

#[test]
fn go_types_adjacently_tagged_unions_and_hoists_inline_variants() {
    let dir = project_from("petstore.yaml", &["go"]);
    let spec = r##"
openapi: 3.1.0
info: { title: Tagged, version: "1.0.0" }
servers: [{ url: "https://x.example.com" }]
paths:
  /events/{id}:
    get:
      operationId: getEvent
      tags: [events]
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Event" }
  /parts:
    post:
      operationId: createPart
      tags: [events]
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Part" }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Part" }
components:
  schemas:
    Event:
      type: object
      required: [type]
      properties:
        type: { type: string }
        at: { type: string }
      oneOf:
        - type: object
          required: [type, data]
          properties:
            type: { type: string, enum: [created] }
            data: { $ref: "#/components/schemas/Thing" }
        - type: object
          required: [type, data]
          properties:
            type: { type: string, enum: [renamed] }
            data:
              type: object
              required: [name]
              properties: { name: { type: string } }
        - type: object
          required: [type]
          properties:
            type: { type: string, enum: [ping] }
    Thing:
      type: object
      required: [id]
      properties: { id: { type: string } }
    Part:
      oneOf:
        - type: object
          required: [type, text]
          properties:
            type: { type: string, enum: [text] }
            text: { type: string }
        - type: object
          required: [type, url]
          properties:
            type: { type: string, enum: [link] }
            url: { type: string }
      discriminator: { propertyName: type }
"##;
    fs::write(dir.path().join("openapi.yaml"), spec).unwrap();
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    assert!(!out.contains("untyped JSON value"), "{out}");
    let sources = files(&dir.path().join("go"));
    let source = |name: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(name))
            .unwrap_or_else(|| panic!("no {name} in {sources:?}"))
            .1
            .clone()
    };
    let event = source("event.go");
    assert!(
        event.contains("unmarshalAdjacentContent(data, \"data\""),
        "{event}"
    );
    assert!(event.contains("func NewEventPing() Event"), "{event}");
    assert!(source("event_data.go").contains("type EventData struct"));
    assert!(source("part_text_variant.go").contains("type PartTextVariant struct"));
    assert!(source("part.go").contains("marshalUnionVariant("));
}

#[test]
fn oauth2_token_urls_are_passed_to_the_runtimes_as_written() {
    let dir = project_from("oauth.yaml", &LANGUAGES);
    let (ok, out) = perseid(dir.path(), &["generate", "--no-format"]);
    assert!(ok, "{out}");
    for language in LANGUAGES {
        let all = generated_text(dir.path(), language);
        assert!(
            all.contains("\"oauth/token\""),
            "{language}: token URL rewritten"
        );
        assert!(
            !all.contains("\"/oauth/token\""),
            "{language}: token URL rewritten"
        );
        assert!(
            all.contains("secrets.admin"),
            "{language}: no scope of the second scheme"
        );
    }
}

#[test]
fn resources_place_operations_and_name_no_unknown_or_invalid_ones() {
    let dir = project();
    let config = dir.path().join("perseid.toml");
    let base = fs::read_to_string(&config).unwrap();
    let with = |table: &str| fs::write(&config, format!("{base}\n[resources]\n{table}")).unwrap();
    with("list_pets = \"store.pets\"\nlistPetz = \"pets\"\n");
    let (ok, out) = perseid(dir.path(), &["inspect", "rust"]);
    assert!(ok, "{out}");
    assert!(out.contains("\"store\",\n        \"pets\""), "{out}");
    let args = ["generate", "--no-format", "--out", "sdks"];
    let (ok, out) = perseid(dir.path(), &args);
    assert!(ok, "{out}");
    assert!(
        out.contains(
            "`listPetz` in the [resources] table of perseid.toml is no operation of the spec"
        ),
        "{out}"
    );
    with("list_pets = \"Store.pets\"\n");
    let (ok, out) = perseid(dir.path(), &["inspect", "rust"]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("[resources] `list_pets = \"Store.pets\"`: `Store.pets` `Store` is not a snake_case name"),
        "{out}"
    );
    assert!(!out.contains("perseid does not support"), "{out}");
}

#[test]
fn docs_data_calls_methods_through_their_child_resources() {
    let dir = project_from("edge-nested.yaml", &LANGUAGES);
    let (ok, out) = perseid(dir.path(), &["docs-data", "--out", "docs.json"]);
    assert!(ok, "{out}");
    let data: Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("docs.json")).unwrap()).unwrap();
    let workspaces = (data["resources"].as_array().unwrap().iter())
        .find(|r| r["name"] == "workspaces")
        .unwrap();
    let peers = &workspaces["subresources"].as_array().unwrap()[1];
    assert_eq!(peers["path"], json!(["workspaces", "peers"]));
    assert!(
        peers["operations"]
            .as_array()
            .unwrap()
            .contains(&json!("get_workspace_peer"))
    );
    assert_eq!(
        peers["subresources"][0]["path"],
        json!(["workspaces", "peers", "sessions"])
    );
    let expected = [
        (
            "rust",
            "client.workspaces().peers().sessions().list",
            "client.schools().class().list",
            "client.workspaces().peers().retrieve(",
        ),
        (
            "typescript",
            "client.workspaces.peers.sessions.list",
            "client.schools.class.list",
            "client.workspaces.peers.retrieve(",
        ),
        (
            "python",
            "client.workspaces.peers.sessions.list",
            "client.schools.class_.list",
            "client.workspaces.peers.retrieve(",
        ),
        (
            "go",
            "client.Workspaces().Peers().Sessions().List",
            "client.Schools().Class().List",
            "client.Workspaces().Peers().Retrieve(",
        ),
        (
            "java",
            "client.workspaces().peers().sessions().list",
            "client.schools().class_().list",
            "Peer client.workspaces().peers().retrieve(",
        ),
        (
            "csharp",
            "client.Workspaces.Peers.Sessions.ListAsync",
            "client.Schools.Class.ListAsync",
            "Task<Peer> client.Workspaces.Peers.RetrieveAsync(",
        ),
    ];
    for (language, sessions, classes, retrieve) in expected {
        let ops = &data["languages"][language]["operations"];
        assert_eq!(ops["list_peer_sessions"]["method"], sessions, "{language}");
        assert_eq!(ops["list_school_classes"]["method"], classes, "{language}");
        let sample = ops["list_peer_sessions"]["sample"].as_str().unwrap();
        assert!(sample.contains(sessions), "{language}: {sample}");
        let signature = ops["get_workspace_peer"]["signature"].as_str().unwrap();
        assert!(signature.starts_with(retrieve), "{language}: {signature}");
    }
}

#[test]
fn docs_data_lists_the_sdks_in_the_order_of_perseid_toml_with_their_version() {
    let dir = project();
    let path = dir.path().join("perseid.toml");
    let config = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        config.replace("sdks = [\"rust\", \"go\"]", "sdks = [\"go\", \"rust\"]"),
    )
    .unwrap();
    let (ok, out) = perseid(dir.path(), &["docs-data"]);
    assert!(ok, "{out}");
    let data: serde_json::Value = serde_json::from_str(&out).unwrap();
    let languages = data["languages"].as_object().unwrap();
    assert_eq!(languages.keys().collect::<Vec<_>>(), ["go", "rust"]);
    for (language, sdk) in languages {
        let keys: Vec<&String> = sdk.as_object().unwrap().keys().take(2).collect();
        assert_eq!(keys, ["version", "install"], "{language}");
        assert!(sdk["version"].is_null(), "{language}");
    }
    let (ok, out) = perseid(dir.path(), &["docs-data", "rust", "go"]);
    assert!(ok, "{out}");
    let data: serde_json::Value = serde_json::from_str(&out).unwrap();
    let keys: Vec<&String> = data["languages"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["go", "rust"]);
}

/// Appends `[targets.docs]` with `table` to perseid.toml in `dir`.
fn with_docs_target(dir: &Path, table: &str) {
    let path = dir.join("perseid.toml");
    let config = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{config}\n[targets.docs]\n{table}")).unwrap();
}

#[test]
fn targets_preview_writes_the_spec_and_docs_data_of_a_docs_target() {
    let dir = project();
    with_docs_target(dir.path(), "repo = \"acme/docs\"\n");
    let (ok, out) = perseid(dir.path(), &["targets"]);
    assert!(!ok && out.contains("--out"), "{out}");
    let (ok, out) = perseid(dir.path(), &["targets", "--out", "preview"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("docs (preview/docs): 3 added, 0 modified, 0 removed"),
        "{out}"
    );
    let docs = dir.path().join("preview/docs");
    let read = |file: &str| fs::read_to_string(docs.join(file)).unwrap();
    assert_eq!(read(".gitattributes"), "* linguist-generated=true\n");
    let spec: serde_json::Value = serde_json::from_str(&read("openapi.json")).unwrap();
    assert_eq!(spec["info"]["title"], "Petstore API");
    let data: serde_json::Value = serde_json::from_str(&read("docs-data.json")).unwrap();
    assert!(data["languages"]["go"]["operations"]["get_pet"].is_object());
    let (ok, out) = perseid(dir.path(), &["targets", "cli", "--out", "preview"]);
    assert!(!ok && out.contains("no [targets.cli]"), "{out}");

    git_in(dir.path(), &["init", "--quiet", "--initial-branch", "main"]);
    let origin = "https://github.com/acme/petstore-sdks";
    git_in(dir.path(), &["remote", "add", "origin", origin]);
    let (ok, out) = perseid(dir.path(), &["init"]);
    assert!(ok, "{out}");
    let workflows = dir.path().join(".github/workflows");
    let sdks = fs::read_to_string(workflows.join("sdks.yml")).unwrap();
    assert!(
        sdks.contains("  repository_dispatch:\n    types: [perseid-released]\n"),
        "{sdks}"
    );
    let release = fs::read_to_string(workflows.join("sdk-release.yml")).unwrap();
    assert!(
        release.contains("uses: meteroid-oss/perseid/report@v0\n")
            && release.contains("to: \"acme/petstore-sdks\"\n"),
        "{release}"
    );
    let kept = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(kept.join("targets-sdks.yml"), sdks).unwrap();
    fs::write(kept.join("targets-sdk-release.yml"), release).unwrap();
}

/// A bare repository at `<dir>/docs.git` written by people: a README, and an `api` folder
/// holding an older spec and a page perseid doesn't write.
#[cfg(unix)]
fn docs_repository(dir: &Path) -> String {
    let work = dir.join("docs-work");
    fs::create_dir_all(work.join("api")).unwrap();
    fs::write(work.join("README.md"), "# Docs\n").unwrap();
    fs::write(work.join("api/guide.md"), "stale\n").unwrap();
    fs::write(work.join("api/openapi.json"), "{}\n").unwrap();
    let git = |args: &[&str]| git_in(&work, args);
    git(&["init", "--quiet", "--initial-branch", "main"]);
    git(&["add", "--all"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "docs",
    ]);
    let bare = dir.join("docs.git");
    git_in(
        dir,
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch",
            "main",
            "docs.git",
        ],
    );
    git(&["push", "--quiet", bare.to_str().unwrap(), "main"]);
    fs::remove_dir_all(&work).unwrap();
    format!("file://{}", bare.display())
}

#[cfg(unix)]
#[test]
fn docs_targets_open_their_pull_request_with_the_sdks_or_once_they_are_released() {
    let dir = pull_request_project();
    let docs = docs_repository(dir.path());
    with_docs_target(
        dir.path(),
        &format!("repo = {docs:?}\nafter = \"generate\"\n"),
    );
    let run = run_pr(dir.path(), &["--bump", "patch"], Answers::default(), &[]);
    assert!(run.ok, "{}", run.output);
    let created = run.requests("POST", "/docs/pulls");
    assert_eq!(created.len(), 1, "{:#?}", run.calls);
    assert_eq!(created[0]["head"], "perseid/targets/docs");
    assert_eq!(created[0]["base"], "main");
    assert_eq!(
        created[0]["title"],
        "fix(api): update the API reference to Petstore API 1.0.0"
    );
    let body = created[0]["body"].as_str().unwrap();
    assert!(
        body.contains("docs: 2 added, 1 modified, 1 removed\n")
            && body.contains("- api/guide.md")
            && body.contains("SDK versions: go (unreleased), rust (unreleased)."),
        "{body}"
    );
    let bare = dir.path().join("docs.git");
    let tree = git_in(
        &bare,
        &["ls-tree", "-r", "--name-only", "perseid/targets/docs"],
    );
    assert_eq!(
        tree.lines().collect::<Vec<_>>(),
        [
            "README.md",
            "api/.gitattributes",
            "api/docs-data.json",
            "api/openapi.json"
        ]
    );
    let data = git_in(&bare, &["show", "perseid/targets/docs:api/docs-data.json"]);
    let data: Value = serde_json::from_str(&data).unwrap();
    assert!(data["languages"]["rust"]["version"].is_null(), "{data}");
    let sdk = run.requests("POST", "/origin/pulls");
    assert_eq!(sdk.len(), 1, "{:#?}", run.calls);

    let config = dir.path().join("perseid.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        text.replace("after = \"generate\"", "after = \"sdks\""),
    )
    .unwrap();
    let run = run_pr(dir.path(), &["--bump", "patch"], Answers::default(), &[]);
    assert!(run.ok, "{}", run.output);
    assert!(
        run.requests("POST", "/docs/pulls").is_empty(),
        "{:#?}",
        run.calls
    );
    assert!(
        run.output.contains("[targets.docs]: waits for the release of go (the repository holding it isn't on GitHub), rust (")
            && run.output.contains("then opens its pull request on file://"),
        "{}",
        run.output
    );
}
