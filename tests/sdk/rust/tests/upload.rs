use petstore::api::Upload;

#[tokio::test]
async fn uploads_from_a_path_are_named_and_typed_after_it() {
    let dir = std::env::temp_dir().join(format!("petstore-upload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = [
        ("pets.CSV", "text/csv"),
        ("photo.jpeg", "image/jpeg"),
        ("batch.jsonl", "application/jsonl"),
        ("archive.tar.gz", "application/octet-stream"),
        ("README", "application/octet-stream"),
    ];
    for (name, content_type) in cases {
        let path = dir.join(name);
        std::fs::write(&path, b"id,name\n1,Rex\n").unwrap();
        let upload = Upload::path(&path).await.unwrap();
        let debug = format!("{upload:?}");
        assert!(
            debug.contains(&format!("filename: Some({name:?})")),
            "{debug}"
        );
        assert!(
            debug.contains(&format!("content_type: {content_type:?}")),
            "{debug}"
        );
        assert!(debug.contains("length: Some(14)"), "{debug}");
    }
    let missing = Upload::path(dir.join("missing.png")).await.unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    std::fs::remove_dir_all(dir).unwrap();
}
