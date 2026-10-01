//! `--bump auto`: the release size of a spec change, from oasdiff comparing the spec with its
//! previous version.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

use crate::{config::Source, pr::Bump};

/// Largest changelog appended to a pull request description, which GitHub caps at 65536.
const CHANGELOG_LIMIT: usize = 50_000;

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

/// The bump of the change from the base spec to the current one, and its changelog in markdown.
/// Without a base spec or oasdiff, a minor release.
pub fn size(c: &Comparison) -> Result<(Bump, Option<String>)> {
    let scratch = tempfile::tempdir()?;
    let current: OsString = match Source::parse(c.spec) {
        Source::File(file) => c.root.join(file).into(),
        Source::Url(url) => url.into(),
    };
    let previous = match (c.base, Source::parse(c.spec)) {
        (None, Source::File(file)) => previous(c.root, file),
        _ => None,
    };
    let base: PathBuf = match (c.base, previous) {
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
            return Ok((Bump::Minor, None));
        }
    };
    if !crate::format::on_path("oasdiff") {
        annotate(
            "warning",
            "oasdiff isn't installed (`perseid tools install` installs it), asking for a minor release",
        );
        return Ok((Bump::Minor, None));
    }
    let mut levels: Vec<OsString> = vec![];
    if c.relax_enum_additions {
        let path = scratch.path().join("oasdiff-levels");
        std::fs::write(&path, "response-property-enum-value-added info\n")?;
        levels = vec!["--severity-levels".into(), path.into()];
    }
    let oasdiff = |args: &[&str]| -> Result<std::process::Output> {
        Command::new("oasdiff")
            .args(args)
            .args(&levels)
            .arg(&base)
            .arg(&current)
            .output()
            .context("running oasdiff")
    };
    let finds = |args: &[&str]| -> Result<bool> {
        let output = oasdiff(args)?;
        if !matches!(output.status.code(), Some(0 | 1)) {
            annotate(
                "warning",
                &format!(
                    "`oasdiff {}` failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            );
        }
        Ok(!output.status.success())
    };
    let bump = if finds(&["breaking", "--fail-on", "ERR"])? {
        Bump::Major
    } else if finds(&["changelog", "--fail-on", "INFO"])? {
        Bump::Minor
    } else {
        Bump::Patch
    };
    if bump == Bump::Patch {
        return Ok((bump, None));
    }
    let markdown = oasdiff(&["changelog", "-f", "markdown"])?;
    let changelog = String::from_utf8_lossy(&markdown.stdout);
    let notes = markdown.status.success().then(|| {
        let mut notes = format!("### API changes\n\n{}", changelog.trim());
        if notes.len() > CHANGELOG_LIMIT {
            let mut end = CHANGELOG_LIMIT;
            while !notes.is_char_boundary(end) {
                end -= 1;
            }
            notes.truncate(end);
        }
        notes
    });
    Ok((bump, notes))
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

    fn size_of(next: &str, relax: bool) -> (Bump, Option<String>) {
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
        assert_eq!(size_of(BASE, true), (Bump::Patch, None));
        let described = BASE.replace("description: deleted", "description: gone for good");
        assert_eq!(size_of(&described, true).0, Bump::Patch);
        let added = BASE.replace(
            "  /pets/{id}:\n",
            "  /toys:\n    get:\n      operationId: listToys\n      responses: { \"200\": { description: ok } }\n  /pets/{id}:\n",
        );
        let (bump, notes) = size_of(&added, true);
        assert_eq!(bump, Bump::Minor);
        let notes = notes.unwrap();
        assert!(notes.starts_with("### API changes\n\n"), "{notes}");
        assert!(notes.contains("/toys"), "{notes}");
        let removed = BASE.split("  /pets/{id}:").next().unwrap();
        assert_eq!(size_of(removed, true).0, Bump::Major);
        let enum_added = BASE.replace("[available, sold]", "[available, pending, sold]");
        assert_eq!(size_of(&enum_added, true).0, Bump::Minor);
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
        assert_eq!(sized, (Bump::Minor, None));
    }
}
