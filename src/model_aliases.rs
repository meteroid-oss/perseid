//! Rust models that render the same code but for their names become aliases of one of them, so
//! the compiler checks and builds each shape once: a third of Stripe's models are duplicates.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8Path;
use heck::{ToSnakeCase as _, ToUpperCamelCase as _};

use crate::{api::Types, template::ident::ident};

/// Rewrites each model of `models` whose code another model's repeats as aliases of the items
/// of the one with the shortest name, likely a named schema rather than one an owner inlines. Returns the schemas aliased.
pub(crate) fn alias_duplicates(
    types: &Types,
    models: &Utf8Path,
) -> anyhow::Result<BTreeSet<String>> {
    let mut shapes: BTreeMap<String, Vec<Model>> = BTreeMap::new();
    for name in types.keys() {
        let path = models.join(format!("{}.rs", name.to_snake_case()));
        if !path.exists() {
            continue;
        }
        let text = fs_err::read_to_string(&path)?;
        if let Some(model) = Model::read(name, text) {
            shapes.entry(model.shape()).or_default().push(model);
        }
    }

    let mut aliased = BTreeSet::new();
    for models_of_shape in shapes.values_mut() {
        models_of_shape.sort_by(|a, b| (a.main.len(), &a.schema).cmp(&(b.main.len(), &b.schema)));
        let Some((canonical, duplicates)) = models_of_shape.split_first() else {
            continue;
        };
        for duplicate in duplicates {
            let path = models.join(format!("{}.rs", duplicate.schema.to_snake_case()));
            fs_err::write(&path, duplicate.aliases_of(canonical)?)?;
            aliased.insert(duplicate.schema.clone());
        }
    }
    Ok(aliased)
}

struct Model {
    schema: String,
    /// The type the schema names, which every public item of its file starts with.
    main: String,
    items: BTreeSet<String>,
    text: String,
}

impl Model {
    fn read(schema: &str, text: String) -> Option<Self> {
        let main = schema.to_upper_camel_case();
        let items = public_items(&text);
        (items.contains(&main) && items.iter().all(|item| item.starts_with(&main))).then_some(
            Self {
                schema: schema.to_owned(),
                main,
                items,
                text,
            },
        )
    }

    /// The code with the items it defines renamed after their suffix, without blank lines and
    /// the docs of top-level items, which name the schema.
    fn shape(&self) -> String {
        let renamed = map_identifiers(&self.text, |ident| {
            self.items
                .contains(ident)
                .then(|| format!("\0{}", &ident[self.main.len()..]))
        });
        renamed
            .lines()
            .filter(|line| !line.starts_with("///") && !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn aliases_of(&self, canonical: &Self) -> anyhow::Result<String> {
        let module = ident(&canonical.schema, "snake", Some("rust"))?;
        let mut out = String::from("// this file is @generated\n");
        out.push_str(&self.main_attributes());
        for item in &self.items {
            let suffix = &item[self.main.len()..];
            let target = format!("{}{suffix}", canonical.main);
            anyhow::ensure!(
                canonical.items.contains(&target),
                "no `{target}` to alias `{item}` to"
            );
            if item != &self.main {
                out.push_str(&format!(
                    "/// [`{target}`], under the name of this schema.\n"
                ));
            }
            out.push_str(&format!("pub type {item} = super::{module}::{target};\n"));
        }
        Ok(out)
    }

    /// The docs and `#[deprecated]` of the type the schema names.
    fn main_attributes(&self) -> String {
        let lines: Vec<&str> = self.text.lines().collect();
        let declaration =
            ["struct", "enum", "type"].map(|kind| format!("pub {kind} {}", self.main));
        let Some(at) = lines.iter().position(|line| {
            let line = line.trim_start();
            declaration.iter().any(|d| {
                line.strip_prefix(d.as_str()).is_some_and(|rest| {
                    !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                })
            })
        }) else {
            return String::new();
        };
        let start = lines[..at]
            .iter()
            .rposition(|line| {
                let line = line.trim_start();
                !(line.starts_with("///") || line.starts_with("#["))
            })
            .map_or(0, |line| line + 1);
        lines[start..at]
            .iter()
            .map(|line| line.trim_start())
            .filter(|line| line.starts_with("///") || line.starts_with("#[deprecated"))
            .map(|line| format!("{line}\n"))
            .collect()
    }
}

/// The names of the `pub struct`, `pub enum` and `pub type` items of `text`.
fn public_items(text: &str) -> BTreeSet<String> {
    let mut items = BTreeSet::new();
    for kind in ["pub struct ", "pub enum ", "pub type "] {
        for (at, _) in text.match_indices(kind) {
            let name: String = text[at + kind.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                items.insert(name);
            }
        }
    }
    items
}

/// `text` with the identifiers `rename` maps replaced.
fn map_identifiers(text: &str, rename: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_alphanumeric() || c == '_') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let word = &rest[..end];
        match rename(word) {
            Some(renamed) => out.push_str(&renamed),
            None => out.push_str(word),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(schema: &str, text: &str) -> Model {
        Model::read(schema, text.to_owned()).expect("a model")
    }

    const CARD: &str = "// this file is @generated\n/// A card.\n#[derive(Clone)]\npub struct Card {\n    /// The brand.\n    pub brand: CardBrand,\n    pub other: Other,\n}\n/// One of a or b.\npub enum CardBrand {\n    A,\n}\n";

    #[test]
    fn models_differing_by_their_names_and_top_level_docs_share_a_shape() {
        let wallet = CARD
            .replace("Card", "Wallet")
            .replace("/// A card.", "/// A wallet.\n/// Kept apart.");
        let undocumented = CARD.replace("Card", "Wallet").replace("/// A card.", "");
        assert_eq!(
            model("card", CARD).shape(),
            model("wallet", &undocumented).shape()
        );
        assert_eq!(
            model("card", CARD).shape(),
            model("wallet", &wallet).shape()
        );
    }

    #[test]
    fn models_differing_by_a_field_doc_or_a_referenced_type_do_not() {
        let other_doc = CARD
            .replace("Card", "Wallet")
            .replace("The brand", "Its brand");
        let other_ref = CARD.replace("Card", "Wallet").replace("Other", "Another");
        let card = model("card", CARD).shape();
        assert_ne!(card, model("wallet", &other_doc).shape());
        assert_ne!(card, model("wallet", &other_ref).shape());
    }

    #[test]
    fn a_duplicate_aliases_every_item_and_keeps_its_docs() {
        let wallet = CARD.replace("Card", "Wallet").replace("A card", "A wallet");
        let aliases = model("wallet", &wallet)
            .aliases_of(&model("card", CARD))
            .unwrap();
        assert_eq!(
            aliases,
            "// this file is @generated\n/// A wallet.\npub type Wallet = super::card::Card;\n\
             /// [`CardBrand`], under the name of this schema.\n\
             pub type WalletBrand = super::card::CardBrand;\n"
        );
    }

    #[test]
    fn files_with_items_not_named_after_their_schema_are_left_alone() {
        assert!(Model::read("card", format!("{CARD}pub struct Helper;\n")).is_none());
    }
}
