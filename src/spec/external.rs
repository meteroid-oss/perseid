//! Bundles `$ref`s to other documents into the main one: each referenced schema is hoisted into
//! `components.schemas` under a name that collides with none, and the reference points to it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

use super::{is_url, load, upgrade};

/// Hoists every schema the document references from another file or URL, resolved from `location`.
pub(super) fn bundle(doc: &mut Value, location: &str, root: &Path) -> Result<()> {
    if !has_external(doc) {
        return Ok(());
    }
    let taken = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .map(|schemas| schemas.keys().cloned().collect())
        .unwrap_or_default();
    let mut bundler = Bundler {
        root,
        docs: BTreeMap::new(),
        names: BTreeMap::new(),
        taken,
        schemas: Map::new(),
    };
    bundler.rewrite(doc, &normalize(location), true)?;
    if bundler.schemas.is_empty() {
        return Ok(());
    }
    let Some(root) = doc.as_object_mut() else {
        return Ok(());
    };
    let components = root
        .entry("components")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(schemas) = components
        .as_object_mut()
        .map(|c| {
            c.entry("schemas")
                .or_insert_with(|| Value::Object(Map::new()))
        })
        .and_then(Value::as_object_mut)
    {
        schemas.extend(bundler.schemas);
    }
    Ok(())
}

fn skipped(key: &str) -> bool {
    key.starts_with("x-") || key.starts_with("example")
}

fn has_external(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            if key == "$ref" {
                child.as_str().is_some_and(|r| !r.starts_with('#'))
            } else {
                !skipped(key) && has_external(child)
            }
        }),
        Value::Array(items) => items.iter().any(has_external),
        _ => false,
    }
}

struct Bundler<'a> {
    root: &'a Path,
    /// Loaded documents by location.
    docs: BTreeMap<String, Value>,
    /// Hoisted component name by (location, fragment).
    names: BTreeMap<(String, String), String>,
    taken: BTreeSet<String>,
    schemas: Map<String, Value>,
}

impl Bundler<'_> {
    /// Points the references in `value`, which lives in the document at `current`, at components
    /// of the main document. Local references of the main document stay as they are.
    fn rewrite(&mut self, value: &mut Value, current: &str, main: bool) -> Result<()> {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref")
                    && !(main && reference.starts_with('#'))
                {
                    let name = self.hoist(&reference.clone(), current)?;
                    map.insert(
                        "$ref".into(),
                        Value::String(format!("#/components/schemas/{name}")),
                    );
                }
                for (key, child) in map.iter_mut() {
                    if key != "$ref" && !skipped(key) {
                        self.rewrite(child, current, main)?;
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.rewrite(item, current, main)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Adds what `reference` points to, with what it references in turn, to the hoisted schemas.
    fn hoist(&mut self, reference: &str, current: &str) -> Result<String> {
        let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
        let location = if file.is_empty() {
            current.to_owned()
        } else {
            join(current, file)
        };
        let key = (location.clone(), fragment.to_owned());
        if let Some(name) = self.names.get(&key) {
            return Ok(name.clone());
        }
        if fragment.starts_with("/components/") && !fragment.starts_with("/components/schemas/") {
            bail!(
                "external reference `{reference}`: only schemas can be read from other files, \
                 bundle the spec first (for example with `redocly bundle`)"
            );
        }
        if !self.docs.contains_key(&location) {
            let mut other = load(&location, self.root).with_context(|| {
                format!(
                    "resolving external reference `{reference}` (to avoid it, bundle the spec \
                     first, for example with `redocly bundle`)"
                )
            })?;
            if other.get("openapi").is_some() || other.get("swagger").is_some() {
                upgrade::to_3_1(&mut other).with_context(|| location.clone())?;
            }
            self.docs.insert(location.clone(), other);
        }
        let mut target = self.docs[&location]
            .pointer(fragment)
            .cloned()
            .with_context(|| format!("unresolved external reference `{reference}`"))?;
        let name = self.name(&location, fragment);
        self.names.insert(key, name.clone());
        self.rewrite(&mut target, &location, false)?;
        self.schemas.insert(name.clone(), target);
        Ok(name)
    }

    fn name(&mut self, location: &str, fragment: &str) -> String {
        let stem = Path::new(location.split(['?', '#']).next().unwrap_or(location))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let last = fragment
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.replace("~1", "/").replace("~0", "~"))
            .unwrap_or_else(|| stem.clone());
        let clean = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect()
        };
        let base = match clean(&last) {
            name if name.is_empty() => "External".to_owned(),
            name => name,
        };
        let mut name = base.clone();
        if self.taken.contains(&name) {
            let prefixed = clean(&stem);
            name = format!("{prefixed}_{base}");
            let mut n = 2;
            while self.taken.contains(&name) {
                name = format!("{prefixed}_{base}{n}");
                n += 1;
            }
        }
        self.taken.insert(name.clone());
        name
    }
}

/// `..` and `.` segments resolved, for locations used as keys.
fn normalize(location: &str) -> String {
    if is_url(location) {
        return location.to_owned();
    }
    let mut parts: Vec<Component> = Vec::new();
    for part in Path::new(location).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts
        .iter()
        .collect::<std::path::PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// `file` as seen from the document at `base`: a path next to it, or a URL on the same host.
fn join(base: &str, file: &str) -> String {
    if is_url(file) {
        return file.to_owned();
    }
    if let Some((scheme, rest)) = base.split_once("://") {
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let mut segments: Vec<&str> = path.split('/').collect();
        let relative = match file.strip_prefix('/') {
            Some(absolute) => {
                segments.clear();
                absolute
            }
            None => {
                segments.pop();
                file
            }
        };
        for part in relative.split('/') {
            match part {
                "." => {}
                ".." => {
                    segments.pop();
                }
                part => segments.push(part),
            }
        }
        return format!("{scheme}://{host}/{}", segments.join("/"));
    }
    if Path::new(file).is_absolute() {
        return file.to_owned();
    }
    let dir = Path::new(base).parent().unwrap_or(Path::new(""));
    normalize(&dir.join(file).to_string_lossy())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn locations_resolve_next_to_their_document() {
        assert_eq!(join("api/spec.yaml", "./a/b.yaml"), "api/a/b.yaml");
        assert_eq!(join("api/spec.yaml", "../c.yaml"), "c.yaml");
        assert_eq!(join("spec.yaml", "c.yaml"), "c.yaml");
        assert_eq!(
            join("https://h.example/v1/spec.json", "../m/c.json"),
            "https://h.example/m/c.json"
        );
        assert_eq!(
            join("https://h.example/v1/spec.json", "/m/c.json"),
            "https://h.example/m/c.json"
        );
    }

    #[test]
    fn referenced_schemas_are_hoisted_with_their_own_references() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("sub/other.json"),
            json!({ "components": { "schemas": {
                "Ext": { "type": "object", "properties": {
                    "inner": { "$ref": "#/components/schemas/Inner" },
                    "back": { "$ref": "../common.json#/Shared" }
                } },
                "Inner": { "type": "string" }
            } } })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("common.json"),
            json!({ "Shared": { "type": "integer" } }).to_string(),
        )
        .unwrap();
        let mut doc = json!({
            "paths": { "/x": { "get": { "responses": { "200": { "content": {
                "application/json": { "schema": {
                    "$ref": "sub/other.json#/components/schemas/Ext" } } } } } } } },
            "components": { "schemas": { "Inner": { "type": "boolean" } } }
        });
        bundle(&mut doc, "spec.json", dir.path()).unwrap();
        let schema =
            &doc["paths"]["/x"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
        assert_eq!(schema["$ref"], "#/components/schemas/Ext");
        let schemas = &doc["components"]["schemas"];
        assert_eq!(schemas["Inner"], json!({ "type": "boolean" }));
        assert_eq!(
            schemas["Ext"]["properties"]["inner"]["$ref"],
            "#/components/schemas/other_Inner"
        );
        assert_eq!(schemas["other_Inner"], json!({ "type": "string" }));
        assert_eq!(
            schemas["Ext"]["properties"]["back"]["$ref"],
            "#/components/schemas/Shared"
        );
        assert_eq!(schemas["Shared"], json!({ "type": "integer" }));
    }

    #[test]
    fn missing_files_name_the_reference_and_suggest_bundling() {
        let dir = tempfile::tempdir().unwrap();
        let mut doc = json!({ "paths": { "/x": { "get": { "responses": { "200": { "content": {
            "application/json": { "schema": { "$ref": "nope.yaml#/X" } } } } } } } } });
        let error = format!(
            "{:#}",
            bundle(&mut doc, "spec.json", dir.path()).unwrap_err()
        );
        assert!(
            error.contains("nope.yaml#/X") && error.contains("redocly bundle"),
            "{error}"
        );
    }
}
