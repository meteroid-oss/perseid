//! Bundles `$ref`s to other documents into the main one: each referenced schema is hoisted into
//! `components.schemas` under a name that collides with none, and the reference points to it.
//! Other referenced objects (path items, responses, parameters, ...) are inlined where they are
//! referenced.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

use super::{is_url, load, normalize::percent_decode, swagger2, upgrade};

/// Inlines the path items, parameters and responses a Swagger 2.0 document, read from `location`,
/// references in other files, before its conversion reshapes them. The schemas they reference are
/// left to `bundle`, their references made relative to the main document.
pub(super) fn inline_swagger_2(doc: &mut Value, location: &str, root: &Path) -> Result<()> {
    let mut inliner = Inliner {
        root,
        main: normalize(location),
        docs: BTreeMap::new(),
        inlining: Vec::new(),
    };
    if let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) {
        for (path, item) in paths.iter_mut() {
            if !path.starts_with("x-") {
                inliner.inline(item, Kind::PathItem)?;
            }
        }
    }
    for (key, kind) in [
        ("parameters", Kind::Parameter),
        ("responses", Kind::Response),
    ] {
        if let Some(shared) = doc.get_mut(key).and_then(Value::as_object_mut) {
            for value in shared.values_mut() {
                inliner.inline(value, kind)?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Kind {
    PathItem,
    Parameter,
    Response,
}

struct Inliner<'a> {
    root: &'a Path,
    main: String,
    /// Loaded documents by location, as written.
    docs: BTreeMap<String, Value>,
    inlining: Vec<(String, String)>,
}

impl Inliner<'_> {
    /// Replaces `value`, a `kind` object, and those below it by what they reference in other files.
    fn inline(&mut self, value: &mut Value, kind: Kind) -> Result<()> {
        if let Some(Value::String(reference)) = value.get("$ref")
            && !reference.starts_with('#')
        {
            let reference = reference.clone();
            let (location, fragment) = locate(&reference, &self.main);
            if location == self.main {
                value["$ref"] = Value::String(format!("#{fragment}"));
                return Ok(());
            }
            let key = (location.clone(), fragment.clone());
            if self.inlining.contains(&key) {
                bail!("external reference `{reference}` contains itself");
            }
            let mut target = self.target(&location, &fragment)?;
            rebase(&mut target, &location, &self.main, self.root, false);
            if let (Value::Object(target), Value::Object(siblings)) = (&mut target, &mut *value) {
                siblings.remove("$ref");
                target.extend(std::mem::take(siblings));
            }
            *value = target;
            self.inlining.push(key);
            let inlined = self.inline(value, kind);
            self.inlining.pop();
            return inlined;
        }
        let Kind::PathItem = kind else {
            return Ok(());
        };
        let Some(item) = value.as_object_mut() else {
            return Ok(());
        };
        for (key, child) in item.iter_mut() {
            if key == "parameters" {
                self.parameters(child)?;
            } else if swagger2::METHODS.contains(&key.as_str())
                && let Some(op) = child.as_object_mut()
            {
                if let Some(parameters) = op.get_mut("parameters") {
                    self.parameters(parameters)?;
                }
                for (status, response) in op
                    .get_mut("responses")
                    .and_then(Value::as_object_mut)
                    .into_iter()
                    .flatten()
                {
                    if !status.starts_with("x-") {
                        self.inline(response, Kind::Response)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn parameters(&mut self, parameters: &mut Value) -> Result<()> {
        for parameter in parameters.as_array_mut().into_iter().flatten() {
            self.inline(parameter, Kind::Parameter)?;
        }
        Ok(())
    }

    fn target(&mut self, location: &str, fragment: &str) -> Result<Value> {
        let reference = format!("{location}#{fragment}");
        if !self.docs.contains_key(location) {
            let doc = load(location, self.root).with_context(|| resolving(&reference))?;
            self.docs.insert(location.to_owned(), doc);
        }
        self.docs[location]
            .pointer(&percent_decode(fragment))
            .cloned()
            .with_context(|| format!("unresolved external reference `{reference}`"))
    }
}

fn resolving(reference: &str) -> String {
    format!(
        "resolving external reference `{reference}` (to avoid it, bundle the spec first, for \
         example with `redocly bundle`)"
    )
}

/// Points the references of `value`, which lives in the document at `current`, at the same
/// objects from the document at `main`.
fn rebase(value: &mut Value, current: &str, main: &str, root: &Path, names: bool) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "$ref" && !names {
                    if let Value::String(reference) = child {
                        let (location, fragment) = locate(reference, current);
                        *reference = match (location == main, fragment.as_str()) {
                            (true, fragment) => format!("#{fragment}"),
                            (false, "") => relative(main, &location, root),
                            (false, fragment) => {
                                format!("{}#{fragment}", relative(main, &location, root))
                            }
                        };
                    }
                } else if !skipped(key, names) {
                    rebase(child, current, main, root, names_below(key, names));
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|i| rebase(i, current, main, root, false)),
        _ => {}
    }
}

/// `location` as a reference from the document at `main`, absolute when no relative path reaches
/// it.
fn relative(main: &str, location: &str, root: &Path) -> String {
    if is_url(location) || Path::new(location).is_absolute() {
        return location.to_owned();
    }
    let dir: Vec<Component> = Path::new(main)
        .parent()
        .map(|p| p.components().collect())
        .unwrap_or_default();
    let target: Vec<Component> = Path::new(location).components().collect();
    let common = dir.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if is_url(main)
        || dir[common..]
            .iter()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        let path = root.join(location);
        return std::path::absolute(&path)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
    }
    let mut path: PathBuf = dir[common..].iter().map(|_| Component::ParentDir).collect();
    path.extend(&target[common..]);
    path.to_string_lossy().into_owned()
}

/// Hoists every schema the document references from another file or URL, resolved from `location`.
/// `from_3_0` tells that the main document was OpenAPI 3.0 (`from_2_0`, Swagger 2.0 converted to
/// it), so that the schemas of plain files it references are upgraded as its own were.
pub(super) fn bundle(
    doc: &mut Value,
    location: &str,
    root: &Path,
    from_3_0: bool,
    from_2_0: bool,
) -> Result<()> {
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
        from_2_0,
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
        let key = locate(reference, &main);
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
    from_2_0: bool,
    /// Loaded documents by location, whether each is an OpenAPI document (already upgraded) and
    /// whether it was Swagger 2.0, whose objects moved.
    docs: BTreeMap<String, (Value, bool, bool)>,
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

    /// The local reference to `fragment` of the main document, where it moved to when it was
    /// Swagger 2.0.
    fn local(&self, fragment: &str) -> String {
        let moved = self
            .from_2_0
            .then(|| swagger2::fragment(fragment))
            .flatten();
        format!("#{}", moved.as_deref().unwrap_or(fragment))
    }

    /// What `fragment` of the document at `location` holds, upgraded from 3.0 when the main
    /// document was 3.0 and the document is a plain file of schemas or objects.
    fn target(&mut self, location: &str, fragment: &str, schema: bool) -> Result<Value> {
        let reference = format!("{location}#{fragment}");
        if !self.docs.contains_key(location) {
            let mut other = load(location, self.root).with_context(|| resolving(&reference))?;
            let swagger = swagger2::is_swagger(&other);
            let openapi = other.get("openapi").is_some() || swagger;
            if openapi {
                upgrade::to_3_1(&mut other).with_context(|| location.to_owned())?;
            }
            self.docs
                .insert(location.to_owned(), (other, openapi, swagger));
        }
        let (doc, openapi, swagger) = &self.docs[location];
        let moved = swagger.then(|| swagger2::fragment(fragment)).flatten();
        let mut target = doc
            .pointer(moved.as_deref().unwrap_or(fragment))
            .cloned()
            .with_context(|| format!("unresolved external reference `{reference}`"))?;
        if self.from_2_0 && !openapi {
            swagger2::schemas(&mut target);
        }
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
        let (location, fragment) = locate(reference, current);
        if location == self.main {
            return Ok(Err(self.local(&fragment)));
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
        let (location, fragment) = locate(reference, current);
        if location == self.main {
            return Ok(self.local(&fragment));
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

/// The (location, fragment) `reference`, seen from the document at `current`, points to.
fn locate(reference: &str, current: &str) -> (String, String) {
    let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
    let location = if file.is_empty() {
        current.to_owned()
    } else {
        join(current, file)
    };
    (location, fragment.to_owned())
}

/// Points the references of `doc`, read from `location`, to other files at their absolute path
/// or URL.
pub(super) fn absolute_refs(doc: &mut Value, location: &str, root: &Path) {
    let base = if is_url(location) {
        location.to_owned()
    } else {
        let path = root.join(location);
        std::path::absolute(&path)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    absolute(doc, &base, false);
}

fn absolute(value: &mut Value, base: &str, names: bool) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "$ref" && !names {
                    if let Value::String(reference) = child
                        && !reference.starts_with('#')
                    {
                        let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
                        let file = join(base, file);
                        *reference = match fragment {
                            "" => file,
                            fragment => format!("{file}#{fragment}"),
                        };
                    }
                } else if !skipped(key, names) {
                    absolute(child, base, names_below(key, names));
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| absolute(i, base, false)),
        _ => {}
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
        bundle(&mut doc, "spec.json", dir.path(), false, false).unwrap();
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
        bundle(&mut doc, "spec.json", dir.path(), false, false).unwrap();
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
    fn swagger_2_specs_reference_files_by_their_2_0_locations() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "owner.json",
            &json!({ "type": "object", "discriminator": "kind", "x-nullable": true,
                "properties": { "kind": { "type": "string" },
                    "pet": { "$ref": "spec.json#/definitions/Pet" } } }),
        );
        write(
            dir.path(),
            "other.json",
            &json!({ "swagger": "2.0", "definitions": {
                "Toy": { "properties": { "maker": { "$ref": "#/definitions/Maker" } } },
                "Maker": { "type": "string" }
            } }),
        );
        let mut doc = json!({ "swagger": "2.0", "definitions": { "Pet": { "properties": {
            "owner": { "$ref": "owner.json" },
            "toy": { "$ref": "other.json#/definitions/Toy" }
        } } } });
        upgrade::to_3_1(&mut doc).unwrap();
        bundle(&mut doc, "spec.json", dir.path(), true, true).unwrap();
        let schemas = &doc["components"]["schemas"];
        let pet = &schemas["Pet"]["properties"];
        assert_eq!(pet["owner"]["$ref"], "#/components/schemas/owner");
        assert_eq!(pet["toy"]["$ref"], "#/components/schemas/Toy");
        let owner = &schemas["owner"];
        assert_eq!(owner["discriminator"], json!({ "propertyName": "kind" }));
        assert_eq!(owner["type"], json!(["object", "null"]));
        assert_eq!(
            owner["properties"]["pet"]["$ref"],
            "#/components/schemas/Pet"
        );
        assert_eq!(
            schemas["Toy"]["properties"]["maker"]["$ref"],
            "#/components/schemas/Maker"
        );
        assert_eq!(schemas["Maker"], json!({ "type": "string" }));
    }

    #[test]
    fn rebased_references_reach_the_same_files_from_the_main_document() {
        let root = Path::new("/repo");
        for (main, location) in [
            ("api/swagger.yaml", "api/paths/pets.yaml"),
            ("api/swagger.yaml", "defs.yaml"),
            ("api/v1/swagger.yaml", "api/v2/defs.yaml"),
            ("swagger.yaml", "../defs.yaml"),
            (
                "https://h.example/v1/swagger.json",
                "https://h.example/defs.json",
            ),
        ] {
            let relative = relative(main, location, root);
            assert_eq!(join(main, &relative), location, "{main} {relative}");
        }
        assert_eq!(
            relative("../api/swagger.yaml", "defs.yaml", root),
            "/repo/defs.yaml"
        );
        let mut value = json!({ "a": { "$ref": "#/definitions/A" }, "b": { "$ref": "../swagger.yaml#/x" },
            "c": { "$ref": "../defs.yaml" }, "example": { "$ref": "#/kept" } });
        rebase(
            &mut value,
            "api/paths/pets.yaml",
            "api/swagger.yaml",
            root,
            false,
        );
        assert_eq!(
            value,
            json!({ "a": { "$ref": "paths/pets.yaml#/definitions/A" }, "b": { "$ref": "#/x" },
                "c": { "$ref": "defs.yaml" }, "example": { "$ref": "#/kept" } })
        );
    }

    #[test]
    fn swagger_2_objects_of_other_files_are_inlined_before_the_conversion() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "params.json",
            &json!({ "a": { "$ref": "#/b" }, "b": { "name": "b", "in": "body", "schema": {} },
                "loop": { "$ref": "#/loop" } }),
        );
        let mut doc = json!({ "swagger": "2.0", "paths": {
            "/x": { "post": { "parameters": [
                { "$ref": "params.json#/a", "description": "d" }
            ] } },
            "x-skipped": { "$ref": "nope.json" }
        } });
        inline_swagger_2(&mut doc, "spec.json", dir.path()).unwrap();
        assert_eq!(
            doc["paths"]["/x"]["post"]["parameters"][0],
            json!({ "name": "b", "in": "body", "schema": {}, "description": "d" })
        );
        let mut looping = json!({ "swagger": "2.0", "parameters": {
            "L": { "$ref": "params.json#/loop" } } });
        let error = inline_swagger_2(&mut looping, "spec.json", dir.path()).unwrap_err();
        assert!(error.to_string().contains("contains itself"), "{error}");
    }

    #[test]
    fn absolute_references_reach_other_files_from_anywhere() {
        let mut doc = json!({ "a": { "$ref": "../b.json#/definitions/B" }, "c": { "$ref": "#/c" },
            "d": { "$ref": "https://h.example/d.json" } });
        absolute_refs(&mut doc, "api/spec.json", Path::new("/repo"));
        assert_eq!(doc["a"]["$ref"], "/repo/b.json#/definitions/B");
        assert_eq!(doc["c"]["$ref"], "#/c");
        assert_eq!(doc["d"]["$ref"], "https://h.example/d.json");
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
        bundle(&mut doc, "spec.json", dir.path(), true, false).unwrap();
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
        bundle(&mut doc, "spec.json", dir.path(), false, false).unwrap();
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
        bundle(&mut doc, "openapi.json", dir.path(), false, false).unwrap();
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
            bundle(&mut doc, "spec.json", dir.path(), false, false).unwrap_err()
        );
        assert!(
            error.contains("nope.yaml#/X") && error.contains("redocly bundle"),
            "{error}"
        );
    }
}
