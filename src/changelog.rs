//! The API changes of a spec change and the few release-please entries summing them up: each
//! breaking change, then the endpoints added, removed and updated. Entries are nested commits of
//! the update commit, and of the description for squash merges taking it as the commit message.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Changes kept, to stay under the 65536 characters GitHub allows a description.
const MAX_CHANGES: usize = 100;
const MAX_TEXT: usize = 300;
const MAX_BREAKING: usize = 10;
/// Endpoints named per entry.
const MAX_NAMED: usize = 5;

const BEGIN: &str = "BEGIN_NESTED_COMMIT";
const END: &str = "END_NESTED_COMMIT";
const STATE: &str = "<!-- perseid:api-changes ";

/// One change oasdiff reports.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Change {
    pub breaking: bool,
    /// `METHOD /path`, for changes to an endpoint.
    pub operation: Option<String>,
    /// oasdiff's rule, such as `endpoint-added`.
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Changelog {
    /// Breaking changes first.
    pub changes: Vec<Change>,
    /// Changes left out past [`MAX_CHANGES`].
    pub omitted: usize,
}

/// A changelog entry.
#[derive(Debug, PartialEq)]
struct Entry {
    breaking: bool,
    text: String,
}

impl Changelog {
    /// The changes of `oasdiff changelog --format json`, errors (level 3) breaking.
    pub fn of_oasdiff(json: &[u8]) -> Result<Self> {
        let changes: Vec<Value> =
            serde_json::from_slice(json).context("reading oasdiff's changelog")?;
        let changes = changes.iter().map(|c| Change {
            breaking: c["level"].as_u64() == Some(3),
            operation: match (c["operation"].as_str(), c["path"].as_str()) {
                (Some(operation), Some(path)) => Some(format!("{operation} {path}")),
                _ => None,
            },
            id: c["id"].as_str().unwrap_or_default().to_owned(),
            text: c["text"].as_str().unwrap_or_default().to_owned(),
        });
        Ok(Self::of(changes, 0))
    }

    fn of(changes: impl IntoIterator<Item = Change>, omitted: usize) -> Self {
        let mut changes: Vec<Change> = changes
            .into_iter()
            .map(|mut c| {
                c.text = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if let Some((end, _)) = c.text.char_indices().nth(MAX_TEXT) {
                    c.text.truncate(end);
                    c.text.push('…');
                }
                c
            })
            .collect();
        changes.sort_by(|a, b| b.breaking.cmp(&a.breaking).then(a.cmp(b)));
        changes.dedup();
        let omitted = omitted + changes.len().saturating_sub(MAX_CHANGES);
        changes.truncate(MAX_CHANGES);
        Changelog { changes, omitted }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.omitted == 0
    }

    pub fn breaking(&self) -> bool {
        self.changes.first().is_some_and(|c| c.breaking)
    }

    /// The changes of both.
    pub fn merge(self, other: Self) -> Self {
        Self::of(
            self.changes.into_iter().chain(other.changes),
            self.omitted + other.omitted,
        )
    }

    /// The changes a description written by [`Changelog::section`] lists.
    pub fn of_description(body: &str) -> Self {
        body.split_once(STATE)
            .and_then(|(_, rest)| serde_json::from_str(rest.split(" -->").next()?).ok())
            .unwrap_or_default()
    }

    fn entries(&self) -> Vec<Entry> {
        let breaking: Vec<&Change> = self.changes.iter().filter(|c| c.breaking).collect();
        let mut entries: Vec<Entry> = breaking
            .iter()
            .take(MAX_BREAKING)
            .map(|c| Entry {
                breaking: true,
                text: match &c.operation {
                    Some(operation) => format!("`{operation}`: {}", c.text),
                    None => c.text.clone(),
                },
            })
            .collect();
        if breaking.len() > MAX_BREAKING {
            entries.push(Entry {
                breaking: true,
                text: format!("{} more breaking changes", breaking.len() - MAX_BREAKING),
            });
        }
        let operations = |ids: &[&str]| -> Vec<&str> {
            let mut found: Vec<&str> = self
                .changes
                .iter()
                .filter(|c| !c.breaking && ids.contains(&c.id.as_str()))
                .filter_map(|c| c.operation.as_deref())
                .collect();
            found.dedup();
            found
        };
        let added = operations(&["endpoint-added"]);
        let removed = operations(&[
            "api-path-removed-with-deprecation",
            "api-removed-with-deprecation",
        ]);
        let mut updated: Vec<&str> = vec![];
        let mut others = self.omitted;
        for change in self.changes.iter().filter(|c| !c.breaking) {
            match change.operation.as_deref() {
                Some(o) if added.contains(&o) || removed.contains(&o) => {}
                Some(o) if !updated.contains(&o) => updated.push(o),
                Some(_) => {}
                None => others += 1,
            }
        }
        let mut summary = |verb: &str, operations: &[&str], more: usize| {
            if operations.is_empty() && more == 0 {
                return;
            }
            let shown = operations.len().min(MAX_NAMED);
            let more = more + operations.len() - shown;
            let mut text = operations[..shown]
                .iter()
                .map(|o| format!("`{o}`"))
                .collect::<Vec<_>>()
                .join(", ");
            text = match (text.is_empty(), more) {
                (true, _) => "the API definitions".to_owned(),
                (false, 0) => text,
                (false, more) => format!("{text} and {more} more"),
            };
            entries.push(Entry {
                breaking: false,
                text: format!("{verb} {text}"),
            });
        };
        summary("add", &added, 0);
        summary("remove deprecated", &removed, 0);
        summary("update", &updated, others);
        entries
    }

    /// The nested commits release-please reads as changelog entries.
    pub fn nested(&self) -> String {
        self.entries()
            .iter()
            .map(|e| {
                let kind = if e.breaking {
                    "feat(api)!"
                } else {
                    "feat(api)"
                };
                format!("{BEGIN}\n{kind}: {}\n{END}", e.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The pull request section: the changelog entries, then every change folded.
    pub fn section(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let line = |breaking: bool, text: &str| match breaking {
            true => format!("- ⚠️ {text}\n"),
            false => format!("- {text}\n"),
        };
        let entries: String = self
            .entries()
            .iter()
            .map(|e| line(e.breaking, &e.text))
            .collect();
        let mut changes: String = self
            .changes
            .iter()
            .map(|c| match &c.operation {
                Some(operation) => line(c.breaking, &format!("`{operation}`: {}", c.text)),
                None => line(c.breaking, &c.text),
            })
            .collect();
        if self.omitted > 0 {
            changes += &format!("- and {} more\n", self.omitted);
        }
        let count = self.changes.len() + self.omitted;
        let state = serde_json::to_string(self)
            .unwrap_or_default()
            .replace('>', "\\u003e");
        format!(
            "### Changelog\n\n{entries}\n<details><summary>All {count} API changes</summary>\n\n\
             {changes}\n</details>\n<details><summary>release-please entries</summary>\n\n\
             ```\n{}\n```\n\n</details>\n{STATE}{state} -->",
            self.nested()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(breaking: bool, operation: &str, id: &str, text: &str) -> Change {
        Change {
            breaking,
            operation: Some(operation.to_owned()).filter(|o| !o.is_empty()),
            id: id.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn oasdiff_changes_are_sorted_breaking_first() {
        let json = br#"[
            {"id": "endpoint-added", "text": "endpoint added", "level": 1, "operation": "POST", "path": "/pets"},
            {"id": "api-path-removed-without-deprecation", "text": "path\nremoved", "level": 3, "operation": "DELETE", "path": "/pets/{id}"},
            {"id": "endpoint-added", "text": "endpoint added", "level": 1, "operation": "POST", "path": "/pets"},
            {"id": "api-security-component-added", "text": "scheme added", "level": 1}
        ]"#;
        let changelog = Changelog::of_oasdiff(json).unwrap();
        assert_eq!(
            changelog.changes,
            [
                change(
                    true,
                    "DELETE /pets/{id}",
                    "api-path-removed-without-deprecation",
                    "path removed"
                ),
                change(false, "", "api-security-component-added", "scheme added"),
                change(false, "POST /pets", "endpoint-added", "endpoint added"),
            ]
        );
        assert!(changelog.breaking());
        assert!(Changelog::of_oasdiff(b"[]").unwrap().is_empty());
    }

    #[test]
    fn entries_name_breaking_changes_and_sum_up_the_rest() {
        let mut changes = vec![
            change(
                true,
                "DELETE /pets/{id}",
                "api-removed-without-deprecation",
                "endpoint deleted",
            ),
            change(
                false,
                "GET /old",
                "api-removed-with-deprecation",
                "endpoint deleted",
            ),
            change(false, "", "api-schema-removed", "schema deleted"),
        ];
        for i in 0..7 {
            changes.push(change(
                false,
                &format!("POST /new{i}"),
                "endpoint-added",
                "added",
            ));
            changes.push(change(
                false,
                &format!("GET /pets{i}"),
                "response-optional-property-added",
                "a",
            ));
            changes.push(change(
                false,
                &format!("GET /pets{i}"),
                "new-optional-request-parameter",
                "b",
            ));
        }
        let changelog = Changelog::of(changes, 0);
        let texts: Vec<_> = changelog.entries().into_iter().map(|e| e.text).collect();
        assert_eq!(
            texts,
            [
                "`DELETE /pets/{id}`: endpoint deleted",
                "add `POST /new0`, `POST /new1`, `POST /new2`, `POST /new3`, `POST /new4` and 2 more",
                "remove deprecated `GET /old`",
                "update `GET /pets0`, `GET /pets1`, `GET /pets2`, `GET /pets3`, `GET /pets4` and 3 more",
            ]
        );
        let only_components = Changelog::of([change(false, "", "x", "y")], 0);
        assert_eq!(
            only_components.entries()[0].text,
            "update the API definitions"
        );
    }

    #[test]
    fn breaking_entries_are_capped() {
        let changes =
            (0..MAX_BREAKING + 3).map(|i| change(true, &format!("GET /{i:02}"), "x", "y"));
        let entries = Changelog::of(changes, 0).entries();
        assert_eq!(entries.len(), MAX_BREAKING + 1);
        assert_eq!(entries[MAX_BREAKING].text, "3 more breaking changes");
    }

    #[test]
    fn changes_are_capped() {
        let long = change(false, "", "x", &"é".repeat(MAX_TEXT + 5));
        assert_eq!(
            Changelog::of([long], 0).changes[0].text.chars().count(),
            MAX_TEXT + 1
        );
        let many =
            (0..MAX_CHANGES + 10).map(|i| change(i == 50, &format!("GET /{i:03}"), "x", "y"));
        let changelog = Changelog::of(many, 0);
        assert_eq!(
            (changelog.changes.len(), changelog.omitted),
            (MAX_CHANGES, 10)
        );
        assert!(changelog.breaking());
    }

    #[test]
    fn descriptions_hold_the_changes_read_back() {
        let changelog = Changelog::of(
            [
                change(
                    true,
                    "DELETE /pets",
                    "api-removed-without-deprecation",
                    "gone -->",
                ),
                change(false, "POST /pets", "endpoint-added", "endpoint added"),
            ],
            2,
        );
        let section = changelog.section();
        assert!(
            section.starts_with(
                "### Changelog\n\n- ⚠️ `DELETE /pets`: gone -->\n- add `POST /pets`\n- update the API definitions\n"
            ),
            "{section}"
        );
        assert!(
            section.contains("<summary>All 4 API changes</summary>"),
            "{section}"
        );
        assert!(
            section
                .contains("BEGIN_NESTED_COMMIT\nfeat(api): add `POST /pets`\nEND_NESTED_COMMIT\n"),
            "{section}"
        );
        let body = format!("summary\n\n{section}\n\nGenerated by perseid.");
        assert_eq!(Changelog::of_description(&body), changelog);
        assert!(Changelog::of_description("no changes").is_empty());
        assert_eq!(Changelog::default().section(), "");
        let merged = changelog
            .clone()
            .merge(Changelog::of([change(false, "GET /x", "y", "z")], 0));
        assert_eq!((merged.changes.len(), merged.omitted), (3, 2));
    }
}
