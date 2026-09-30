use std::path::Path;

use anyhow::{Context, Result};
use heck::ToKebabCase;

use super::Ui;
use crate::config::Config;

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

/// The conventional repository suffix of a language's SDK.
pub fn suffix(language: &str) -> &str {
    match language {
        "typescript" => "node",
        "csharp" => "dotnet",
        other => other,
    }
}

/// Offers to move the SDKs perseid.toml keeps in this repository to one repository each.
pub fn plan(config_path: &Path, ui: &Ui, owner: Option<&str>) -> Result<()> {
    let (config, root) = Config::load(config_path)?;
    let local: Vec<_> = config
        .sdks(&[])?
        .into_iter()
        .filter(|s| s.repo.is_none())
        .map(|s| s.language)
        .collect();
    if local.is_empty() {
        return Ok(());
    }
    let spec = required_spec_repo(&root)?;
    let owner = owner.unwrap_or_else(|| spec.split('/').next().unwrap_or_default());
    let name = config.name.to_kebab_case();
    let repos: Vec<_> = local
        .iter()
        .map(|l| (*l, format!("{owner}/{name}-{}", suffix(l))))
        .collect();
    let list = repos
        .iter()
        .map(|(_, r)| r.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let question =
        format!("Give each SDK its own repository ({list}) instead of keeping them in {spec}?");
    if !ui.confirm(&question, false)? {
        return Ok(());
    }
    let mut toml = std::fs::read_to_string(config_path)?;
    let default_module = format!("github.com/{name}/{name}-go");
    for (language, repo) in &repos {
        toml = set_key(&toml, language, "repo", repo);
        let go = config.go.as_ref().and_then(|t| t.module.as_deref());
        if *language == "go" && go == Some(default_module.as_str()) {
            toml = set_key(&toml, "go", "module", &format!("github.com/{repo}"));
        }
        if root.join(language).is_dir() {
            ui.warn(&format!(
                "{language}/ is no longer generated: delete it once {repo} has its first SDK pull request"
            ));
        }
    }
    crate::fsx::write(config_path, toml.as_bytes())?;
    ui.ok(&format!("perseid.toml now points the SDKs to {list}"));
    Ok(())
}

/// Sets `key` of the `[table]` of a TOML document, keeping every other line as written.
pub fn set_key(toml: &str, table: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", toml::Value::String(value.into()));
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let header = format!("[{table}]");
    let Some(start) = lines
        .iter()
        .position(|l| l.trim() == header || l.trim().starts_with(&format!("{header} ")))
    else {
        let separator = if toml.is_empty() || toml.ends_with("\n\n") {
            ""
        } else if toml.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return format!("{toml}{separator}{header}\n{line}\n");
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let existing = (start + 1..end).find(|&i| {
        let text = lines[i].trim_start();
        text.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    match existing {
        Some(i) => lines[i] = line,
        None => lines.insert(start + 1, line),
    }
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
    fn keys_are_set_in_their_table_only() {
        let toml = "name = \"Acme\"\n\n[go]\nmodule = \"github.com/acme/acme-go\" # old\n\n[go.context]\nrepo = 1\n\n[rust]\n";
        let toml = set_key(toml, "go", "module", "github.com/acme-inc/acme-go");
        let toml = set_key(&toml, "go", "repo", "acme-inc/acme-go");
        let toml = set_key(&toml, "rust", "repo", "acme-inc/acme-rust");
        assert_eq!(
            toml,
            "name = \"Acme\"\n\n[go]\nrepo = \"acme-inc/acme-go\"\nmodule = \"github.com/acme-inc/acme-go\"\n\n[go.context]\nrepo = 1\n\n[rust]\nrepo = \"acme-inc/acme-rust\"\n"
        );
        assert_eq!(
            set_key("a = 1\n", "java", "repo", "o/r"),
            "a = 1\n\n[java]\nrepo = \"o/r\"\n"
        );
        assert_eq!(
            set_key("[rust]\nrepository = 1\n", "rust", "repo", "o/r"),
            "[rust]\nrepo = \"o/r\"\nrepository = 1\n"
        );
    }

    #[test]
    fn repositories_are_named_after_the_ecosystem() {
        let names: Vec<_> = crate::config::LANGUAGES.iter().map(|l| suffix(l)).collect();
        assert_eq!(names, ["rust", "node", "python", "go", "java", "dotnet"]);
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
