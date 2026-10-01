use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Sdk;

pub const SDKS_WORKFLOW: &str = ".github/workflows/sdks.yml";

/// `owner/name` of the GitHub repository the `origin` remote of `root` points to.
pub fn spec_repo(root: &Path) -> Option<String> {
    github_repo(&crate::init::git_remote(root)?)
}

pub fn required_spec_repo(root: &Path) -> Result<String> {
    spec_repo(root).context(
        "the GitHub setup runs in a clone of the spec repository: its `origin` remote must be on github.com",
    )
}

fn github_repo(url: &str) -> Option<String> {
    let path = url.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}

/// `acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-node`, without the spec leg when the
/// SDKs are generated where the spec lives.
pub fn diagram(source: Option<&str>, hub: &str, targets: &[String]) -> String {
    let targets = match targets.is_empty() {
        true => "…".to_owned(),
        false => targets.join(", "),
    };
    match source {
        Some(source) => format!("{source} ──spec──▶ {hub} ──PRs──▶ {targets}"),
        None => format!("{hub} ──PRs──▶ {targets}"),
    }
}

/// Where the pull requests of `sdks` land: their repositories, or `hub (typescript/, …)`.
pub fn targets(sdks: &[Sdk], hub: &str) -> Vec<String> {
    let local: Vec<String> = sdks
        .iter()
        .filter(|s| s.repo.is_none())
        .map(|s| format!("{}/", s.path))
        .collect();
    let mut out = Vec::new();
    if !local.is_empty() {
        out.push(format!("{hub} ({})", local.join(", ")));
    }
    out.extend(sdks.iter().filter_map(|s| s.repo.clone()));
    out
}

/// Sets a top-level `key` of a TOML document, keeping every other line as written.
pub fn set_top(toml: &str, key: &str, value: &str) -> String {
    set_in(toml, "", key, value)
}

/// Sets `key` of `table` (the top level when empty), adding the table before the others if
/// missing, and keeping every other line as written.
pub fn set_in(toml: &str, table: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", toml::Value::String(value.into()));
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let header = |l: &String| l.trim_start().starts_with('[');
    let first_table = lines.iter().position(header).unwrap_or(lines.len());
    let start = match table {
        "" => 0,
        table => match lines.iter().position(|l| l.trim() == format!("[{table}]")) {
            Some(at) => at + 1,
            None => {
                let new = [format!("[{table}]"), line];
                match first_table == lines.len() {
                    true => lines.extend([String::new()].into_iter().chain(new)),
                    false => {
                        let new = new.into_iter().chain([String::new()]);
                        lines.splice(first_table..first_table, new);
                    }
                }
                return joined(toml, &lines);
            }
        },
    };
    let end = lines[start..]
        .iter()
        .position(header)
        .map_or(lines.len(), |i| start + i);
    let existing = lines[start..end].iter().position(|l| {
        l.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    match existing {
        Some(i) => lines[start + i] = line,
        None => {
            let last = lines[start..end]
                .iter()
                .rposition(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
            lines.insert(last.map_or(start, |i| start + i + 1), line);
        }
    }
    joined(toml, &lines)
}

fn joined(toml: &str, lines: &[String]) -> String {
    let mut out = lines.join("\n");
    if toml.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_keys_are_set_before_the_tables() {
        let toml = "spec = \"api/openapi.yaml\"\nname = \"Acme\"\n\n[go]\nspec = 1\n";
        assert_eq!(
            set_top(toml, "spec", "openapi.yaml"),
            "spec = \"openapi.yaml\"\nname = \"Acme\"\n\n[go]\nspec = 1\n"
        );
        assert_eq!(
            set_top("name = \"A\"\n", "repository", "r"),
            "name = \"A\"\nrepository = \"r\"\n"
        );
    }

    #[test]
    fn table_keys_are_set_in_their_table_created_if_missing() {
        let toml = "name = \"A\"\n\n[package]\nlicense = \"MIT\"\nrepository = \"a\"\n\n[go]\n";
        assert_eq!(
            set_in(toml, "package", "repository", "b"),
            toml.replace("\"a\"", "\"b\"")
        );
        assert_eq!(
            set_in("name = \"A\"\n\n[go]\n", "package", "repository", "b"),
            "name = \"A\"\n\n[package]\nrepository = \"b\"\n\n[go]\n"
        );
        assert_eq!(
            set_in("name = \"A\"\n", "package", "license", "MIT"),
            "name = \"A\"\n\n[package]\nlicense = \"MIT\"\n"
        );
        assert_eq!(
            set_in(
                "[package]\nlicense = \"MIT\"\n[go]\n",
                "package",
                "homepage",
                "h"
            ),
            "[package]\nlicense = \"MIT\"\nhomepage = \"h\"\n[go]\n"
        );
    }

    #[test]
    fn diagrams_show_where_the_spec_and_pull_requests_go() {
        let targets = ["acme/api-node".to_owned(), "acme/api-go".to_owned()];
        assert_eq!(
            diagram(Some("acme/api"), "acme/api-sdks", &targets),
            "acme/api ──spec──▶ acme/api-sdks ──PRs──▶ acme/api-node, acme/api-go"
        );
        assert_eq!(diagram(None, "acme/api", &[]), "acme/api ──PRs──▶ …");
    }

    #[test]
    fn only_github_remotes_name_a_spec_repository() {
        assert_eq!(
            github_repo("https://github.com/acme/api").as_deref(),
            Some("acme/api")
        );
        assert_eq!(github_repo("https://gitlab.com/acme/api"), None);
        assert_eq!(github_repo("https://github.com/acme"), None);
    }
}
