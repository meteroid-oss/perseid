use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Sdk;

pub const SDKS_WORKFLOW: &str = ".github/workflows/sdks.yml";

/// `owner/name` of the GitHub repository the `origin` remote of `root` points to.
pub fn origin_repo(root: &Path) -> Option<String> {
    github_repo(&crate::init::git_remote(root)?)
}

/// The repository of this clone, `what` it is to the command run here.
pub fn required_origin_repo(root: &Path, what: &str) -> Result<String> {
    origin_repo(root).with_context(|| {
        format!("run this in a clone of {what}: its `origin` remote must be on github.com")
    })
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
        .filter(|s| s.local)
        .map(|s| format!("{}/", s.path))
        .collect();
    let mut out = Vec::new();
    if !local.is_empty() {
        out.push(format!("{hub} ({})", local.join(", ")));
    }
    out.extend(sdks.iter().filter_map(|s| s.remote().map(str::to_owned)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn only_github_remotes_name_a_repository() {
        assert_eq!(
            github_repo("https://github.com/acme/api").as_deref(),
            Some("acme/api")
        );
        assert_eq!(github_repo("https://gitlab.com/acme/api"), None);
        assert_eq!(github_repo("https://github.com/acme"), None);
    }
}
