//! The dependencies of the runtime that the manifest of an SDK lacks, added on each generation.
//!
//! A manifest is scaffolded once and is the SDK's own afterwards: perseid appends what a new SDK's
//! manifest declares and this one lacks, at the version a new SDK starts from, and changes no line.

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::{
    config::{Config, Sdk},
    fsx, scaffold,
};

const MANIFESTS: [&str; 8] = [
    "Cargo.toml",
    "go.mod",
    "go.sum",
    "package.json",
    "pyproject.toml",
    "build.gradle",
    "build.gradle.kts",
    "pom.xml",
];

/// Whether the scaffolded file `name` declares dependencies.
pub fn is_manifest(name: &str) -> bool {
    MANIFESTS.contains(&name) || name.ends_with(".csproj")
}

/// The new text of a manifest, and the dependencies it gained.
type Edit = Option<(String, Vec<String>)>;

/// Adds to the manifest of the SDK at `dir` the dependencies of the runtime it lacks. Returns the
/// files changed, relative to `dir`.
pub fn complete(config: &Config, sdk: &Sdk, dir: &Path) -> Result<Vec<PathBuf>> {
    let scaffolded = scaffold::manifests(config, sdk, dir)?;
    let wanted = |path: &str| {
        scaffolded
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.as_str())
    };
    let read = |path: &str| std::fs::read_to_string(dir.join(path)).ok();
    let mut edits: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut edit = |path: &str, edit: Edit| {
        if let Some((text, added)) = edit {
            edits.push((path.to_owned(), text, added));
        }
    };
    match sdk.language {
        "rust" => {
            if let (Some(current), Some(wanted)) = (read("Cargo.toml"), wanted("Cargo.toml")) {
                edit(
                    "Cargo.toml",
                    read_or_warn("Cargo.toml", cargo(&current, wanted)),
                );
            }
        }
        "go" => {
            if let (Some(current), Some(module)) = (read("go.mod"), wanted("go.mod")) {
                let sum = read("go.sum").unwrap_or_default();
                let (module, sum) = go(&current, &sum, module, wanted("go.sum").unwrap_or(""));
                edit("go.mod", module);
                edit("go.sum", sum.map(|s| (s, vec![])));
            }
        }
        "typescript" => {
            if let (Some(current), Some(wanted)) = (read("package.json"), wanted("package.json")) {
                let edited = package_json(&current, wanted);
                edit("package.json", read_or_warn("package.json", edited));
            }
        }
        "python" => {
            if let (Some(current), Some(wanted)) =
                (read("pyproject.toml"), wanted("pyproject.toml"))
            {
                let edited = pyproject(&current, wanted);
                edit("pyproject.toml", read_or_warn("pyproject.toml", edited));
            }
        }
        "java" => {
            let catalog = read("gradle/libs.versions.toml").unwrap_or_default();
            let wanted = wanted("build.gradle").unwrap_or("");
            if let Some(current) = read("build.gradle") {
                edit("build.gradle", gradle(&current, wanted, &catalog, false));
            } else if let Some(current) = read("build.gradle.kts") {
                edit("build.gradle.kts", gradle(&current, wanted, &catalog, true));
            } else if let Some(current) = read("pom.xml") {
                edit("pom.xml", pom(&current, wanted));
            }
        }
        _ => {
            for (path, wanted) in scaffolded.iter().filter(|(p, _)| p.ends_with(".csproj")) {
                let tests = path.split('/').any(|p| p.ends_with(".Tests"));
                if let (false, Some(current)) = (tests, read(path)) {
                    let props = ["Directory.Packages.props", "../Directory.Packages.props"];
                    let central = props
                        .iter()
                        .any(|p| dir.join(path).with_file_name(p).exists())
                        || current.contains("ManagePackageVersionsCentrally");
                    edit(path, csproj(&current, wanted, central, path));
                }
            }
        }
    }
    let mut changed = Vec::new();
    for (path, text, added) in edits {
        fsx::write(&dir.join(&path), text.as_bytes())?;
        if !added.is_empty() {
            eprintln!("{}: added {} to {path}", sdk.language, added.join(", "));
        }
        changed.push(PathBuf::from(path));
    }
    Ok(changed)
}

fn read_or_warn(path: &str, edit: Result<Edit>) -> Edit {
    edit.unwrap_or_else(|e| {
        eprintln!("warning: {path} is left as is, as perseid cannot read it: {e:#}");
        None
    })
}

fn warn(path: &str, missing: &[String], why: &str) {
    eprintln!(
        "warning: {path} lacks {}, which the SDK needs: {why}, add them by hand",
        missing.join(", ")
    );
}

/// `[dependencies]` and `[features]` of Cargo.toml, but the `default` feature: the features
/// referencing a dependency that is not optional are left out.
fn cargo(current: &str, scaffold: &str) -> Result<Edit> {
    let mut doc: DocumentMut = current.parse()?;
    let wanted: DocumentMut = scaffold.parse()?;
    let mut added = Vec::new();
    if let Some(wanted) = wanted.get("dependencies").and_then(Item::as_table_like) {
        for (name, item) in wanted.iter() {
            if !cargo_tables(&doc).any(|t| declares(t, name))
                && let Some(table) = doc
                    .entry("dependencies")
                    .or_insert_with(toml_edit::table)
                    .as_table_like_mut()
            {
                table.insert(name, undecorated(item));
                added.push(name.to_owned());
            }
        }
    }
    let wanted = wanted.get("features").and_then(Item::as_table_like);
    let mut missing: Vec<(&str, &Item)> = wanted
        .into_iter()
        .flat_map(|t| t.iter())
        .filter(|(name, _)| *name != "default")
        .filter(|(name, _)| !features(&doc).is_some_and(|f| f.contains_key(name)))
        .collect();
    // A feature can enable another one added after it.
    while let Some(at) = missing.iter().position(|(_, item)| {
        item.as_array()
            .is_some_and(|a| a.iter().filter_map(Value::as_str).all(|v| enables(&doc, v)))
    }) {
        let (name, item) = missing.remove(at);
        if let Some(table) = doc
            .entry("features")
            .or_insert_with(toml_edit::table)
            .as_table_like_mut()
        {
            table.insert(name, undecorated(item));
            added.push(format!("the `{name}` feature"));
        }
    }
    Ok((!added.is_empty()).then(|| (doc.to_string(), added)))
}

fn undecorated(item: &Item) -> Item {
    let mut item = item.clone();
    if let Some(value) = item.as_value_mut() {
        value.decor_mut().clear();
    }
    item
}

/// The dependency tables of a Cargo.toml, of every target too.
fn cargo_tables(doc: &DocumentMut) -> impl Iterator<Item = &dyn TableLike> {
    let targets = doc
        .get("target")
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|t| t.iter())
        .filter_map(|(_, target)| target.get("dependencies"));
    doc.get("dependencies")
        .into_iter()
        .chain(targets)
        .filter_map(Item::as_table_like)
}

/// Whether `table` declares the crate `name`, under its name or renamed.
fn declares(table: &dyn TableLike, name: &str) -> bool {
    table.contains_key(name)
        || table.iter().any(|(_, item)| {
            item.as_table_like()
                .and_then(|t| t.get("package"))
                .and_then(Item::as_str)
                == Some(name)
        })
}

fn features(doc: &DocumentMut) -> Option<&dyn TableLike> {
    doc.get("features").and_then(Item::as_table_like)
}

/// Whether Cargo accepts `value` in a feature of `doc`.
fn enables(doc: &DocumentMut, value: &str) -> bool {
    let dependency = |name: &str| cargo_tables(doc).find_map(|t| t.get(name));
    let optional = |name: &str| {
        dependency(name)
            .and_then(Item::as_table_like)
            .and_then(|t| t.get("optional"))
            .and_then(Item::as_bool)
            == Some(true)
    };
    let feature = |name: &str| features(doc).is_some_and(|f| f.contains_key(name));
    match value.split_once('/') {
        Some((name, _)) => match name.strip_suffix('?') {
            Some(name) => optional(name),
            None => dependency(name).is_some(),
        },
        None => match value.strip_prefix("dep:") {
            Some(name) => optional(name),
            None => feature(value) || optional(value),
        },
    }
}

/// `require` directives of go.mod, with the lines of go.sum for the modules they add.
fn go(module: &str, sum: &str, wanted: &str, wanted_sum: &str) -> (Edit, Option<String>) {
    let missing: Vec<(String, String)> = requirements(wanted)
        .into_iter()
        .filter(|(path, _)| !requirements(module).iter().any(|(p, _)| p == path))
        .collect();
    if missing.is_empty() {
        return (None, None);
    }
    let mut module = with_newline(module);
    for (path, version) in &missing {
        let blank = if module.ends_with("\n\n") { "" } else { "\n" };
        module = format!("{module}{blank}require {path} {version}\n");
    }
    let lines: Vec<&str> = wanted_sum
        .lines()
        .filter(|l| {
            let path = l.split_whitespace().next();
            missing.iter().any(|(p, _)| Some(p.as_str()) == path)
        })
        .filter(|l| !sum.lines().any(|s| s == *l))
        .collect();
    let sum = (!lines.is_empty()).then(|| format!("{}{}\n", with_newline(sum), lines.join("\n")));
    let added = missing.into_iter().map(|(path, _)| path).collect();
    (Some((module, added)), sum)
}

fn with_newline(text: &str) -> String {
    match text.is_empty() || text.ends_with('\n') {
        true => text.to_owned(),
        false => format!("{text}\n"),
    }
}

/// The modules go.mod requires, with their versions.
fn requirements(module: &str) -> Vec<(String, String)> {
    let mut block = false;
    let mut required = Vec::new();
    for line in module.lines() {
        let line = line.split("//").next().unwrap_or_default();
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            [")"] => block = false,
            ["require", "("] => block = true,
            ["require", path, version, ..] => required.push((path.into(), version.into())),
            [path, version, ..] if block => required.push((path.into(), version.into())),
            _ => {}
        }
    }
    required
}

/// `dependencies` of package.json.
fn package_json(current: &str, scaffold: &str) -> Result<Edit> {
    let wanted: serde_json::Value = serde_json::from_str(scaffold)?;
    let have: serde_json::Value = serde_json::from_str(current)?;
    let declared = |name: &str| {
        ["dependencies", "peerDependencies"]
            .iter()
            .any(|field| have[field].get(name).is_some())
    };
    let missing: Vec<(&String, &serde_json::Value)> = wanted["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !declared(name))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    let top = object(current, 0).context("reading the top-level object")?;
    let unit = top
        .members
        .first()
        .map_or("  ", |m| indentation(current, m.at));
    let entries: Vec<String> = missing
        .iter()
        .map(|(name, version)| format!("{}: {version}", serde_json::Value::from(name.as_str())))
        .collect();
    let text = match top.members.iter().find(|m| m.key == "dependencies") {
        Some(member) => {
            let dependencies = object(current, member.value.start)
                .filter(|o| o.close < member.value.end)
                .context("`dependencies` is not an object")?;
            append(current, &dependencies, &entries, unit)
        }
        None => {
            let outer = indentation(current, top.open);
            let inner = format!("{outer}{unit}{unit}");
            let entries = entries.join(&format!(",\n{inner}"));
            let member = format!("\"dependencies\": {{\n{inner}{entries}\n{outer}{unit}}}");
            append(current, &top, &[member], unit)
        }
    };
    let added = missing.into_iter().map(|(name, _)| name.clone()).collect();
    Ok(Some((text, added)))
}

/// A JSON object of a text: where its braces are, and its members.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

struct Member {
    key: String,
    /// Where the key starts.
    at: usize,
    value: Range<usize>,
}

/// The first JSON object of `text` from `from`.
fn object(text: &str, from: usize) -> Option<Object> {
    let bytes = text.as_bytes();
    let open = from + text[from..].find('{')?;
    let mut members = Vec::new();
    let mut at = skip(bytes, open + 1);
    loop {
        match bytes.get(at)? {
            b'}' => {
                return Some(Object {
                    open,
                    close: at,
                    members,
                });
            }
            b',' => at = skip(bytes, at + 1),
            b'"' => {
                let end = value_end(bytes, at)?;
                let key = serde_json::from_str(&text[at..end]).ok()?;
                let colon = skip(bytes, end);
                (bytes.get(colon)? == &b':').then_some(())?;
                let start = skip(bytes, colon + 1);
                let end = value_end(bytes, start)?;
                members.push(Member {
                    key,
                    at,
                    value: start..end,
                });
                at = skip(bytes, end);
            }
            _ => return None,
        }
    }
}

fn skip(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// Where the JSON value starting at `start` ends.
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    let (mut depth, mut string, mut escaped) = (0, false, false);
    for (at, &c) in bytes.iter().enumerate().skip(start) {
        if string {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' if depth == 0 => return Some(at + 1),
                b'"' => string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' if depth == 0 => return Some(at),
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            b',' if depth == 0 => return Some(at),
            c if depth == 0 && c.is_ascii_whitespace() => return Some(at),
            _ => {}
        }
    }
    None
}

/// `text` with `entries` appended to the members of `object`, laid out as they are.
fn append(text: &str, object: &Object, entries: &[String], unit: &str) -> String {
    let (at, inserted) = match object.members.last() {
        Some(last) if !text[object.open..last.at].contains('\n') => {
            (last.value.end, format!(", {}", entries.join(", ")))
        }
        Some(last) => {
            let indent = indentation(text, last.at);
            let lines: String = entries.iter().map(|e| format!(",\n{indent}{e}")).collect();
            (last.value.end, lines)
        }
        None => {
            let outer = indentation(text, object.open);
            let entries = entries.join(&format!(",\n{outer}{unit}"));
            let inside = format!("\n{outer}{unit}{entries}\n{outer}");
            let text = [&text[..object.open + 1], &inside, &text[object.close..]].concat();
            return text;
        }
    };
    [&text[..at], &inserted, &text[at..]].concat()
}

/// The leading whitespace of the line holding `at`.
fn indentation(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let line = &text[start..];
    &line[..line.len() - line.trim_start().len()]
}

/// `project.dependencies` of pyproject.toml.
fn pyproject(current: &str, scaffold: &str) -> Result<Edit> {
    let wanted: DocumentMut = scaffold.parse()?;
    let Some(wanted) = wanted
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(Item::as_array)
    else {
        return Ok(None);
    };
    let mut doc: DocumentMut = current.parse()?;
    let declared: Vec<String> = doc
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(Item::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(distribution)
        .collect();
    let missing: Vec<&str> = wanted
        .iter()
        .filter_map(Value::as_str)
        .filter(|r| !declared.contains(&distribution(r)))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    let names: Vec<String> = missing.iter().map(|r| distribution(r)).collect();
    let dynamic = doc
        .get("project")
        .and_then(|p| p.get("dynamic"))
        .and_then(Item::as_array)
        .is_some_and(|d| d.iter().any(|v| v.as_str() == Some("dependencies")));
    let Some(project) = doc
        .get_mut("project")
        .and_then(Item::as_table_like_mut)
        .filter(|_| !dynamic)
    else {
        warn(
            "pyproject.toml",
            &names,
            "its dependencies are not in [project]",
        );
        return Ok(None);
    };
    let Some(dependencies) = project
        .entry("dependencies")
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
    else {
        return Ok(None);
    };
    let prefix = dependencies
        .iter()
        .last()
        .and_then(|v| v.decor().prefix())
        .and_then(|p| p.as_str())
        .map(str::to_owned);
    for requirement in missing {
        let mut value = Value::from(requirement);
        if let Some(prefix) = &prefix {
            value.decor_mut().set_prefix(prefix.as_str());
        }
        dependencies.push_formatted(value);
    }
    Ok(Some((doc.to_string(), names)))
}

/// The normalized name of the distribution a requirement names.
fn distribution(requirement: &str) -> String {
    let name: String = requirement
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    name.to_lowercase()
        .split(['-', '_', '.'])
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Configurations whose dependencies the SDK is compiled or run with.
const CONFIGURATIONS: [&str; 4] = ["api", "implementation", "compileOnly", "runtimeOnly"];

/// The configuration and the `group:artifact:version` of each dependency of a build.gradle.
fn coordinates(build: &str) -> Vec<(&str, &str)> {
    build
        .lines()
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .filter(|(configuration, _)| CONFIGURATIONS.contains(configuration))
        .map(|(configuration, c)| (configuration, c.trim().trim_matches(['\'', '"'])))
        .filter(|(_, c)| c.split(':').count() == 3)
        .collect()
}

/// `group:artifact` of `group:artifact:version`.
fn module(coordinate: &str) -> &str {
    coordinate.rsplit_once(':').map_or(coordinate, |(m, _)| m)
}

/// Dependencies of build.gradle or build.gradle.kts: those of the version catalog count.
fn gradle(current: &str, scaffold: &str, catalog: &str, kotlin: bool) -> Edit {
    let declared = |coordinate: &str| {
        let module = module(coordinate);
        let (group, name) = module.split_once(':').unwrap_or_default();
        let named = |l: &str| {
            l.contains(&format!("group = \"{group}\"")) && l.contains(&format!("name = \"{name}\""))
        };
        [current, catalog].iter().any(|text| {
            [":", "'", "\""]
                .iter()
                .any(|end| text.contains(&format!("{module}{end}")))
        }) || catalog.lines().any(named)
    };
    let missing: Vec<(&str, &str)> = coordinates(scaffold)
        .into_iter()
        .filter(|(_, c)| !declared(c))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let lines: Vec<String> = missing
        .iter()
        .map(|(configuration, c)| match kotlin {
            true => format!("{configuration}(\"{c}\")"),
            false => format!("{configuration} '{c}'"),
        })
        .collect();
    let text = match top_level_block(current, "dependencies") {
        Some((open, close)) => {
            let indent = current[open + 1..close]
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map_or("    ", |l| &l[..l.len() - l.trim_start().len()]);
            let indent = if indent.is_empty() { "    " } else { indent };
            let lines: String = lines.iter().map(|l| format!("{indent}{l}\n")).collect();
            insert_before(current, close, &lines)
        }
        None => {
            let lines: String = lines.iter().map(|l| format!("    {l}\n")).collect();
            format!("{}\ndependencies {{\n{lines}}}\n", with_newline(current))
        }
    };
    let added = missing.iter().map(|(_, c)| module(c).to_owned()).collect();
    Some((text, added))
}

/// The braces of the block `name { ... }` at the top level of a Gradle build script.
fn top_level_block(script: &str, name: &str) -> Option<(usize, usize)> {
    let bytes = script.as_bytes();
    let (mut depth, mut open, mut at) = (0usize, None, 0);
    while at < bytes.len() {
        let rest = &script[at..];
        match bytes[at] {
            _ if rest.starts_with("//") => {
                at += rest.find('\n').unwrap_or(rest.len());
                continue;
            }
            _ if rest.starts_with("/*") => {
                at += rest.find("*/").map_or(rest.len(), |n| n + 2);
                continue;
            }
            _ if rest.starts_with("\"\"\"") => {
                at += rest[3..].find("\"\"\"").map_or(rest.len(), |n| n + 6);
                continue;
            }
            quote @ (b'"' | b'\'') => {
                let mut end = at + 1;
                while end < bytes.len() && bytes[end] != quote {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                at = end + 1;
                continue;
            }
            b'{' => {
                let before = script[..at].trim_end();
                let named = before.strip_suffix(name).is_some_and(|b| {
                    !b.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
                });
                if depth == 0 && named && open.is_none() {
                    open = Some(at);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(open) = open
                {
                    return Some((open, at));
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// `text` with `lines`, each ending with a newline, on their own lines before the one of `at`.
fn insert_before(text: &str, at: usize, lines: &str) -> String {
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    match text[start..at].trim().is_empty() {
        true => [&text[..start], lines, &text[start..]].concat(),
        false => [&text[..at], "\n", lines, &text[at..]].concat(),
    }
}

/// The dependencies of the project of a pom.xml, from those of the scaffolded build.gradle.
fn pom(current: &str, scaffold: &str) -> Edit {
    let nested: Vec<Range<usize>> = ["dependencyManagement", "build", "profiles", "reporting"]
        .iter()
        .flat_map(|tag| elements(current, tag))
        .collect();
    let outside = |range: &Range<usize>| !nested.iter().any(|n| n.contains(&range.start));
    let declared = |coordinate: &str| {
        let (group, artifact) = module(coordinate).split_once(':').unwrap_or_default();
        elements(current, "dependency")
            .into_iter()
            .filter(outside)
            .any(|d| {
                let d = &current[d];
                d.contains(&format!("<groupId>{group}</groupId>"))
                    && d.contains(&format!("<artifactId>{artifact}</artifactId>"))
            })
    };
    let missing: Vec<(&str, &str)> = coordinates(scaffold)
        .into_iter()
        .filter(|(_, c)| !declared(c))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let dependencies = elements(current, "dependencies").into_iter().find(outside);
    let unit = current
        .find("<modelVersion>")
        .map_or("    ", |at| indentation(current, at));
    let unit = if unit.is_empty() { "    " } else { unit };
    let entries = |indent: &str| -> String {
        missing
            .iter()
            .map(|(configuration, c)| {
                let mut parts = c.splitn(3, ':');
                let (group, artifact, version) = (
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default(),
                );
                let scope = match *configuration {
                    "compileOnly" => format!("{indent}{unit}<scope>provided</scope>\n"),
                    "runtimeOnly" => format!("{indent}{unit}<scope>runtime</scope>\n"),
                    _ => String::new(),
                };
                format!(
                    "{indent}<dependency>\n\
                     {indent}{unit}<groupId>{group}</groupId>\n\
                     {indent}{unit}<artifactId>{artifact}</artifactId>\n\
                     {indent}{unit}<version>{version}</version>\n\
                     {scope}{indent}</dependency>\n"
                )
            })
            .collect()
    };
    let text = match dependencies {
        Some(range) => {
            let close = range.end - "</dependencies>".len();
            let outer = indentation(current, close);
            let inner = current[range.clone()]
                .find("<dependency>")
                .map(|at| indentation(current, range.start + at).to_owned())
                .unwrap_or_else(|| format!("{outer}{unit}"));
            insert_before(current, close, &entries(&inner))
        }
        None => {
            let close = current.rfind("</project>").unwrap_or(current.len());
            let block = format!(
                "{unit}<dependencies>\n{}{unit}</dependencies>\n",
                entries(&format!("{unit}{unit}"))
            );
            insert_before(current, close, &block)
        }
    };
    let added = missing.iter().map(|(_, c)| module(c).to_owned()).collect();
    Some((text, added))
}

/// Where each `<tag>...</tag>` of `xml` is, outermost first.
fn elements(xml: &str, tag: &str) -> Vec<Range<usize>> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = xml[from..].find(&open).map(|n| from + n) {
        let Some(end) = xml[start..].find(&close).map(|n| start + n + close.len()) else {
            break;
        };
        found.push(start..end);
        from = end;
    }
    found
}

/// A `PackageReference` of a csproj, with the condition of its `ItemGroup`.
struct Reference<'a> {
    name: &'a str,
    version: &'a str,
    condition: Option<&'a str>,
}

/// `PackageReference`s of a csproj, each added in a new `ItemGroup` keeping its condition.
fn csproj(current: &str, scaffold: &str, central: bool, path: &str) -> Edit {
    let declared = |name: &str| {
        tags(current, "PackageReference").any(|tag| {
            ["Include", "Update"]
                .iter()
                .any(|a| attribute(tag, a).is_some_and(|n| n.eq_ignore_ascii_case(name)))
        })
    };
    let missing: Vec<Reference> = references(scaffold)
        .into_iter()
        .filter(|r| !declared(r.name))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let names: Vec<String> = missing.iter().map(|r| r.name.to_owned()).collect();
    if central {
        warn(path, &names, "its package versions are managed centrally");
        return None;
    }
    let close = current.rfind("</Project>")?;
    let unit = ["<PropertyGroup", "<ItemGroup"]
        .iter()
        .filter_map(|tag| current.find(tag))
        .min()
        .map_or("  ", |at| indentation(current, at));
    let unit = if unit.is_empty() { "  " } else { unit };
    let mut groups: Vec<(Option<&str>, String)> = Vec::new();
    for reference in &missing {
        let line = format!(
            "{unit}{unit}<PackageReference Include=\"{}\" Version=\"{}\" />\n",
            reference.name, reference.version
        );
        match groups.iter_mut().find(|(c, _)| *c == reference.condition) {
            Some((_, lines)) => lines.push_str(&line),
            None => groups.push((reference.condition, line)),
        }
    }
    let groups: String = groups
        .into_iter()
        .map(|(condition, lines)| {
            let condition = condition.map_or(String::new(), |c| format!(" Condition=\"{c}\""));
            format!("\n{unit}<ItemGroup{condition}>\n{lines}{unit}</ItemGroup>\n")
        })
        .collect();
    Some((insert_before(current, close, &groups), names))
}

fn references(csproj: &str) -> Vec<Reference<'_>> {
    let mut references = Vec::new();
    for (start, _) in csproj.match_indices("<ItemGroup") {
        let Some(group) = csproj[start..]
            .find("</ItemGroup>")
            .map(|n| &csproj[start..start + n])
        else {
            continue;
        };
        let condition = tags(group, "ItemGroup")
            .next()
            .and_then(|t| attribute(t, "Condition"));
        for tag in tags(group, "PackageReference") {
            if let (Some(name), Some(version)) =
                (attribute(tag, "Include"), attribute(tag, "Version"))
            {
                references.push(Reference {
                    name,
                    version,
                    condition,
                });
            }
        }
    }
    references
}

/// The opening tags `<name ...>` of `xml`.
fn tags<'a>(xml: &'a str, name: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{name}");
    let tags: Vec<&str> = xml
        .match_indices(&open)
        .filter(|(at, _)| {
            xml[at + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')
        })
        .filter_map(|(at, _)| xml[at..].find('>').map(|end| &xml[at..=at + end]))
        .collect();
    tags.into_iter()
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("{name}=\"");
    tag.match_indices(&pattern)
        .find(|(at, _)| tag[..*at].ends_with(char::is_whitespace))
        .and_then(|(at, _)| {
            let value = &tag[at + pattern.len()..];
            value.find('"').map(|end| &value[..end])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold(path: &str) -> String {
        let (language, path) = path.split_once('/').unwrap();
        let (_, text) = crate::assets::under(&format!("scaffold/{language}"))
            .find(|(p, _)| *p == path)
            .unwrap();
        String::from_utf8_lossy(text).replace("@@AUTHORS_JSON@@", "[]")
    }

    #[test]
    fn cargo_gains_the_crates_and_features_it_lacks() {
        let current = "[package]\nname = \"acme\"\n\n[dependencies]\n# Mine\nserde = \"1.0.100\"\nmy-uuid = { package = \"uuid\", version = \"1\" }\nhyper = \"1\"\nhyper-util = \"0.1\"\nhttp-body-util = \"0.1\"\nbytes = \"1\"\nfutures-core = \"0.3\"\nhttp = \"1\"\nhyper-rustls = { version = \"0.27\", optional = true }\nrustls = { version = \"0.23\", optional = true }\nserde_json = \"1.0\"\nrust_decimal = \"1\"\nchrono = \"0.4\"\nurl = \"2\"\npercent-encoding = \"2\"\nhmac = { version = \"0.12\", optional = true }\nsha2 = { version = \"0.10\", optional = true }\nbase64 = { version = \"0.22\", optional = true }\ntokio = \"1\"\n\n[dev-dependencies]\ntokio = \"1\"\n\n[features]\ndefault = [\"rustls-tls\"]\nrustls-tls = [\"dep:hyper-rustls\", \"dep:rustls\"]\nwebhooks = [\"dep:hmac\", \"dep:sha2\", \"dep:base64\"]\n";
        let (text, added) = cargo(current, &scaffold("rust/Cargo.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(
            added,
            [
                "hyper-tls",
                "tracing",
                "the `http1` feature",
                "the `http2` feature",
                "the `native-tls` feature",
                "the `tracing` feature",
            ]
        );
        assert!(text.starts_with(&current[..current.find("[dev-dependencies]").unwrap() - 1]));
        assert!(text.contains("tokio = \"1\"\nhyper-tls = { version = \"0.6\", optional = true }\ntracing = { version = \"0.1\", optional = true }\n\n[dev-dependencies]"));
        assert!(text.ends_with("tracing = [\"dep:tracing\"]\n"));
        assert!(!text.contains("default = [\"http1\""));
        assert_eq!(cargo(&text, &scaffold("rust/Cargo.toml")).unwrap(), None);
    }

    #[test]
    fn cargo_leaves_out_features_of_crates_that_are_not_optional() {
        let current = "[dependencies]\nhyper-tls = \"0.6\"\n";
        let wanted = "[dependencies]\nhyper-tls = { version = \"0.6\", optional = true }\n\n[features]\nnative-tls = [\"dep:hyper-tls\"]\n";
        assert_eq!(cargo(current, wanted).unwrap(), None);
    }

    #[test]
    fn go_mod_requires_the_modules_it_lacks_with_their_sums() {
        let wanted = "module x\n\ngo 1.23\n\nrequire github.com/google/uuid v1.6.0\n";
        let wanted_sum = "github.com/google/uuid v1.6.0 h1:a=\ngithub.com/google/uuid v1.6.0/go.mod h1:b=\nother v1 h1:c=\n";
        let current = "module github.com/acme/go\n\ngo 1.22\n";
        let (module, sum) = go(current, "", wanted, wanted_sum);
        let (module, added) = module.unwrap();
        assert_eq!(
            module,
            "module github.com/acme/go\n\ngo 1.22\n\nrequire github.com/google/uuid v1.6.0\n"
        );
        assert_eq!(added, ["github.com/google/uuid"]);
        assert_eq!(
            sum.unwrap(),
            "github.com/google/uuid v1.6.0 h1:a=\ngithub.com/google/uuid v1.6.0/go.mod h1:b=\n"
        );
        let block = "module m\n\nrequire (\n\tgithub.com/google/uuid v1.3.0 // indirect\n)\n";
        assert_eq!(go(block, "", wanted, wanted_sum), (None, None));
        let (module, _) = go("module m\n\n", "", wanted, wanted_sum);
        assert_eq!(
            module.unwrap().0,
            "module m\n\nrequire github.com/google/uuid v1.6.0\n"
        );
    }

    #[test]
    fn package_json_gains_dependencies_laid_out_as_the_file() {
        let wanted = r#"{ "dependencies": { "a": "^1.0.0", "b": "^2.0.0" } }"#;
        let absent = "{\n  \"name\": \"x\",\n  \"scripts\": { \"build\": \"tsc\" }\n}\n";
        let (text, added) = package_json(absent, wanted).unwrap().unwrap();
        assert_eq!(added, ["a", "b"]);
        assert_eq!(
            text,
            "{\n  \"name\": \"x\",\n  \"scripts\": { \"build\": \"tsc\" },\n  \"dependencies\": {\n    \"a\": \"^1.0.0\",\n    \"b\": \"^2.0.0\"\n  }\n}\n"
        );
        let some = "{\n  \"dependencies\": {\n    \"a\": \"^1.2.0\"\n  },\n  \"x\": [1, {\"}\": \"\\\"\"}]\n}";
        let (text, _) = package_json(some, wanted).unwrap().unwrap();
        assert_eq!(
            text,
            "{\n  \"dependencies\": {\n    \"a\": \"^1.2.0\",\n    \"b\": \"^2.0.0\"\n  },\n  \"x\": [1, {\"}\": \"\\\"\"}]\n}"
        );
        let empty = "{\n  \"dependencies\": {}\n}";
        let (text, _) = package_json(empty, wanted).unwrap().unwrap();
        assert_eq!(
            text,
            "{\n  \"dependencies\": {\n    \"a\": \"^1.0.0\",\n    \"b\": \"^2.0.0\"\n  }\n}"
        );
        let inline = "{ \"dependencies\": { \"c\": \"1\" } }";
        let (text, _) = package_json(inline, wanted).unwrap().unwrap();
        assert_eq!(
            text,
            "{ \"dependencies\": { \"c\": \"1\", \"a\": \"^1.0.0\", \"b\": \"^2.0.0\" } }"
        );
        assert_eq!(package_json(&text, wanted).unwrap(), None);
        assert_eq!(
            package_json(absent, &scaffold("typescript/package.json")).unwrap(),
            None
        );
    }

    #[test]
    fn pyproject_gains_the_distributions_it_lacks() {
        let wanted = "[project]\ndependencies = [\"httpx>=0.27,<1\", \"typing_extensions>=4\"]\n";
        let current = "[project]\nname = \"acme\"\ndependencies = [\n    \"Typing.Extensions\",\n    \"attrs\",\n]\n\n[tool.x]\n";
        let (text, added) = pyproject(current, wanted).unwrap().unwrap();
        assert_eq!(added, ["httpx"]);
        assert_eq!(
            text,
            "[project]\nname = \"acme\"\ndependencies = [\n    \"Typing.Extensions\",\n    \"attrs\",\n    \"httpx>=0.27,<1\",\n]\n\n[tool.x]\n"
        );
        let none = "[project]\nname = \"acme\"\n";
        let (text, _) = pyproject(none, wanted).unwrap().unwrap();
        assert_eq!(
            text,
            "[project]\nname = \"acme\"\ndependencies = [\"httpx>=0.27,<1\", \"typing_extensions>=4\"]\n"
        );
        let poetry = "[tool.poetry.dependencies]\nhttpx = \"*\"\n";
        assert_eq!(pyproject(poetry, wanted).unwrap(), None);
    }

    #[test]
    fn gradle_gains_the_modules_it_lacks_in_its_top_level_block() {
        let wanted = scaffold("java/build.gradle");
        let current = "buildscript {\n    dependencies { classpath 'x:y:1' }\n}\n// dependencies {\ndependencies {\n    api \"com.fasterxml.jackson.core:jackson-databind:2.17.0\"\n    api libs.okhttp\n    implementation 'com.fasterxml.jackson.datatype:jackson-datatype-jdk8:2.17.0'\n    implementation 'com.fasterxml.jackson.datatype:jackson-datatype-jsr310:2.17.0'\n}\n";
        let catalog = "[libraries]\nokhttp = { module = \"com.squareup.okhttp3:okhttp\", version = \"4.12.0\" }\n";
        let (text, added) = gradle(current, &wanted, catalog, false).unwrap();
        assert_eq!(added, ["com.google.code.findbugs:jsr305"]);
        assert_eq!(
            text,
            current.replace(
                "2.17.0'\n}",
                "2.17.0'\n    compileOnly 'com.google.code.findbugs:jsr305:3.0.2'\n}"
            )
        );
        let kotlin = "plugins { `java-library` }\n";
        let (text, _) = gradle(kotlin, &wanted, "", true).unwrap();
        assert!(text.ends_with("\ndependencies {\n    api(\"com.fasterxml.jackson.core:jackson-databind:2.18.2\")\n    api(\"com.squareup.okhttp3:okhttp:4.12.0\")\n    implementation(\"com.fasterxml.jackson.datatype:jackson-datatype-jdk8:2.18.2\")\n    implementation(\"com.fasterxml.jackson.datatype:jackson-datatype-jsr310:2.18.2\")\n    compileOnly(\"com.google.code.findbugs:jsr305:3.0.2\")\n}\n"));
        assert!(!text.contains("junit"));
    }

    #[test]
    fn pom_gains_the_modules_it_lacks_in_the_project_dependencies() {
        let wanted = "dependencies {\n    api 'g:a:1'\n    compileOnly 'g:b:2'\n}\n";
        let current = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <dependencyManagement>\n    <dependencies>\n      <dependency><groupId>g</groupId><artifactId>b</artifactId></dependency>\n    </dependencies>\n  </dependencyManagement>\n  <dependencies>\n    <dependency>\n      <groupId>g</groupId>\n      <artifactId>a</artifactId>\n    </dependency>\n  </dependencies>\n</project>\n";
        let (text, added) = pom(current, wanted).unwrap();
        assert_eq!(added, ["g:b"]);
        assert_eq!(text, current.replace("    </dependency>\n  </dependencies>\n</project>", "    </dependency>\n    <dependency>\n      <groupId>g</groupId>\n      <artifactId>b</artifactId>\n      <version>2</version>\n      <scope>provided</scope>\n    </dependency>\n  </dependencies>\n</project>"));
        let bare = "<project>\n  <modelVersion>4.0.0</modelVersion>\n</project>\n";
        let (text, _) = pom(bare, wanted).unwrap();
        assert!(text.contains("  <dependencies>\n    <dependency>\n      <groupId>g</groupId>"));
        assert!(text.ends_with("    </dependency>\n  </dependencies>\n</project>\n"));
    }

    #[test]
    fn csproj_gains_the_packages_it_lacks_in_new_item_groups() {
        let wanted = scaffold("csharp/@@PACKAGE_NAME@@/@@PACKAGE_NAME@@.csproj");
        let current = "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net8.0</TargetFramework>\n  </PropertyGroup>\n  <ItemGroup>\n    <PackageReference Include=\"system.text.json\" Version=\"8.0.0\" />\n  </ItemGroup>\n</Project>\n";
        let (text, added) = csproj(current, &wanted, false, "Acme/Acme.csproj").unwrap();
        assert_eq!(added, ["Microsoft.Extensions.Http"]);
        assert_eq!(text, current.replace("</ItemGroup>\n</Project>", "</ItemGroup>\n\n  <ItemGroup Condition=\"Exists('@@CLIENT_NAME@@ServiceCollectionExtensions.cs')\">\n    <PackageReference Include=\"Microsoft.Extensions.Http\" Version=\"8.0.1\" />\n  </ItemGroup>\n</Project>"));
        assert_eq!(csproj(&text, &wanted, false, "Acme/Acme.csproj"), None);
        assert_eq!(csproj(current, &wanted, true, "Acme/Acme.csproj"), None);
    }

    #[test]
    fn complete_writes_the_manifest_of_an_existing_sdk() {
        let config: Config = toml::from_str("name = \"Acme\"\nsdks = [\"go\"]\n").unwrap();
        let sdk = config.sdks(&[]).unwrap().remove(0);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module github.com/acme/go\n\ngo 1.23\n",
        )
        .unwrap();
        let changed = complete(&config, &sdk, dir.path()).unwrap();
        assert_eq!(changed, [PathBuf::from("go.mod"), PathBuf::from("go.sum")]);
        let module = std::fs::read_to_string(dir.path().join("go.mod")).unwrap();
        assert!(module.ends_with("\nrequire github.com/google/uuid v1.6.0\n"));
        assert!(complete(&config, &sdk, dir.path()).unwrap().is_empty());
    }
}
