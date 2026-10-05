//! Sample JSON instances of every model, for round-trip tests of the generated SDKs.
//!
//! Internal: the hidden `perseid samples [--spec <path>] --out <file.json>` subcommand writes, per
//! schema of the spec, `{"type_name", "names", "kind", "samples": [{"name", "json"}, ...]}`, where
//! `names` holds the type name each language generates. The samples are derived from the API
//! model, so they are valid for the schema as perseid models it, and deterministic:
//!
//! - `full`: every property present, optional and nullable ones too, two items per array, two
//!   keys per map, nested models down to a bounded depth (cycles are cut by leaving optional
//!   properties out and arrays empty);
//! - `minimal`: required properties only;
//! - `nulls`: every nullable property, array item and map value is `null`; `nulls_items` keeps
//!   nullable properties set and nulls their nullable items and values, when that differs;
//! - `variant_<name>`: one sample per variant of a union, and `full_pick_<n>` the same for the
//!   unions nested in a struct, `value_<n>` one per value of an enum.

use std::collections::BTreeMap;

use anyhow::Result;
use heck::{ToSnakeCase as _, ToUpperCamelCase as _};
use serde_json::{Map, Value, json};

use crate::{
    api::{
        FieldType, Types,
        types::{EnumVariantType, Field, StructEnumRepr, Type, TypeData},
    },
    config::LANGUAGES,
    spec::{self, Filters},
};

/// Models nested deeper than this are cut short, whatever they hold.
const MAX_DEPTH: usize = 12;
/// Values one sample may hold before every remaining optional property is left out.
const BUDGET: usize = 4000;
/// Samples of the unions nested in a struct, at most.
const MAX_PICKS: usize = 8;

/// The spec text, the filters of no language, and the filters of each language.
type SpecWithFilters = (String, Filters, Vec<(&'static str, Filters)>);

/// Reads the spec at `location` (a path under `root` or a URL), with the filters of every
/// language and of none, as `perseid samples` does without a `perseid.toml`.
pub fn without_config(location: &str, root: &std::path::Path) -> Result<SpecWithFilters> {
    let text = spec::read(location, root)?;
    let filters = |reserved| Filters {
        include_mode: Default::default(),
        excluded: Default::default(),
        specified: Default::default(),
        pagination: Vec::new(),
        reserved,
        names: BTreeMap::new(),
    };
    let targets = LANGUAGES
        .into_iter()
        .map(|language| {
            (
                language,
                filters(crate::reserved::type_names(language, "Client")),
            )
        })
        .collect();
    Ok((text, filters(Default::default()), targets))
}

/// The samples of every model of `spec`, keyed by schema name. `base` shapes the model, and each
/// language of `languages` names the types it generates.
pub fn run(spec: &str, base: &Filters, languages: &[(&str, Filters)]) -> Result<Value> {
    let (api, _) = spec::api_with_renames(spec, base)?;
    let mut renames = Vec::new();
    for (language, filters) in languages {
        let (_, renamed) = spec::api_with_renames(spec, filters)?;
        renames.push((*language, renamed));
    }
    let mut generator = Gen::new(&api.types, false);
    let mut out = Map::new();
    for (name, ty) in &api.types {
        let (kind, samples) = generator.samples(name, ty);
        let names: Map<String, Value> = renames
            .iter()
            .map(|(language, renamed)| {
                let generated = renamed.get(name).unwrap_or(name).to_upper_camel_case();
                ((*language).to_owned(), Value::from(generated))
            })
            .collect();
        let mut entry = Map::new();
        entry.insert("type_name".into(), Value::from(name.to_upper_camel_case()));
        entry.insert("names".into(), Value::Object(names));
        entry.insert("kind".into(), Value::from(kind));
        entry.insert("samples".into(), Value::Array(samples));
        out.insert(name.clone(), Value::Object(entry));
    }
    Ok(Value::Object(out))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    Minimal,
    /// Nullable properties, items and values are `null`.
    Nulls,
    /// Nullable properties are set, their nullable items and values are `null`.
    NullItems,
}

#[derive(Clone, Copy)]
struct Cx {
    mode: Mode,
    /// Which variant the unions take, modulo their number of variants.
    pick: usize,
}

struct Gen<'a> {
    types: &'a Types,
    /// The models being expanded, outermost first.
    stack: Vec<&'a str>,
    /// How many `null`s the current sample holds.
    nulls: usize,
    budget: usize,
    /// Strings are all `sample`, for samples embedded in source code.
    plain: bool,
}

/// The minimal instance of schema `name`, with plain strings: the request bodies and responses
/// of the generated tests.
pub(crate) fn minimal<'a>(types: &'a Types, name: &'a str) -> Value {
    Gen::new(types, true).run(name, Mode::Minimal, 0, 0).0
}

/// The minimal instance of `ty`, with plain strings.
pub(crate) fn minimal_of<'a>(types: &'a Types, ty: &'a FieldType) -> Value {
    let mut generator = Gen::new(types, true);
    generator.value(
        ty,
        0,
        Cx {
            mode: Mode::Minimal,
            pick: 0,
        },
        false,
    )
}

fn mix(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn choose<T>(items: &[T], seed: u64) -> &T {
    &items[(seed % items.len() as u64) as usize]
}

fn sample(name: &str, json: Value) -> Value {
    json!({ "name": name, "json": json })
}

/// Strings whose format the model does not keep, told by the name of the property.
fn hinted(name: &str, ty: &FieldType) -> Option<Value> {
    if *ty != FieldType::String {
        return None;
    }
    let name = name.to_lowercase();
    let value = if name.contains("email") {
        "alice@example.com"
    } else if name.contains("uuid") {
        "123e4567-e89b-12d3-a456-426614174000"
    } else if name.contains("base64") || name.contains("byte") {
        "SGVsbG8sIHdvcmxkIQ=="
    } else {
        return None;
    };
    Some(Value::from(value))
}

fn json_width(ty: &FieldType) -> usize {
    match ty {
        FieldType::Union { variants, .. } => variants
            .iter()
            .map(|v| json_width(&v.r#type))
            .max()
            .unwrap_or(1)
            .max(variants.len()),
        FieldType::List { inner } | FieldType::Set { inner } | FieldType::Nullable { inner } => {
            json_width(inner)
        }
        FieldType::Map { value_ty } => json_width(value_ty),
        _ => 1,
    }
}

/// How many variants the unions nested in `ty` have, at most.
fn width(ty: &Type) -> usize {
    let widest = |fields: &[Field]| fields.iter().map(|f| json_width(&f.r#type)).max();
    let width = match &ty.data {
        TypeData::Struct {
            fields,
            additional_properties,
        } => widest(fields)
            .into_iter()
            .chain(additional_properties.as_deref().map(json_width))
            .max(),
        TypeData::StructEnum { fields, repr, .. } => {
            let (StructEnumRepr::AdjacentlyTagged { variants, .. }
            | StructEnumRepr::InternallyTagged { variants }) = repr;
            let nested = variants.iter().filter_map(|v| match &v.content {
                EnumVariantType::Struct { fields } => widest(fields),
                EnumVariantType::Ref { .. } => None,
            });
            widest(fields).into_iter().chain(nested).max()
        }
        TypeData::Alias { target } => Some(json_width(target)),
        _ => None,
    };
    width.unwrap_or(1).min(MAX_PICKS)
}

impl<'a> Gen<'a> {
    fn new(types: &'a Types, plain: bool) -> Self {
        Self {
            types,
            stack: Vec::new(),
            nulls: 0,
            budget: BUDGET,
            plain,
        }
    }

    /// The kind and the samples of the model `name`.
    fn samples(&mut self, name: &'a str, ty: &'a Type) -> (&'static str, Vec<Value>) {
        let mut out = Vec::new();
        let mut kind = "struct";
        let (full, _) = self.run(name, Mode::Full, 0, 0);
        out.push(sample("full", full));
        match &ty.data {
            TypeData::StringEnum { values } => {
                kind = "enum";
                for seed in 0..values.len() {
                    let (json, _) = self.run(name, Mode::Full, 0, seed as u64);
                    out.push(sample(&format!("value_{seed}"), json));
                }
            }
            TypeData::IntegerEnum { variants } => {
                kind = "enum";
                for seed in 0..variants.len() {
                    let (json, _) = self.run(name, Mode::Full, 0, seed as u64);
                    out.push(sample(&format!("value_{seed}"), json));
                }
            }
            TypeData::StringAlias => kind = "alias",
            TypeData::Alias { target } => {
                kind = "alias";
                if let FieldType::Union { variants, .. } = target.non_null() {
                    kind = "union";
                    for (pick, variant) in variants.iter().enumerate() {
                        let (json, _) = self.run(name, Mode::Full, pick, 0);
                        out.push(sample(&format!("variant_{}", variant.name), json));
                    }
                } else {
                    self.picks(name, ty, &mut out);
                }
            }
            TypeData::StructEnum { repr, .. } => {
                kind = "union";
                let (StructEnumRepr::AdjacentlyTagged { variants, .. }
                | StructEnumRepr::InternallyTagged { variants }) = repr;
                for (pick, variant) in variants.iter().enumerate() {
                    let (json, _) = self.run(name, Mode::Full, pick, 0);
                    out.push(sample(&format!("variant_{}", variant.name), json));
                }
            }
            TypeData::Struct { .. } => self.picks(name, ty, &mut out),
        }
        let (minimal, _) = self.run(name, Mode::Minimal, 0, 0);
        out.push(sample("minimal", minimal));
        let (nulls, count) = self.run(name, Mode::Nulls, 0, 0);
        if count > 0 {
            let (items, _) = self.run(name, Mode::NullItems, 0, 0);
            if items != nulls {
                out.push(sample("nulls_items", items));
            }
            out.push(sample("nulls", nulls));
        }
        (kind, out)
    }

    /// Samples taking the other variants of the unions nested in `ty`.
    fn picks(&mut self, name: &'a str, ty: &Type, out: &mut Vec<Value>) {
        for pick in 1..width(ty) {
            let (json, _) = self.run(name, Mode::Full, pick, 0);
            out.push(sample(&format!("full_pick_{pick}"), json));
        }
    }

    fn run(&mut self, name: &'a str, mode: Mode, pick: usize, seed: u64) -> (Value, usize) {
        self.stack.clear();
        self.nulls = 0;
        self.budget = BUDGET;
        let json = self.model(name, seed, Cx { mode, pick });
        (json, self.nulls)
    }

    fn model(&mut self, name: &'a str, seed: u64, cx: Cx) -> Value {
        let types = self.types;
        let Some(ty) = types.get(name) else {
            return json!({});
        };
        if self.stack.len() >= MAX_DEPTH {
            return json!({});
        }
        let cyclic = self.stack.contains(&name);
        self.stack.push(name);
        let depth = self.stack.len();
        let cut = (cyclic && depth >= 3) || depth >= 6 || self.budget == 0;
        let json = self.of_type(ty, seed, cx, cut);
        self.stack.pop();
        json
    }

    fn of_type(&mut self, ty: &'a Type, seed: u64, cx: Cx, cut: bool) -> Value {
        match &ty.data {
            TypeData::Struct {
                fields,
                additional_properties,
            } => {
                let mut out = Map::new();
                self.fields(fields, &ty.discriminator_defaults, seed, cx, cut, &mut out);
                if let Some(FieldType::Map { value_ty }) = additional_properties.as_deref()
                    && cx.mode != Mode::Minimal
                    && !cut
                {
                    for (i, key) in ["extra_a", "extra_b"].into_iter().enumerate() {
                        if !out.contains_key(key) {
                            let value = self.value(value_ty, mix(seed, 100 + i as u64), cx, cut);
                            out.insert(key.to_owned(), value);
                        }
                    }
                }
                Value::Object(out)
            }
            TypeData::StringEnum { values } => values
                .get((seed % values.len().max(1) as u64) as usize)
                .map_or_else(|| Value::from(""), |v| Value::from(v.as_str())),
            TypeData::IntegerEnum { variants } => variants
                .get((seed % variants.len().max(1) as u64) as usize)
                .map_or_else(|| Value::from(0), |(_, v)| Value::from(*v)),
            TypeData::StringAlias => {
                Value::from(format!("{}_{}", ty.name.to_snake_case(), seed % 100))
            }
            TypeData::Alias { target } => self.value(target, seed, cx, cut),
            TypeData::StructEnum {
                discriminator_field,
                repr,
                fields,
            } => self.tagged(ty, discriminator_field, repr, fields, seed, cx, cut),
        }
    }

    /// An instance of one variant of a union of structs, with its tag.
    #[allow(clippy::too_many_arguments)]
    fn tagged(
        &mut self,
        ty: &'a Type,
        discriminator: &str,
        repr: &'a StructEnumRepr,
        common: &'a [Field],
        seed: u64,
        cx: Cx,
        cut: bool,
    ) -> Value {
        let (variants, content_field) = match repr {
            StructEnumRepr::AdjacentlyTagged {
                content_field,
                variants,
            } => (variants, Some(content_field)),
            StructEnumRepr::InternallyTagged { variants } => (variants, None),
        };
        if variants.is_empty() {
            return json!({});
        }
        let index = if cut {
            variants
                .iter()
                .position(|v| {
                    !matches!(
                        &v.content,
                        EnumVariantType::Ref { schema_ref: Some(n), .. }
                            if self.stack.contains(&n.as_str())
                    )
                })
                .unwrap_or(0)
        } else {
            cx.pick % variants.len()
        };
        let variant = &variants[index];
        // A variant without content (a bare tag) carries no content field.
        let body = match &variant.content {
            EnumVariantType::Ref {
                schema_ref: Some(target),
                ..
            } => Some(self.model(target, seed, cx)),
            EnumVariantType::Ref {
                schema_ref: None,
                inner: Some(inner),
            } => Some(self.of_type(inner, seed, cx, cut)),
            EnumVariantType::Ref { .. } => None,
            EnumVariantType::Struct { fields } => {
                let mut own = Map::new();
                self.fields(fields, &BTreeMap::new(), seed, cx, cut, &mut own);
                Some(Value::Object(own))
            }
        };
        let mut out = Map::new();
        out.insert(discriminator.to_owned(), Value::from(variant.name.as_str()));
        match content_field {
            Some(field) => {
                if let Some(body) = body {
                    out.insert(field.clone(), body);
                }
            }
            None => {
                if let Some(Value::Object(body)) = body {
                    for (key, value) in body {
                        if key != discriminator {
                            out.insert(key, value);
                        }
                    }
                }
            }
        }
        let mut shared = Map::new();
        self.fields(
            common,
            &ty.discriminator_defaults,
            mix(seed, 7),
            cx,
            cut,
            &mut shared,
        );
        for (key, value) in shared {
            out.entry(key).or_insert(value);
        }
        Value::Object(out)
    }

    fn fields(
        &mut self,
        fields: &'a [Field],
        defaults: &BTreeMap<String, String>,
        seed: u64,
        cx: Cx,
        cut: bool,
        out: &mut Map<String, Value>,
    ) {
        for (i, field) in fields.iter().enumerate() {
            let seed = mix(seed, i as u64 + 1);
            if field.flatten {
                if let FieldType::SchemaRef { name, .. } = &field.r#type
                    && let Value::Object(base) = self.model(name, seed, cx)
                {
                    for (key, value) in base {
                        out.entry(key).or_insert(value);
                    }
                }
                continue;
            }
            if !field.required && (cut || cx.mode == Mode::Minimal) {
                continue;
            }
            let value = if let Some(constant) = &field.constant {
                constant.clone()
            } else if let Some(tag) = defaults.get(&field.name) {
                Value::from(tag.as_str())
            } else if field.nullable && cx.mode == Mode::Nulls {
                self.nulls += 1;
                Value::Null
            } else if field.nullable && cut {
                Value::Null
            } else if let Some(value) = hinted(&field.name, &field.r#type) {
                value
            } else {
                self.value(&field.r#type, seed, cx, cut)
            };
            out.insert(field.name.clone(), value);
        }
    }

    fn value(&mut self, ty: &'a FieldType, seed: u64, cx: Cx, cut: bool) -> Value {
        self.budget = self.budget.saturating_sub(1);
        let cut = cut || self.budget == 0;
        match ty {
            FieldType::Bool => Value::from(seed.is_multiple_of(2)),
            FieldType::Int16 => Value::from(*choose(&[1234i64, -1234, 32767, -32768], seed)),
            FieldType::UInt16 => Value::from(*choose(&[54321u64, 65535, 1], seed)),
            FieldType::Int32 => Value::from(*choose(
                &[123_456_789i64, -123_456_789, 2_147_483_647, -2_147_483_648],
                seed,
            )),
            FieldType::Int64 => Value::from(*choose(
                &[9_007_199_254_740_993i64, -9_007_199_254_740_993],
                seed,
            )),
            FieldType::UInt64 => Value::from(*choose(&[u64::MAX, 9_007_199_254_740_993], seed)),
            FieldType::Float => Value::from(*choose(&[1.5f64, -2.25], seed)),
            FieldType::Double => Value::from(*choose(
                &[12_345.678_901_234_5f64, -0.000_123_456_789, 0.1],
                seed,
            )),
            FieldType::String if self.plain => Value::from("sample"),
            FieldType::String => Value::from(*choose(
                &[
                    "sample",
                    "quote \" backslash \\ slash / unicode \u{e9} \u{65e5}\u{672c} \u{1f389}",
                    "line\nbreak\ttab",
                ],
                seed,
            )),
            FieldType::Decimal => Value::from(*choose(&["12345.6789", "-0.000123"], seed)),
            FieldType::DateTime => Value::from(*choose(
                &[
                    "2024-03-15T10:30:45.123+02:00",
                    "2023-12-31T23:59:59.999-05:30",
                ],
                seed,
            )),
            FieldType::Date => Value::from(*choose(&["2024-02-29", "1999-12-31"], seed)),
            FieldType::Uri => Value::from(*choose(
                &[
                    "https://example.com/a/b?q=1&r=%C3%A9#frag",
                    "http://localhost:8080/",
                ],
                seed,
            )),
            FieldType::JsonObject => json!({
                "key": "value",
                "count": 3,
                "ratio": 0.5,
                "flags": [true, false],
                "nested": { "ok": true }
            }),
            FieldType::List { inner } | FieldType::Set { inner } => {
                let count = match () {
                    _ if cut => 0,
                    _ if cx.mode == Mode::Minimal || self.stack.len() > 3 => 1,
                    _ => 2,
                };
                let mut items: Vec<Value> = Vec::new();
                for i in 0..count {
                    let item = self.value(inner, seed.wrapping_add(i), cx, cut);
                    if !(matches!(ty, FieldType::Set { .. }) && items.contains(&item)) {
                        items.push(item);
                    }
                }
                Value::Array(items)
            }
            FieldType::Map { value_ty } => {
                let count = match () {
                    _ if cut => 0,
                    _ if cx.mode == Mode::Minimal || self.stack.len() > 3 => 1,
                    _ => 2,
                };
                let mut map = Map::new();
                for (i, key) in ["alpha", "beta"].into_iter().take(count).enumerate() {
                    let value = self.value(value_ty, seed.wrapping_add(i as u64), cx, cut);
                    map.insert(key.to_owned(), value);
                }
                Value::Object(map)
            }
            FieldType::Nullable { inner } => match cx.mode {
                Mode::Nulls | Mode::NullItems => {
                    self.nulls += 1;
                    Value::Null
                }
                Mode::Full | Mode::Minimal => self.value(inner, seed, cx, cut),
            },
            FieldType::SchemaRef { name, .. } => self.model(name, seed, cx),
            FieldType::Union { variants, .. } => {
                if variants.is_empty() {
                    return json!({});
                }
                let index = if cut {
                    variants
                        .iter()
                        .position(|v| {
                            !matches!(
                                &v.r#type,
                                FieldType::SchemaRef { name, .. }
                                    if self.stack.contains(&name.as_str())
                            )
                        })
                        .unwrap_or(0)
                } else {
                    cx.pick % variants.len()
                };
                let variant = &variants[index];
                if variant.empty {
                    Value::from("")
                } else {
                    self.value(&variant.r#type, seed, cx, cut)
                }
            }
            FieldType::StringEnum { values, .. } => values
                .get((seed % values.len().max(1) as u64) as usize)
                .map_or_else(|| Value::from(""), |v| Value::from(v.as_str())),
        }
    }
}
