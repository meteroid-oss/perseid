//! The API changes of a spec change, as release-please changelog entries: each one is a nested
//! conventional commit of the update commit, and of the pull request description for squash
//! merges that take it as the commit message.

use anyhow::{Context, Result};
use serde_json::Value;

/// Most entries kept, under the 65536 characters GitHub allows a description.
const MAX_ENTRIES: usize = 100;
const MAX_TEXT: usize = 300;

const BEGIN: &str = "BEGIN_NESTED_COMMIT";
const END: &str = "END_NESTED_COMMIT";

/// An API change, one entry of the SDK changelogs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// Breaking entries sort first.
    pub breaking: Breaking,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Breaking {
    Yes,
    No,
}

impl Entry {
    fn commit(&self) -> String {
        let kind = match self.breaking {
            Breaking::Yes => "feat(api)!",
            Breaking::No => "feat(api)",
        };
        format!("{kind}: {}", self.text)
    }

    fn of_commit(commit: &str) -> Option<Self> {
        let (kind, text) = commit.trim().split_once(": ")?;
        let breaking = match kind {
            "feat(api)!" => Breaking::Yes,
            "feat(api)" => Breaking::No,
            _ => return None,
        };
        Some(Entry {
            breaking,
            text: text.to_owned(),
        })
    }
}

/// The entries of `oasdiff changelog --format json`, errors (level 3) breaking.
pub fn of_oasdiff(json: &[u8]) -> Result<Vec<Entry>> {
    let changes: Vec<Value> =
        serde_json::from_slice(json).context("reading oasdiff's changelog")?;
    let entries = changes.iter().map(|change| {
        let text = change["text"].as_str().unwrap_or_default();
        let text = match (change["operation"].as_str(), change["path"].as_str()) {
            (Some(operation), Some(path)) => format!("`{operation} {path}`: {text}"),
            _ => text.to_owned(),
        };
        Entry {
            breaking: match change["level"].as_u64() {
                Some(3) => Breaking::Yes,
                _ => Breaking::No,
            },
            text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        }
    });
    Ok(merged(entries))
}

/// `entries` sorted, deduplicated, shortened, at most [`MAX_ENTRIES`].
pub fn merged(entries: impl IntoIterator<Item = Entry>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = entries
        .into_iter()
        .map(|mut e| {
            if let Some((end, _)) = e.text.char_indices().nth(MAX_TEXT) {
                e.text.truncate(end);
                e.text.push('…');
            }
            e
        })
        .filter(|e| !e.text.is_empty())
        .collect();
    entries.sort();
    entries.dedup();
    entries.truncate(MAX_ENTRIES);
    entries
}

/// The entries nested in `message`.
pub fn parse(message: &str) -> Vec<Entry> {
    message
        .split(BEGIN)
        .skip(1)
        .filter_map(|part| Entry::of_commit(part.split(END).next()?))
        .collect()
}

/// The nested commits release-please reads as changelog entries.
pub fn nested(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| format!("{BEGIN}\n{}\n{END}", e.commit()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pull request section listing `entries`, with their nested commits folded.
pub fn section(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let list: String = entries
        .iter()
        .map(|e| match e.breaking {
            Breaking::Yes => format!("- ⚠️ {}\n", e.text),
            Breaking::No => format!("- {}\n", e.text),
        })
        .collect();
    format!(
        "### API changes\n\n{list}\n<details><summary>Changelog entries</summary>\n\n```\n{}\n```\n\n</details>",
        nested(entries)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oasdiff_changes_become_entries_breaking_first() {
        let json = br#"[
            {"text": "endpoint added", "level": 1, "operation": "POST", "path": "/pets"},
            {"text": "api path removed\nwithout deprecation", "level": 3, "operation": "DELETE", "path": "/pets/{id}"},
            {"text": "endpoint added", "level": 1, "operation": "POST", "path": "/pets"},
            {"text": "security scheme added", "level": 1}
        ]"#;
        let entries = of_oasdiff(json).unwrap();
        let texts: Vec<_> = entries.iter().map(Entry::commit).collect();
        assert_eq!(
            texts,
            [
                "feat(api)!: `DELETE /pets/{id}`: api path removed without deprecation",
                "feat(api): `POST /pets`: endpoint added",
                "feat(api): security scheme added",
            ]
        );
        assert!(of_oasdiff(b"[]").unwrap().is_empty());
    }

    #[test]
    fn descriptions_hold_entries_read_back() {
        let entries =
            of_oasdiff(br#"[{"text": "a", "level": 3}, {"text": "b: c", "level": 1}]"#).unwrap();
        let listed = section(&entries);
        assert!(
            listed.starts_with("### API changes\n\n- ⚠️ a\n- b: c\n"),
            "{listed}"
        );
        assert!(
            listed.contains("BEGIN_NESTED_COMMIT\nfeat(api)!: a\nEND_NESTED_COMMIT\n"),
            "{listed}"
        );
        let body = format!("summary\n\n{listed}\n\nGenerated by perseid.");
        assert_eq!(parse(&body), entries);
        assert!(parse("no entries").is_empty());
        assert_eq!(section(&[]), "");
    }

    #[test]
    fn entries_are_capped() {
        let long = Entry {
            breaking: Breaking::No,
            text: "é".repeat(MAX_TEXT + 5),
        };
        assert_eq!(merged([long])[0].text.chars().count(), MAX_TEXT + 1);
        let many = (0..MAX_ENTRIES + 10).map(|i| Entry {
            breaking: if i == MAX_ENTRIES + 5 {
                Breaking::Yes
            } else {
                Breaking::No
            },
            text: format!("change {i:03}"),
        });
        let kept = merged(many);
        assert_eq!(kept.len(), MAX_ENTRIES);
        assert_eq!(kept[0].breaking, Breaking::Yes);
    }
}
