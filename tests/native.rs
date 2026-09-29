use perseid::{
    project::{
        io,
        prepare::{Preparation, prepare},
    },
    runner::render_runtime,
};
use serde_json::json;
use std::{collections::BTreeMap, fs};

#[test]
fn runtime_tokens_are_explicit_and_required() {
    let context = BTreeMap::from([("client_name".into(), json!("Example"))]);
    assert_eq!(
        render_runtime("@@CLIENT_NAME@@ {ordinary}", &context).unwrap(),
        "Example {ordinary}"
    );
    assert!(render_runtime("@@MISSING@@", &context).is_err());
}
#[test]
fn repository_paths_cannot_escape_or_follow_symlinks() {
    let root = tempfile::tempdir().unwrap();
    assert!(io::relative(root.path(), "../outside").is_err());
    assert!(io::relative(root.path(), "temp/../inside").is_err());
    assert!(io::relative(root.path(), "/outside").is_err());
    #[cfg(unix)]
    {
        let victim = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(victim.path(), root.path().join("link")).unwrap();
        assert!(io::relative(root.path(), "link/openapi.json").is_err());
        assert!(io::files(root.path(), false).is_err());
    }
}
#[test]
fn preparation_promotes_inline_objects_and_reports_excluded_endpoints() {
    let spec = json!({"openapi":"3.0.3","info":{"title":"Example","version":"1"},"paths":{
        "/widgets":{"post":{"operationId":"create_widget","tags":["super::café"],"requestBody":{"content":{"application/json":{"schema":{"type":"object","properties":{"nested":{"type":"object","properties":{"value":{"type":"string"}}}}}}}},"responses":{"200":{"description":"ok"}}}},
        "/socket":{"get":{"operationId":"socket","responses":{"101":{"description":"upgrade"}}}}
    },"components":{"schemas":{}}});
    assert!(prepare(spec.clone(), "source.json", &Preparation::default()).is_err());
    let (prepared, coverage) = prepare(
        spec,
        "source.json",
        &Preparation {
            exclude_unsupported: true,
            normalize_tags: true,
        },
    )
    .unwrap();
    assert_eq!(
        prepared["paths"]["/widgets"]["post"]["tags"],
        json!(["cafe"])
    );
    assert_eq!(
        prepared["components"]["schemas"]["CreateWidgetRequest"]["properties"]["nested"]["$ref"],
        json!("#/components/schemas/CreateWidgetRequestNested")
    );
    assert_eq!(coverage["included_operations"], 1);
    assert_eq!(coverage["excluded_operations"][0]["operation_id"], "socket");
}
#[test]
fn atomic_write_preserves_existing_executable_bit() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("script");
    io::write(&file, b"before").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    io::write(&file, b"after").unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"after");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
