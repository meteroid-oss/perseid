//! What the Rust templates need to know of other types: the union variants to box, given the
//! estimated sizes of the generated types, and the types constructors convert into.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

use heck::ToUpperCamelCase as _;

use crate::api::{
    Types,
    types::{EnumVariantType, Field, FieldType, SimpleVariant, StructEnumRepr, Type, TypeData},
};

/// Clippy's `large_enum_variant` threshold (200 bytes), less a margin for the estimates.
const LARGE_VARIANT: usize = 184;
const BOX: usize = 8;
const TAG: usize = 8;
const JSON_VALUE: usize = 32;
const MAP: usize = 24;

pub(crate) struct Sizes<'a> {
    types: &'a Types,
    known: RefCell<BTreeMap<&'a str, usize>>,
    visiting: RefCell<BTreeSet<&'a str>>,
}

impl<'a> Sizes<'a> {
    pub(crate) fn new(types: &'a Types) -> Self {
        Self {
            types,
            known: RefCell::default(),
            visiting: RefCell::default(),
        }
    }

    /// The variants of the union `ty` to box: the largest, while it outgrows the next one by
    /// more than clippy allows.
    pub(crate) fn boxed_variants(&self, ty: &'a Type) -> BTreeSet<&'a str> {
        let TypeData::StructEnum { repr, .. } = &ty.data else {
            return BTreeSet::new();
        };
        let mut sizes: Vec<(Option<&str>, usize)> = variants(repr)
            .iter()
            .map(|v| (Some(v.name.as_str()), self.variant(v)))
            .chain([(None, JSON_VALUE)])
            .collect();
        let mut boxed = BTreeSet::new();
        loop {
            sizes.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
            match sizes[..] {
                [(Some(name), largest), (_, next), ..] if largest > next + LARGE_VARIANT => {
                    boxed.insert(name);
                    sizes[0].1 = BOX;
                }
                _ => break boxed,
            }
        }
    }

    fn variant(&self, variant: &'a SimpleVariant) -> usize {
        match &variant.content {
            EnumVariantType::Struct { fields } => self.declared(fields) + MAP,
            EnumVariantType::Ref {
                schema_ref: Some(name),
                ..
            } => self.schema(name),
            EnumVariantType::Ref { .. } => 0,
        }
    }

    fn schema(&self, name: &'a str) -> usize {
        if let Some(size) = self.known.borrow().get(name) {
            return *size;
        }
        // A type leading back to itself is boxed.
        if !self.visiting.borrow_mut().insert(name) {
            return BOX;
        }
        let size = match self.types.get(name).map(|t| &t.data) {
            Some(TypeData::Struct { fields, .. }) => self.declared(fields) + MAP,
            Some(TypeData::StructEnum { fields, repr, .. }) => {
                let largest = (variants(repr).iter().map(|v| self.variant(v)))
                    .chain([JSON_VALUE])
                    .max()
                    .unwrap_or(JSON_VALUE);
                self.declared(fields) + largest + TAG
            }
            Some(TypeData::Alias { target }) => self.of(target),
            Some(TypeData::IntegerEnum { .. }) => 16,
            Some(TypeData::StringEnum { .. } | TypeData::StringAlias) | None => 24,
        };
        self.visiting.borrow_mut().remove(name);
        self.known.borrow_mut().insert(name, size);
        size
    }

    fn declared(&self, fields: &'a [Field]) -> usize {
        let size: usize = (fields.iter())
            .map(|f| match f.required && !f.nullable {
                true => self.of(&f.r#type),
                false => self.optional(&f.r#type),
            })
            .sum();
        size.next_multiple_of(8)
    }

    fn of(&self, ty: &'a FieldType) -> usize {
        match ty {
            FieldType::Bool => 1,
            FieldType::Int16 | FieldType::UInt16 => 2,
            FieldType::Int32 | FieldType::Float | FieldType::Date => 4,
            FieldType::Int64 | FieldType::UInt64 | FieldType::Double => 8,
            FieldType::DateTime => 12,
            FieldType::Decimal | FieldType::Uuid | FieldType::IntegerEnum { .. } => 16,
            FieldType::String
            | FieldType::Uri
            | FieldType::StringEnum { .. }
            | FieldType::List { .. }
            | FieldType::Set { .. } => 24,
            FieldType::JsonObject | FieldType::Union { .. } => JSON_VALUE,
            FieldType::Map { .. } => 48,
            FieldType::Nullable { inner } => self.optional(inner),
            FieldType::SchemaRef { name, .. } => self.schema(name),
        }
    }

    /// The size of an `Option` of the type: the same, but for numbers and dates, which need a
    /// tag of their alignment.
    fn optional(&self, ty: &'a FieldType) -> usize {
        let size = self.of(ty);
        match ty {
            FieldType::Int16 | FieldType::UInt16 | FieldType::Int32 | FieldType::Float => size * 2,
            FieldType::Int64 | FieldType::UInt64 | FieldType::Double => 16,
            FieldType::Decimal | FieldType::DateTime | FieldType::Date => size + 4,
            FieldType::Uuid => size + 1,
            _ => size,
        }
    }
}

/// The names of the types constructors take as `impl Into<_>`: enums, which strings or their
/// variants' payloads convert into.
pub(crate) fn convertible_types(types: &Types) -> BTreeSet<String> {
    let convertible = |ty: &Type| match &ty.data {
        TypeData::StringEnum { .. } => true,
        TypeData::StructEnum { fields, .. } => fields.is_empty(),
        TypeData::Alias { target } => matches!(**target, FieldType::Union { .. }),
        _ => false,
    };
    (types.values())
        .filter(|ty| convertible(ty))
        .map(|ty| ty.name.to_upper_camel_case())
        .collect()
}

/// The schema each alias of a schema stands for (`Updated` for `Created`), through chains;
/// a recursive alias is a newtype, a type of its own.
pub(crate) fn alias_targets(
    types: &Types,
    recursive: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let target = |name: &str| match types.get(name).map(|t| &t.data) {
        Some(TypeData::Alias { target }) if !recursive.contains(name) => match &**target {
            FieldType::SchemaRef { name, .. } => Some(name.clone()),
            _ => None,
        },
        _ => None,
    };
    let mut aliases = BTreeMap::new();
    for name in types.keys() {
        let mut resolved = name.clone();
        for _ in 0..types.len() {
            match target(&resolved) {
                Some(next) if next != *name => resolved = next,
                _ => break,
            }
        }
        if resolved != *name {
            aliases.insert(name.to_upper_camel_case(), resolved.to_upper_camel_case());
        }
    }
    aliases
}

fn variants(repr: &StructEnumRepr) -> &[SimpleVariant] {
    match repr {
        StructEnumRepr::AdjacentlyTagged { variants, .. }
        | StructEnumRepr::InternallyTagged { variants } => variants,
    }
}
