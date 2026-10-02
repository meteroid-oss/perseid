//! Bundles `$ref`s to other documents into the main one: each referenced schema is hoisted into
//! `components.schemas` under a name that collides with none, and the reference points to it.
//! Other referenced objects (path items, responses, parameters, ...) are inlined where they are
//! referenced.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

use super::{is_url, load, upgrade};

/// Hoists every schema the document references from another file or URL, resolved from `location`.
/// `from_3_0` tells that the main document was OpenAPI 3.0, so that the schemas of plain files it
/// references are upgraded as its own were.
pub(super) fn bundle(doc: &mut Value, location: &str, root: &Path, from_3_0: bool) -> Result<()> {
    if !has_external(doc, false) {
        return Ok(());
    }
    let taken = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .map(|schemas| schemas.keys().cloned().collect())
        .unwrap_or_default();
    let main = normalize(location);
    let mut bundler = Bundler {
        root,
        main: main.clone(),
        from_3_0,
        docs: BTreeMap::new(),
        names: BTreeMap::new(),
        taken,
        schemas: Map::new(),
        inlining: Vec::new(),
    };
    // A component that is only a reference to another file (a split layout) takes the referenced
    // schema under its own name, which other references to the same schema reuse.
    let mut aliases = Vec::new();
    for (name, schema) in doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(Value::String(reference)) = schema.get("$ref") else {
            continue;
        };
        if schema.as_object().is_some_and(|s| s.len() > 1) || reference.starts_with('#') {
            continue;
        }
        let key = bundler.locate(reference, &main);
        if key.0 != main && !bundler.names.contains_key(&key) {
            bundler.names.insert(key.clone(), name.clone());
            aliases.push((name.clone(), key));
        }
    }
    for (name, (location, fragment)) in aliases {
        let mut target = bundler.target(&location, &fragment, true)?;
        bundler.rewrite(&mut target, &location, false, true)?;
        doc["components"]["schemas"][&name] = target;
    }
    bundler.rewrite(doc, &main, true, false)?;
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

/// Keys whose values are examples, defaults or extensions rather than parts of the API, so a
/// `$ref` in them is not followed. Keys of a map of names (`names`) are never skipped: a property
/// can be called `example`.
fn skipped(key: &str, names: bool) -> bool {
    !names && (key.starts_with("x-") || upgrade::LITERALS.contains(&key))
}

/// Whether the children of `key` are named entries (properties, responses, media types, ...).
fn names_below(key: &str, names: bool) -> bool {
    !names && upgrade::NAME_MAPS.contains(&key)
}

fn has_external(value: &Value, names: bool) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            if key == "$ref" && !names {
                child.as_str().is_some_and(|r| !r.starts_with('#'))
            } else {
                !skipped(key, names) && has_external(child, names_below(key, names))
            }
        }),
        Value::Array(items) => items.iter().any(|i| has_external(i, false)),
        _ => false,
    }
}

struct Bundler<'a> {
    root: &'a Path,
    /// Location of the main document.
    main: String,
    from_3_0: bool,
    /// Loaded documents by location, and whether each is an OpenAPI document (already upgraded).
    docs: BTreeMap<String, (Value, bool)>,
    /// Hoisted component name by (location, fragment).
    names: BTreeMap<(String, String), String>,
    taken: BTreeSet<String>,
    schemas: Map<String, Value>,
    /// The (location, fragment) being inlined, to reject an object that contains itself.
    inlining: Vec<(String, String)>,
}

impl Bundler<'_> {
    /// Points the references in `value`, which lives in the document at `current`, at components
    /// of the main document. Local references of the main document stay as they are. `schema`
    /// tells whether `value` is a schema: a reference elsewhere (to a path item, a response, ...)
    /// is replaced by the object it points to.
    fn rewrite(
        &mut self,
        value: &mut Value,
        current: &str,
        main: bool,
        schema: bool,
    ) -> Result<()> {
        self.walk(value, current, main, schema, false)
    }

    fn walk(
        &mut self,
        value: &mut Value,
        current: &str,
        main: bool,
        schema: bool,
        names: bool,
    ) -> Result<()> {
        match value {
            Value::Object(map) => {
                if !names
                    && let Some(Value::String(reference)) = map.get("$ref")
                    && !(main && reference.starts_with('#'))
                {
                    let reference = reference.clone();
                    if schema {
                        let target = self.hoist(&reference, current)?;
                        map.insert("$ref".into(), Value::String(target));
                    } else {
                        match self.inline(&reference, current)? {
                            Ok(Value::Object(mut target)) => {
                                // Siblings of the reference (a `summary`, a `description`) win.
                                map.remove("$ref");
                                target.extend(std::mem::take(map));
                                *map = target;
                                return Ok(());
                            }
                            Ok(target) => {
                                *value = target;
                                return Ok(());
                            }
                            Err(local) => {
                                map.insert("$ref".into(), Value::String(local));
                            }
                        }
                    }
                }
                for (key, child) in map.iter_mut() {
                    if names {
                        self.walk(child, current, main, schema, false)?;
                    } else if key != "$ref" && !skipped(key, false) {
                        let schema = schema || key == "schema" || key == "schemas";
                        self.walk(child, current, main, schema, names_below(key, false))?;
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, current, main, schema, false)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The (location, fragment) `reference`, seen from the document at `current`, points to.
    fn locate(&self, reference: &str, current: &str) -> (String, String) {
        let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
        let location = if file.is_empty() {
            current.to_owned()
        } else {
            join(current, file)
        };
        (location, fragment.to_owned())
    }

    /// What `fragment` of the document at `location` holds, upgraded from 3.0 when the main
    /// document was 3.0 and the document is a plain file of schemas or objects.
    fn target(&mut self, location: &str, fragment: &str, schema: bool) -> Result<Value> {
        let reference = format!("{location}#{fragment}");
        if !self.docs.contains_key(location) {
            let mut other = load(location, self.root).with_context(|| {
                format!(
                    "resolving external reference `{reference}` (to avoid it, bundle the spec \
                     first, for example with `redocly bundle`)"
                )
            })?;
            let openapi = other.get("openapi").is_some() || other.get("swagger").is_some();
            if openapi {
                upgrade::to_3_1(&mut other).with_context(|| location.to_owned())?;
            }
            self.docs.insert(location.to_owned(), (other, openapi));
        }
        let (doc, openapi) = &self.docs[location];
        let mut target = doc
            .pointer(fragment)
            .cloned()
            .with_context(|| format!("unresolved external reference `{reference}`"))?;
        if self.from_3_0 && !openapi {
            if schema {
                upgrade::schema(&mut target);
            } else {
                upgrade::walk(&mut target);
            }
        }
        Ok(target)
    }

    /// The object `reference` points to, with its own references rewritten, or the local
    /// reference to use when it points into the main document.
    fn inline(&mut self, reference: &str, current: &str) -> Result<Result<Value, String>> {
        let (location, fragment) = self.locate(reference, current);
        if location == self.main {
            return Ok(Err(format!("#{fragment}")));
        }
        let key = (location.clone(), fragment.clone());
        if self.inlining.contains(&key) {
            bail!("external reference `{reference}` contains itself");
        }
        let mut target = self.target(&location, &fragment, false)?;
        self.inlining.push(key);
        self.rewrite(&mut target, &location, false, false)?;
        self.inlining.pop();
        Ok(Ok(target))
    }

    /// Adds the schema `reference` points to, with what it references in turn, to the hoisted
    /// schemas, and returns the local reference to it.
    fn hoist(&mut self, reference: &str, current: &str) -> Result<String> {
        let (location, fragment) = self.locate(reference, current);
        if location == self.main {
            return Ok(format!("#{fragment}"));
        }
        let key = (location.clone(), fragment.clone());
        if let Some(name) = self.names.get(&key) {
            return Ok(format!("#/components/schemas/{name}"));
        }
        if fragment.starts_with("/components/") && !fragment.starts_with("/components/schemas/") {
            bail!(
                "external reference `{reference}`: only schemas can be read from other files, \
                 bundle the spec first (for example with `redocly bundle`)"
            );
        }
        let mut target = self.target(&location, &fragment, true)?;
        let name = self.name(&location, &fragment);
        self.names.insert(key, name.clone());
        self.rewrite(&mut target, &location, false, true)?;
        self.schemas.insert(name.clone(), target);
        Ok(format!("#/components/schemas/{name}"))
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
        bundle(&mut doc, "spec.json", dir.path(), false).unwrap();
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

    fn write(dir: &Path, path: &str, value: &Value) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    #[test]
    fn referenced_path_items_and_responses_are_inlined() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "paths/pets.json",
            &json!({ "get": { "responses": {
                "200": { "description": "ok", "content": { "application/json": {
                    "schema": { "$ref": "../schemas/Pet.json" } } } },
                "404": { "$ref": "../responses/NotFound.json" }
            } } }),
        );
        write(
            dir.path(),
            "responses/NotFound.json",
            &json!({ "description": "not found", "content": { "application/json": {
                "schema": { "$ref": "#/Body" } } }, "Body": { "type": "string" } }),
        );
        write(dir.path(), "schemas/Pet.json", &json!({ "type": "object" }));
        let mut doc = json!({ "paths": {
            "/pets": { "$ref": "paths/pets.json", "summary": "Pets" }
        } });
        bundle(&mut doc, "spec.json", dir.path(), false).unwrap();
        let item = &doc["paths"]["/pets"];
        assert_eq!(item["summary"], "Pets");
        assert!(item.get("$ref").is_none(), "{item}");
        let responses = &item["get"]["responses"];
        assert_eq!(
            responses["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Pet"
        );
        assert_eq!(responses["404"]["description"], "not found");
        assert_eq!(
            responses["404"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Body"
        );
        assert_eq!(
            doc["components"]["schemas"]["Body"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn plain_schema_files_of_a_3_0_spec_are_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Nul.json",
            &json!({ "type": "string", "nullable": true }),
        );
        let mut doc = json!({ "components": { "schemas": { "Thing": { "properties": {
            "n": { "$ref": "Nul.json" } } } } } });
        bundle(&mut doc, "spec.json", dir.path(), true).unwrap();
        assert_eq!(
            doc["components"]["schemas"]["Nul"],
            json!({ "type": ["string", "null"] })
        );
    }

    #[test]
    fn properties_named_like_skipped_keywords_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Ex.json", &json!({ "type": "string" }));
        let mut doc = json!({ "components": { "schemas": { "Thing": { "properties": {
            "exampleValue": { "$ref": "Ex.json" },
            "example": { "$ref": "Ex.json" }
        }, "example": { "$ref": "nope.json" } } } } });
        bundle(&mut doc, "spec.json", dir.path(), false).unwrap();
        let properties = &doc["components"]["schemas"]["Thing"]["properties"];
        assert_eq!(
            properties["exampleValue"]["$ref"],
            "#/components/schemas/Ex"
        );
        assert_eq!(properties["example"]["$ref"], "#/components/schemas/Ex");
    }

    #[test]
    fn split_components_keep_their_names_and_back_references_reach_the_main_document() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "schemas/Pet.json",
            &json!({ "type": "object", "properties": {
                "lastError": { "$ref": "../openapi.json#/components/schemas/Error" } } }),
        );
        let mut doc = json!({
            "paths": { "/pets": { "get": { "responses": { "200": { "description": "ok",
                "content": { "application/json": { "schema": {
                    "$ref": "schemas/Pet.json" } } } } } } } },
            "components": { "schemas": {
                "Pet": { "$ref": "./schemas/Pet.json" },
                "Error": { "type": "object" }
            } }
        });
        bundle(&mut doc, "openapi.json", dir.path(), false).unwrap();
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        let mut names = schemas.keys().collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["Error", "Pet"], "{schemas:?}");
        assert_eq!(
            schemas["Pet"]["properties"]["lastError"]["$ref"],
            "#/components/schemas/Error"
        );
        assert_eq!(
            doc["paths"]["/pets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                ["$ref"],
            "#/components/schemas/Pet"
        );
    }

    #[test]
    fn missing_files_name_the_reference_and_suggest_bundling() {
        let dir = tempfile::tempdir().unwrap();
        let mut doc = json!({ "paths": { "/x": { "get": { "responses": { "200": { "content": {
            "application/json": { "schema": { "$ref": "nope.yaml#/X" } } } } } } } } });
        let error = format!(
            "{:#}",
            bundle(&mut doc, "spec.json", dir.path(), false).unwrap_err()
        );
        assert!(
            error.contains("nope.yaml#/X") && error.contains("redocly bundle"),
            "{error}"
        );
    }
}
