//! Bootstrap assets ship inside each binary, so init never fetches moving templates.
use std::collections::BTreeSet;

pub fn files(languages: &BTreeSet<&String>) -> Vec<(&'static str, String)> {
    let mut setup = String::new();
    let mut formatters = String::new();
    for language in languages {
        match language.as_str() {
            "rust" => {
                setup.push_str("      - uses: dtolnay/rust-toolchain@stable\n");
                formatters.push_str(r#"rustup toolchain install nightly-2025-02-27 --profile minimal --component rustfmt
cat > "$PERSEID_TOOLS/bin/rustfmt" <<'FORMATTER'
#!/usr/bin/env bash
exec rustup run nightly-2025-02-27 rustfmt "$@"
FORMATTER
chmod +x "$PERSEID_TOOLS/bin/rustfmt"
"#);
            }
            "typescript" => {
                setup.push_str("      - uses: actions/setup-node@v4\n        with:\n          node-version: '22'\n");
                formatters.push_str(r#"npm install --prefix "$PERSEID_TOOLS/formatters" --ignore-scripts --no-audit --no-fund @biomejs/biome@2.1.4
printf '%s\n' "$PERSEID_TOOLS/formatters/node_modules/.bin" >> "$GITHUB_PATH"
"#);
            }
            "python" => {
                setup.push_str("      - uses: actions/setup-python@v5\n        with:\n          python-version: '3.12'\n");
                formatters.push_str("python -m pip install ruff==0.14.10\n");
            }
            "go" => {
                setup.push_str("      - uses: actions/setup-go@v5\n        with:\n          go-version: '1.24.7'\n          cache: false\n");
            }
            "java" => {
                setup.push_str("      - uses: actions/setup-java@v4\n        with:\n          distribution: temurin\n          java-version: '21'\n      - uses: gradle/actions/setup-gradle@v4\n        with:\n          gradle-version: '8.12'\n");
                formatters.push_str(r#"curl --fail --silent --show-error --location https://github.com/google/google-java-format/releases/download/v1.25.2/google-java-format-1.25.2-all-deps.jar -o "$PERSEID_TOOLS/google-java-format.jar"
printf '%s  %s\n' 25157797a0a972c2290b5bc71530c4f7ad646458025e3484412a6e5a9b8c9aa6 "$PERSEID_TOOLS/google-java-format.jar" | sha256sum --check
cat > "$PERSEID_TOOLS/bin/google-java-format" <<'FORMATTER'
#!/usr/bin/env bash
exec java -jar "$(dirname "$0")/../google-java-format.jar" "$@"
FORMATTER
chmod +x "$PERSEID_TOOLS/bin/google-java-format"
"#);
            }
            _ => unreachable!(),
        }
    }
    setup.push_str("      - name: Install released Perseid and selected formatters\n        run: bash .perseid/setup-ci.sh\n");
    vec![
        (
            ".github/workflows/update-sdks.yml",
            include_str!("scaffold/update-sdks.yml").replace("@@SETUP@@", setup.trim_end()),
        ),
        (
            ".github/workflows/check-sdks.yml",
            include_str!("scaffold/check-sdks.yml").replace("@@SETUP@@", setup.trim_end()),
        ),
        (
            ".perseid/setup-ci.sh",
            include_str!("scaffold/setup-ci.sh")
                .replace("@@VERSION@@", env!("CARGO_PKG_VERSION"))
                .replace("@@FORMATTERS@@", formatters.trim_end()),
        ),
    ]
}

pub fn notification(repository: &str, branch: &str, spec: &str) -> String {
    include_str!("scaffold/notify-openapi.yml")
        .replace(
            "@@REPOSITORY@@",
            &serde_json::to_string(repository).unwrap(),
        )
        .replace("@@BRANCH@@", &serde_json::to_string(branch).unwrap())
        .replace("@@SPEC@@", &serde_json::to_string(spec).unwrap())
}
