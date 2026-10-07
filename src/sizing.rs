//! `--bump auto`: the release size of a spec change and its changelog, from oasdiff comparing the
//! spec with its previous version.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

use crate::{changelog::Changelog, config::Source, pr::Bump};

/// A workflow command in GitHub Actions, a plain line on stderr elsewhere.
fn annotate(level: &str, message: &str) {
    match std::env::var("GITHUB_ACTIONS").as_deref() {
        Ok("true") => println!("::{level}::{message}"),
        _ => eprintln!("{level}: {message}"),
    }
}

/// The spec file at the commit before the pushed ones (`GITHUB_EVENT_BEFORE`), or else at the
/// previous commit.
fn previous(root: &Path, file: &str) -> Option<String> {
    let rev = match std::env::var("GITHUB_EVENT_BEFORE") {
        Ok(sha) if !sha.trim_matches('0').is_empty() => {
            crate::pr::git(root, &["fetch", "--quiet", "--depth", "1", "origin", &sha]).ok()?;
            sha
        }
        _ => "HEAD~1".to_owned(),
    };
    crate::pr::git(root, &["show", &format!("{rev}:./{file}")]).ok()
}

/// What oasdiff compares: the base spec and the current one, `spec` read from `root`.
pub struct Comparison<'a> {
    pub root: &'a Path,
    pub spec: &'a str,
    /// The previous spec, found through git when absent.
    pub base: Option<&'a Path>,
    /// Counts enum values added to responses as minor changes instead of breaking ones.
    pub relax_enum_additions: bool,
}

/// The bump of the change from the base spec to the current one, and its changelog.
/// Without a base spec or oasdiff, a minor release.
pub fn size(c: &Comparison) -> Result<(Bump, Changelog)> {
    let scratch = tempfile::tempdir()?;
    let current: OsString = match Source::parse(c.spec) {
        Source::File(file) => c.root.join(file).into(),
        Source::Url(url) => url.into(),
    };
    let previous = match (c.base, Source::parse(c.spec)) {
        (None, Source::File(file)) => previous(c.root, file),
        _ => None,
    };
    let base: PathBuf = match (c.base, &previous) {
        (Some(base), _) => base.to_owned(),
        (None, Some(text)) => {
            let path = scratch.path().join("base-spec");
            std::fs::write(&path, text)?;
            path
        }
        (None, None) => {
            annotate(
                "notice",
                "no previous spec to compare with, asking for a minor release",
            );
            return Ok((Bump::Minor, Changelog::default()));
        }
    };
    if !crate::format::on_path("oasdiff") {
        annotate(
            "warning",
            "oasdiff isn't installed (`perseid tools install` installs it), asking for a minor release",
        );
        return Ok((Bump::Minor, Changelog::default()));
    }
    let current = match converted(scratch.path(), "spec.json", c.spec, c.root, None)? {
        Some(path) => path.into(),
        None => current,
    };
    // The previous spec's references are read from the spec's location, as no other files were
    // kept from its commit.
    let (location, text) = match &previous {
        Some(text) if c.base.is_none() => (c.spec.to_owned(), Some(text.as_str())),
        _ => (base.to_string_lossy().into_owned(), None),
    };
    let base =
        converted(scratch.path(), "base-spec.json", &location, c.root, text)?.unwrap_or(base);
    let mut levels: Vec<OsString> = vec![];
    if c.relax_enum_additions {
        let path = scratch.path().join("oasdiff-levels");
        std::fs::write(&path, "response-property-enum-value-added info\n")?;
        levels = vec!["--severity-levels".into(), path.into()];
    }
    let output = Command::new("oasdiff")
        .args(["changelog", "--format", "json"])
        .args(&levels)
        .arg(&base)
        .arg(&current)
        .output()
        .context("running oasdiff")?;
    if !output.status.success() {
        annotate(
            "warning",
            &format!(
                "`oasdiff changelog` failed, asking for a minor release: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        );
        return Ok((Bump::Minor, Changelog::default()));
    }
    let changelog = Changelog::of_oasdiff(&output.stdout)?;
    let bump = match () {
        _ if changelog.breaking() => Bump::Major,
        _ if !changelog.is_empty() => Bump::Minor,
        _ => Bump::Patch,
    };
    Ok((bump, changelog))
}

/// The spec at `location` (`text` when given), converted to OpenAPI 3.0 into `scratch` when it is
/// Swagger 2.0: oasdiff reads 2.0 as 3.0, missing its request bodies. `None` for any other spec,
/// or when the conversion fails, with a warning: oasdiff reads the spec as it is then.
fn converted(
    scratch: &Path,
    name: &str,
    location: &str,
    root: &Path,
    text: Option<&str>,
) -> Result<Option<PathBuf>> {
    let converted = match crate::spec::swagger_2_as_3_0(location, root, text) {
        Ok(converted) => converted,
        Err(error) => {
            let version = if text.is_some() { "previous " } else { "" };
            annotate(
                "warning",
                &format!(
                    "reading the {version}{location} for oasdiff failed, it compares the file as \
                     it is: {error:#}"
                ),
            );
            None
        }
    };
    let Some(converted) = converted else {
        return Ok(None);
    };
    let path = scratch.join(name);
    std::fs::write(&path, converted)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"openapi: 3.1.0
info: { title: Pets, version: "1" }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  status: { type: string, enum: [available, sold] }
  /pets/{id}:
    delete:
      operationId: deletePet
      parameters: [{ name: id, in: path, required: true, schema: { type: string } }]
      responses: { "204": { description: deleted } }
"#;

    fn size_of(next: &str, relax: bool) -> (Bump, Changelog) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("base.yaml"), BASE).unwrap();
        std::fs::write(dir.path().join("openapi.yaml"), next).unwrap();
        let base = dir.path().join("base.yaml");
        size(&Comparison {
            root: dir.path(),
            spec: "openapi.yaml",
            base: Some(&base),
            relax_enum_additions: relax,
        })
        .unwrap()
    }

    #[test]
    fn oasdiff_sizes_spec_changes() {
        if !crate::format::on_path("oasdiff") {
            eprintln!("oasdiff isn't installed, skipped");
            return;
        }
        assert_eq!(size_of(BASE, true), (Bump::Patch, Changelog::default()));
        let described = BASE.replace("description: deleted", "description: gone for good");
        assert_eq!(size_of(&described, true).0, Bump::Patch);
        let added = BASE.replace(
            "  /pets/{id}:\n",
            "  /toys:\n    get:\n      operationId: listToys\n      responses: { \"200\": { description: ok } }\n  /pets/{id}:\n",
        );
        let (bump, changelog) = size_of(&added, true);
        assert_eq!(bump, Bump::Minor);
        let toys = &changelog.changes[0];
        assert_eq!(
            (toys.operation.as_deref(), toys.id.as_str()),
            (Some("GET /toys"), "endpoint-added")
        );
        let removed = BASE.split("  /pets/{id}:").next().unwrap();
        let (bump, changelog) = size_of(removed, true);
        assert_eq!(bump, Bump::Major);
        assert_eq!(
            changelog.changes[0].operation.as_deref(),
            Some("DELETE /pets/{id}")
        );
        let enum_added = BASE.replace("[available, sold]", "[available, pending, sold]");
        assert_eq!(size_of(&enum_added, true).0, Bump::Minor);
    }

    const SWAGGER: &str = r##"swagger: "2.0"
info: { title: Pets, version: "1" }
host: pets.example.com
paths:
  /pets:
    post:
      operationId: createPet
      consumes: [application/json]
      parameters:
        - { name: pet, in: body, required: true, schema: { $ref: "#/definitions/Pet" } }
      responses: { "204": { description: created } }
definitions:
  Pet: { type: object, required: [name], properties: { name: { type: string } } }
"##;

    const OPENAPI: &str = r##"openapi: 3.0.3
info: { title: Pets, version: "1" }
servers: [{ url: "https://pets.example.com" }]
paths:
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: "#/components/schemas/Pet" } } }
      responses: { "204": { description: created } }
components:
  schemas:
    Pet: { type: object, required: [name], properties: { name: { type: string } } }
"##;

    fn size_between(base: &str, next: &str) -> Bump {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("base.yaml"), base).unwrap();
        std::fs::write(dir.path().join("openapi.yaml"), next).unwrap();
        let base = dir.path().join("base.yaml");
        let comparison = Comparison {
            root: dir.path(),
            spec: "openapi.yaml",
            base: Some(&base),
            relax_enum_additions: true,
        };
        size(&comparison).unwrap().0
    }

    #[test]
    fn swagger_2_specs_are_compared_as_their_openapi_3_conversion() {
        if !crate::format::on_path("oasdiff") {
            eprintln!("oasdiff isn't installed, skipped");
            return;
        }
        assert_eq!(size_between(SWAGGER, OPENAPI), Bump::Patch);
        assert_eq!(size_between(OPENAPI, SWAGGER), Bump::Patch);
        let integer = SWAGGER.replace("name: { type: string }", "name: { type: integer }");
        assert_eq!(size_between(SWAGGER, &integer), Bump::Major);
    }

    const SPLIT_SWAGGER: &str = r##"swagger: "2.0"
info: { title: Pets, version: "1" }
paths:
  /pets:
    post:
      operationId: createPet
      parameters:
        - { name: pet, in: body, required: true, schema: { $ref: "defs.yaml#/Pet" } }
      responses: { "204": { description: created } }
"##;

    #[test]
    fn the_previous_swagger_2_spec_reads_its_files_next_to_the_spec() {
        if !crate::format::on_path("oasdiff") {
            eprintln!("oasdiff isn't installed, skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| crate::pr::git(dir.path(), args).unwrap();
        git(&["init", "--quiet"]);
        std::fs::create_dir(dir.path().join("api")).unwrap();
        std::fs::write(
            dir.path().join("api/defs.yaml"),
            "Pet: { type: object, properties: { name: { type: string } } }\n",
        )
        .unwrap();
        for description in ["created", "the pet was created"] {
            let spec = SPLIT_SWAGGER.replace("created", description);
            std::fs::write(dir.path().join("api/swagger.yaml"), spec).unwrap();
            git(&["add", "."]);
            git(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "m",
            ]);
        }
        let comparison = Comparison {
            root: dir.path(),
            spec: "api/swagger.yaml",
            base: None,
            relax_enum_additions: true,
        };
        assert_eq!(size(&comparison).unwrap().0, Bump::Patch);
    }

    #[test]
    fn swagger_2_specs_that_do_not_convert_are_compared_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let spec = SPLIT_SWAGGER.replace(
            r##"{ name: pet, in: body, required: true, schema: { $ref: "defs.yaml#/Pet" } }"##,
            r##"{ $ref: "missing.yaml#/pet" }"##,
        );
        std::fs::write(dir.path().join("swagger.yaml"), spec).unwrap();
        let converted = converted(dir.path(), "out.json", "swagger.yaml", dir.path(), None);
        assert_eq!(converted.unwrap(), None);
    }

    #[test]
    fn without_a_previous_spec_releases_are_minor() {
        let dir = tempfile::tempdir().unwrap();
        let sized = size(&Comparison {
            root: dir.path(),
            spec: "openapi.yaml",
            base: None,
            relax_enum_additions: true,
        })
        .unwrap();
        assert_eq!(sized, (Bump::Minor, Changelog::default()));
    }
}
