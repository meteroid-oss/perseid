//! Packs: programs wrapping an SDK, such as a CLI over the Rust SDK, rendered from templates of
//! their own against the model of that SDK, into a repository of their own.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    MODEL_VERSION, assets,
    config::{Config, Language, Sdk},
    format, fsx,
    generate::{self, Change, GENERATION, Layout, Options, Stale},
    generator, pr,
    targets::{Kind, Target},
};

/// The manifest at the root of a pack.
pub const MANIFEST: &str = "pack.toml";

/// Where a pack's templates find those of the SDK it wraps.
pub(crate) const SDK_TEMPLATES: &str = "sdk/";

/// The spec a pack target was last generated from, which sizes its next pull request.
pub const SPEC: &str = ".perseid/openapi.json";

/// `pack.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    /// The SDK languages it can wrap.
    pub wraps: Vec<Language>,
    /// The version of the model its templates read, [`MODEL_VERSION`].
    pub model: u32,
    /// Each template of `templates/` by name, which says what it renders per.
    pub templates: BTreeMap<String, Output>,
    /// Where `runtime/` is vendored, relative to the target's folder.
    pub runtime: Option<String>,
    /// The command formatting the generated files, given their paths: the wrapped SDK's
    /// formatter by default, none when empty.
    pub format: Option<Vec<String>>,
}

/// Where a template renders.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// Its folder, relative to the target's.
    pub dir: String,
    /// The extension of its files: that of the wrapped SDK's language by default.
    pub extension: Option<String>,
}

/// A pack, checked.
#[derive(Debug)]
pub struct Pack {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Pack {
    /// The pack at `dir`, checked to wrap the SDK of `language`.
    pub fn load(dir: &Path, language: &str) -> Result<Self> {
        let path = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("no pack at {}: {MANIFEST} is missing", dir.display()))?;
        let manifest: Manifest =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let pack = Pack {
            dir: dir.to_owned(),
            manifest,
        };
        pack.check(language)
            .with_context(|| format!("in {}", path.display()))?;
        Ok(pack)
    }

    fn check(&self, language: &str) -> Result<()> {
        let Manifest {
            name,
            wraps,
            model,
            templates,
            runtime,
            ..
        } = &self.manifest;
        ensure!(!name.is_empty(), "`name` is empty");
        ensure!(
            wraps.iter().any(|l| l.name() == language),
            "the {name} pack wraps {}, not {language}",
            wraps
                .iter()
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
        ensure!(
            *model == MODEL_VERSION,
            "the {name} pack reads version {model} of the model, and perseid {} gives version {MODEL_VERSION}: {}",
            env!("CARGO_PKG_VERSION"),
            match *model > MODEL_VERSION {
                true => "upgrade perseid",
                false => "use a version of the pack written for it",
            }
        );
        ensure!(!templates.is_empty(), "`templates` lists none");
        ensure!(
            !self.dir.join("templates").join(SDK_TEMPLATES).exists(),
            "templates/{SDK_TEMPLATES} is where templates find those of the SDK: rename it"
        );
        for (template, output) in templates {
            ensure!(
                generator::TEMPLATES.contains(&template.as_str()),
                "[templates] `{template}`: name each template after what it renders per, among {}",
                generator::TEMPLATES.join(", ")
            );
            ensure!(
                folder(&output.dir),
                "[templates] `{template}`: `dir = {:?}` must be a folder of the target, such as \"src\"",
                output.dir
            );
            let file = self.template(template, output, language);
            ensure!(
                self.dir.join("templates").join(&file).is_file(),
                "[templates] `{template}`: templates/{file} is missing"
            );
        }
        let vendored = self.dir.join("runtime").is_dir();
        match runtime {
            Some(dir) => {
                ensure!(
                    folder(dir),
                    "`runtime = {dir:?}` must be a folder of the target, such as \"src/runtime\""
                );
                ensure!(vendored, "`runtime = {dir:?}`, but there is no runtime/");
            }
            None => ensure!(!vendored, "set `runtime`, the folder runtime/ goes to"),
        }
        Ok(())
    }

    /// The file of `template`, relative to `templates/`.
    fn template(&self, template: &str, output: &Output, language: &str) -> String {
        let extension = (output.extension.as_deref()).unwrap_or(generate::extension(language));
        format!("{template}.{extension}.jinja")
    }
}

/// Whether `path` is a folder below the one it is relative to, or that one (`.`), outside the
/// hidden folders such as `.git` and `.perseid`.
fn folder(path: &str) -> bool {
    path == "."
        || !path.is_empty()
            && !path.starts_with('/')
            && (path.trim_end_matches('/').split('/')).all(|p| !p.is_empty() && !p.starts_with('.'))
}

/// What generating a pack target changed in its folder, and the scaffold files among them.
pub struct Generated {
    pub changes: Vec<Change>,
    pub scaffold: Vec<PathBuf>,
}

/// Generates the pack `target` into `dir`, against the SDK it wraps at the version `released`,
/// else the one where `generate` writes it. With `check`, only reports what would change.
pub fn generate(
    config: &Config,
    root: &Path,
    target: &Target,
    dir: &Path,
    spec: &str,
    released: Option<&str>,
    options: &Options,
) -> Result<Generated> {
    // Templates recurse through nested types, as when generating.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(scope, || {
                run(config, root, target, dir, spec, released, options)
                    .with_context(|| format!("generating [targets.{}]", target.name))
            })
            .expect("spawning a pack thread")
            .join()
            .expect("pack thread panicked")
    })
}

fn run(
    config: &Config,
    root: &Path,
    target: &Target,
    dir: &Path,
    spec: &str,
    released: Option<&str>,
    options: &Options,
) -> Result<Generated> {
    let Kind::Pack { dir: from, wraps } = &target.kind else {
        bail!("[targets.{}] isn't a pack", target.name);
    };
    let pack = Pack::load(&root.join(from), wraps)?;
    let sdk = config.sdks(&[(*wraps).to_owned()])?.remove(0);
    let (context, pack_value) = contexts(config, root, &sdk, &pack, target, released);
    let mut tokens = context.clone();
    for key in ["name", "target", "repo", "path"] {
        tokens[format!("pack_{key}")] = pack_value[key].clone();
    }
    std::fs::create_dir_all(dir)?;
    let scaffolded = scaffold_files(&pack, &tokens)?;
    let globals = Map::from_iter([
        ("sdk".to_owned(), context.clone()),
        ("pack".to_owned(), pack_value),
    ]);
    let stage = generate::stage(dir)?;
    let produced = generate::logged(&target.name, || {
        render(
            config,
            root,
            &pack,
            &sdk,
            &globals,
            &tokens,
            spec,
            stage.path(),
        )
    })?;
    let overwritten: Vec<_> = (scaffolded.iter())
        .filter(|(_, path)| produced.contains(path))
        .map(|(file, _)| file.display().to_string())
        .collect();
    ensure!(
        overwritten.is_empty(),
        "the scaffold of the {} pack writes files its templates or runtime generate: {}",
        pack.manifest.name,
        overwritten.join(", ")
    );
    let mut scaffold = Vec::new();
    if !options.check && !dir.join(GENERATION).exists() {
        scaffold = self::scaffold(&pack, dir, &tokens, &scaffolded)?;
    }
    if options.format {
        let staged = generate::staged(dir, stage.path(), &produced);
        match pack.manifest.format.as_deref() {
            None => format::format(sdk.language, dir, &staged)?,
            Some([]) => {}
            Some(command) => run_format(command, dir, &staged)?,
        }
    }
    let stale = Stale::Recorded;
    let mut changes = generate::settle(dir, stage.path(), produced, &stale, options.check)?;
    if !changes.is_empty() && !options.check {
        let pretty = serde_json::to_string_pretty(&serde_json::from_str::<Value>(spec)?)? + "\n";
        match std::fs::read_to_string(dir.join(SPEC)) {
            Ok(old) if old == pretty => {}
            Ok(_) => changes.push(Change::Modified(SPEC.into())),
            Err(_) => changes.push(Change::Added(SPEC.into())),
        }
        fsx::write(&dir.join(SPEC), pretty.as_bytes())?;
    }
    changes.extend(scaffold.iter().cloned().map(Change::Added));
    changes.sort_by_key(|c| match c {
        Change::Added(p) | Change::Modified(p) | Change::Removed(p) => p.clone(),
    });
    Ok(Generated { changes, scaffold })
}

/// The globals of the pack's templates: `sdk`, the context the wrapped SDK's templates see, at
/// the version the pack depends on, and `pack`.
fn contexts(
    config: &Config,
    root: &Path,
    sdk: &Sdk,
    pack: &Pack,
    target: &Target,
    released: Option<&str>,
) -> (Value, Value) {
    let dir = match sdk.remote() {
        Some(repo) => pr::checkout_dir(repo, root).join(&sdk.path),
        None => root.join(&sdk.path),
    };
    let mut context = config.context(sdk, &dir);
    if let Some(version) = released {
        context["version"] = version.into();
    }
    let pack = json!({
        "name": pack.manifest.name,
        "target": target.name,
        "repo": target.repo,
        "path": target.path,
        "sdk": {
            "language": sdk.language,
            "package": config.package(sdk),
            "version": context["version"],
            "released": released.is_some(),
        },
    });
    (context, pack)
}

/// Renders the templates of `pack`, then its runtime, into `stage`.
#[allow(clippy::too_many_arguments)]
fn render(
    config: &Config,
    root: &Path,
    pack: &Pack,
    sdk: &Sdk,
    globals: &Map<String, Value>,
    tokens: &Value,
    spec: &str,
    stage: &Path,
) -> Result<Vec<PathBuf>> {
    let language = sdk.language;
    let assets_dir = tempfile::tempdir()?;
    assets::materialize(
        language,
        Some(&config.overrides_dir(root)),
        assets_dir.path(),
    )?;
    let templates = assets_dir.path().join("pack");
    let own = pack.dir.join("templates");
    for file in assets::walk_sources(&own)? {
        fsx::write(
            &templates.join(file.strip_prefix(&own)?),
            &std::fs::read(&file)?,
        )?;
    }
    std::fs::rename(
        assets_dir.path().join("templates").join(language),
        templates.join(SDK_TEMPLATES),
    )?;
    let extension = generate::extension(language);
    let manifest = &pack.manifest;
    let layout = Layout {
        templates,
        tasks: (manifest.templates.iter())
            .map(|(template, output)| {
                let output_extension = output.extension.as_deref().unwrap_or(extension);
                (
                    template.clone(),
                    PathBuf::from(&output.dir),
                    output_extension.to_owned(),
                )
            })
            .collect(),
        runtime: (
            pack.dir.join("runtime"),
            PathBuf::from(manifest.runtime.as_deref().unwrap_or(".")),
        ),
        origin: pack.dir.join("runtime").display().to_string(),
    };
    let api = generate::sdk_api(config, sdk, &globals["sdk"], spec)?;
    let produced = generate::draw(&api, globals, &layout, tokens, extension, stage)?;
    generate::checked(stage, &produced)?;
    Ok(produced)
}

/// The files of the pack's `scaffold/`, each with its path in the target: `tokens` substituted.
fn scaffold_files(pack: &Pack, tokens: &Value) -> Result<Vec<(PathBuf, PathBuf)>> {
    let from = pack.dir.join("scaffold");
    if !from.is_dir() {
        return Ok(vec![]);
    }
    let mut files = Vec::new();
    for file in assets::walk_sources(&from)? {
        let relative = file.strip_prefix(&pack.dir)?.to_owned();
        let name = (file.strip_prefix(&from)?.to_str()).context("non UTF-8 path")?;
        let path = generate::tokens(name, tokens)?;
        ensure!(
            generate::inside(Path::new(&path)),
            "{} goes to {path:?}, outside the target",
            relative.display()
        );
        files.push((relative, generate::clean(Path::new(&path))));
    }
    Ok(files)
}

/// Writes the `files` of the pack's scaffold that `dir` lacks, with `tokens` substituted in their
/// text. Returns their paths relative to `dir`.
fn scaffold(
    pack: &Pack,
    dir: &Path,
    tokens: &Value,
    files: &[(PathBuf, PathBuf)],
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (file, path) in files {
        if dir.join(path).exists() {
            continue;
        }
        let content = std::fs::read(pack.dir.join(file))?;
        let content = match String::from_utf8(content) {
            Ok(text) => generate::tokens(&text, tokens)?.into_bytes(),
            Err(binary) => binary.into_bytes(),
        };
        fsx::write(&dir.join(path), &content)?;
        written.push(path.clone());
    }
    Ok(written)
}

/// Runs the formatter `command` of a pack in `dir` on `files`.
fn run_format(command: &[String], dir: &Path, files: &[PathBuf]) -> Result<()> {
    let shown = command.join(" ");
    let output = Command::new(&command[0])
        .args(&command[1..])
        .args(files)
        .current_dir(dir)
        .output()
        .with_context(|| format!("running `{shown}`, the pack's `format`"))?;
    ensure!(
        output.status.success(),
        "`{shown}` failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A copy of the toy pack, changed by `edit`.
    fn toy(edit: impl FnOnce(&Path)) -> tempfile::TempDir {
        let toy = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packs/toy");
        let copy = tempfile::tempdir().unwrap();
        for file in assets::walk(&toy).unwrap() {
            let to = copy.path().join(file.strip_prefix(&toy).unwrap());
            fsx::write(&to, &std::fs::read(&file).unwrap()).unwrap();
        }
        edit(copy.path());
        copy
    }

    /// The error loading the toy pack for `language` once `edit` changed a copy of it.
    fn error(language: &str, edit: impl FnOnce(&Path)) -> String {
        let copy = toy(edit);
        let error = Pack::load(copy.path(), language).expect_err("an invalid pack");
        format!("{error:#}")
    }

    /// A project generating the pack at `pack` as `[targets.cli]`, from the petstore spec into
    /// `cli/`, with `settings` at the top of its perseid.toml.
    struct Project {
        root: tempfile::TempDir,
        config: Config,
        spec: String,
    }

    impl Project {
        fn new(pack: &Path, settings: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let toml = format!(
                "name = \"Acme\"\nsdks = [\"rust\"]\n{settings}\n[targets.cli]\npack = {:?}\nwraps = \"rust\"\nrepo = \"acme/cli\"\n",
                pack.display()
            );
            let config = Config::parse(&toml, "perseid.toml").unwrap();
            let petstore =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/petstore.yaml");
            let spec = crate::spec::read(petstore.to_str().unwrap(), root.path()).unwrap();
            Project { root, config, spec }
        }

        fn dir(&self) -> PathBuf {
            self.root.path().join("cli")
        }

        fn generate(&self, check: bool, format: bool) -> Result<Generated> {
            let target = self.config.targets(&[])?.remove(0);
            let options = Options { check, format };
            let root = self.root.path();
            generate(
                &self.config,
                root,
                &target,
                &self.dir(),
                &self.spec,
                None,
                &options,
            )
        }

        fn shown(&self, check: bool) -> Vec<String> {
            let generated = self.generate(check, false).unwrap();
            generated.changes.iter().map(ToString::to_string).collect()
        }

        fn write(&self, path: &str, text: &str) {
            fsx::write(&self.dir().join(path), text.as_bytes()).unwrap();
        }

        fn read(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(self.dir().join(path)).ok()
        }
    }

    #[test]
    fn packs_remove_only_the_files_they_generated_before() {
        let pack = toy(|_| {});
        let project = Project::new(pack.path(), "");
        let foreign = [
            (
                "src/proto.rs",
                "// This file is @generated by prost-build.\n",
            ),
            (
                "docs/api.md",
                "<!-- this file is @generated by another SDK -->\n",
            ),
            (
                "src/commands/old.rs",
                "// This file is @generated by perseid.\n",
            ),
        ];
        for (path, text) in foreign {
            project.write(path, text);
        }
        let first = project.shown(false);
        assert!(first.contains(&"+ api.md".to_owned()), "{first:?}");
        assert!(!first.iter().any(|c| c.starts_with('-')), "{first:?}");
        let record: Value = serde_json::from_str(&project.read(GENERATION).unwrap()).unwrap();
        assert_eq!(
            record["paths"],
            json!([
                "api.md",
                "src/commands/mod.rs",
                "src/commands/pets.rs",
                "src/runtime/output.rs"
            ])
        );

        let text = std::fs::read_to_string(pack.path().join(MANIFEST)).unwrap();
        let text = text.replace("api_reference = { dir = \".\", extension = \"md\" }\n", "");
        std::fs::write(pack.path().join(MANIFEST), text).unwrap();
        std::fs::remove_file(pack.path().join("templates/api_reference.md.jinja")).unwrap();
        project.write("src/runtime/mine.rs", "// handwritten\n");
        assert_eq!(
            project.shown(true),
            ["~ .perseid/generation.json", "- api.md"]
        );
        assert!(project.read("api.md").is_some(), "--check must not write");
        assert_eq!(
            project.shown(false),
            ["~ .perseid/generation.json", "- api.md"]
        );
        assert!(project.read("api.md").is_none());
        for (path, text) in foreign {
            assert_eq!(project.read(path).as_deref(), Some(text), "{path}");
        }
        assert!(project.read("src/runtime/mine.rs").is_some());
        assert!(project.shown(false).is_empty());
    }

    #[test]
    fn records_naming_paths_outside_the_target_are_ignored() {
        let pack = toy(|_| {});
        let project = Project::new(pack.path(), "");
        project.generate(false, false).unwrap();
        let outside = project.root.path().join("outside.rs");
        std::fs::write(&outside, "// This file is @generated by perseid.\n").unwrap();
        let record = project.read(GENERATION).unwrap();
        let tampered = record.replace("\"api.md\"", "\"../outside.rs\", \"api.md\"");
        project.write(GENERATION, &tampered);
        project.generate(false, false).unwrap();
        assert!(outside.exists());
    }

    #[test]
    fn scaffolds_neither_overwrite_generated_files_nor_leave_the_target() {
        let colliding = toy(|dir| {
            let runtime = std::fs::read(dir.join("runtime/output.rs")).unwrap();
            fsx::write(&dir.join("scaffold/src/runtime/output.rs"), &runtime).unwrap();
        });
        let error = format!(
            "{:#}",
            Project::new(colliding.path(), "")
                .generate(false, false)
                .err()
                .unwrap()
        );
        assert!(
            error.contains(
                "the scaffold of the toy pack writes files its templates or runtime generate: scaffold/src/runtime/output.rs"
            ),
            "{error}"
        );
        let escaping = toy(|dir| fsx::write(&dir.join("scaffold/@@WHERE@@/x"), b"").unwrap());
        let pack = Pack::load(escaping.path(), "rust").unwrap();
        for (place, path) in [("../..", "../../x"), ("/etc", "/etc/x")] {
            let tokens = json!({ "where": place });
            let error = format!("{:#}", scaffold_files(&pack, &tokens).unwrap_err());
            assert!(
                error.contains(&format!(
                    "scaffold/@@WHERE@@/x goes to {path:?}, outside the target"
                )),
                "{error}"
            );
        }
    }

    #[test]
    fn build_output_in_a_pack_is_left_out() {
        let pack = toy(|dir| {
            for path in [
                "runtime/target/.rustc_info.json",
                "runtime/node_modules/x/index.js",
                "scaffold/target/debug/cli",
                "templates/target/x.jinja",
            ] {
                fsx::write(&dir.join(path), b"{}").unwrap();
            }
            fsx::write(&dir.join("scaffold/.gitignore"), b"/target\n").unwrap();
        });
        let project = Project::new(pack.path(), "");
        let generated = project.generate(false, false).unwrap();
        assert_eq!(
            generated.scaffold,
            [
                PathBuf::from(".github/workflows/release.yml"),
                ".gitignore".into(),
                "Cargo.toml".into(),
                "src/main.rs".into()
            ]
        );
        assert!(
            project
                .read("src/runtime/target/.rustc_info.json")
                .is_none()
        );

        let unmarked =
            toy(|dir| fsx::write(&dir.join("runtime/util.rs"), b"pub fn f() {}\n").unwrap());
        let error = format!(
            "{:#}",
            Project::new(unmarked.path(), "")
                .generate(false, false)
                .err()
                .unwrap()
        );
        let source = unmarked.path().join("runtime/util.rs");
        assert!(
            error.contains(&format!(
                "src/runtime/util.rs lacks the `@generated` marker in its first lines: add it to {}",
                source.display()
            )),
            "{error}"
        );
    }

    #[test]
    fn runtime_features_are_copied_when_the_sdk_turns_them_on() {
        let pack = toy(|dir| {
            for feature in ["webhooks", "tests"] {
                let path = dir.join(format!("runtime/features/{feature}/{feature}.rs"));
                fsx::write(&path, b"// This file is @generated by perseid.\n").unwrap();
            }
        });
        let project = Project::new(pack.path(), "");
        project.generate(false, false).unwrap();
        assert!(project.read("src/runtime/tests.rs").is_some());
        assert!(project.read("src/runtime/webhooks.rs").is_none());
        let project = Project::new(pack.path(), "webhooks = true");
        project.generate(false, false).unwrap();
        assert!(project.read("src/runtime/webhooks.rs").is_some());
    }

    #[test]
    fn templates_include_sdk_templates_that_import_others() {
        let pack = toy(|dir| {
            manifest(|t| format!("{t}component_type = {{ dir = \"src/models\" }}\n"))(dir);
            let template = "{% include \"sdk/component_type.rs.jinja\" %}\n";
            fsx::write(
                &dir.join("templates/component_type.rs.jinja"),
                template.as_bytes(),
            )
            .unwrap();
        });
        let project = Project::new(pack.path(), "");
        project.generate(false, false).unwrap();
        let pet = project.read("src/models/pet.rs").unwrap();
        assert!(pet.contains("pub struct Pet {"), "{pet}");
    }

    #[cfg(unix)]
    #[test]
    fn a_pack_may_format_with_a_command_of_its_own() {
        let pack = toy(manifest(|t| {
            let command =
                r#"format = ["sh", "-c", "for f; do echo '// formatted' >> \"$f\"; done", "fmt"]"#;
            t.replace("runtime = ", &format!("{command}\nruntime = "))
        }));
        let project = Project::new(pack.path(), "");
        project.generate(false, true).unwrap();
        for path in ["api.md", "src/commands/pets.rs", "src/runtime/output.rs"] {
            let text = project.read(path).unwrap();
            assert!(text.ends_with("// formatted\n"), "{path}: {text}");
        }
        assert!(!project.read("Cargo.toml").unwrap().contains("formatted"));
        assert!(project.generate(true, true).unwrap().changes.is_empty());

        let failing = toy(manifest(|t| {
            t.replace("runtime = ", "format = [\"false\"]\nruntime = ")
        }));
        let error = format!(
            "{:#}",
            Project::new(failing.path(), "")
                .generate(false, true)
                .err()
                .unwrap()
        );
        assert!(error.contains("`false` failed"), "{error}");
    }

    fn manifest(edit: impl Fn(&str) -> String) -> impl FnOnce(&Path) {
        move |dir| {
            let path = dir.join(MANIFEST);
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, edit(&text)).unwrap();
        }
    }

    #[test]
    fn the_toy_pack_wraps_rust() {
        let toy = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packs/toy");
        let pack = Pack::load(&toy, "rust").unwrap();
        assert_eq!(pack.manifest.name, "toy");
        assert_eq!(
            pack.manifest.templates.keys().collect::<Vec<_>>(),
            ["api_reference", "api_resource", "api_summary"]
        );
    }

    #[test]
    fn manifests_are_checked_against_the_model_and_the_wrapped_sdk() {
        let typescript = error("typescript", |_| {});
        assert!(
            typescript.contains("pack.toml: the toy pack wraps rust, not typescript"),
            "{typescript}"
        );
        let newer = error("rust", manifest(|t| t.replace("model = 1", "model = 2")));
        assert!(
            newer.contains("the toy pack reads version 2 of the model, and perseid")
                && newer.contains("gives version 1: upgrade perseid"),
            "{newer}"
        );
        let older = error("rust", manifest(|t| t.replace("model = 1", "model = 0")));
        assert!(
            older.ends_with("use a version of the pack written for it"),
            "{older}"
        );
        for (edit, expected) in [
            (
                ("api_summary =", "api_index ="),
                "[templates] `api_index`: name each template after what it renders per, among api_resource,",
            ),
            (
                ("dir = \".\"", "dir = \"../docs\""),
                "[templates] `api_reference`: `dir = \"../docs\"` must be a folder of the target",
            ),
            (
                ("extension = \"md\"", "extension = \"txt\""),
                "[templates] `api_reference`: templates/api_reference.txt.jinja is missing",
            ),
            (
                ("runtime = \"src/runtime\"\n", ""),
                "set `runtime`, the folder runtime/ goes to",
            ),
            (("wraps", "wrap"), "unknown field `wrap`"),
        ] {
            let text = error("rust", manifest(move |t| t.replace(edit.0, edit.1)));
            assert!(text.contains(expected), "{edit:?}: {text}");
        }
        let reserved = error("rust", |dir| {
            fsx::write(&dir.join("templates/sdk/x.jinja"), b"").unwrap();
        });
        assert!(
            reserved.contains("templates/sdk/ is where templates find those of the SDK"),
            "{reserved}"
        );
        let missing = format!(
            "{:#}",
            Pack::load(Path::new("/nonexistent"), "rust").unwrap_err()
        );
        assert!(
            missing.contains("no pack at /nonexistent: pack.toml is missing"),
            "{missing}"
        );
    }

    #[test]
    fn folders_stay_within_the_target() {
        for ok in [".", "src", "src/commands/", "crates/cli"] {
            assert!(folder(ok), "{ok}");
        }
        for bad in [
            "",
            "/src",
            "..",
            "src/../..",
            "a//b",
            "./src",
            ".github/workflows",
            ".perseid",
            "src/.cache",
        ] {
            assert!(!folder(bad), "{bad}");
        }
    }
}
