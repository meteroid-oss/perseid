//! Unions of several objects without a discriminator, told apart by their properties.

use itertools::Itertools as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{Field, FieldType, TypeData, Types};

/// How a union picks the variant of a JSON object when several variants are objects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UnionMode {
    /// By JSON type alone: at most one variant is an object.
    #[default]
    Json,
    /// By the `when` conditions of the object variants, checked in `rank` order.
    Rules,
    /// By the variant whose required properties are present and which knows the most.
    BestMatch,
}

impl UnionMode {
    pub(crate) fn is_json(&self) -> bool {
        *self == Self::Json
    }

    /// The value of `x-perseid-union`.
    pub(crate) fn from_extension(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "json" => Some(Self::Json),
            "best-match" => Some(Self::BestMatch),
            _ => None,
        }
    }
}

/// How a decoder picks the variant of a JSON value in a union.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UnionDecode {
    /// By JSON type alone: no two variants share one.
    #[default]
    JsonType,
    /// By trying the variants in `try_variants()` order: some share a JSON type.
    Try,
}

impl UnionDecode {
    pub(crate) fn is_json_type(&self) -> bool {
        *self == Self::JsonType
    }
}

/// The JSON type of a value, and of the items of an array when it has to be told apart.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct JsonShape {
    /// `string`, `integer`, `number`, `boolean`, `array` or `object`.
    pub json_type: String,
    /// What the items of an `array` look like, absent when any item is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonShape>>,
}

/// A property a JSON object must have, equal to `value` when set.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Condition {
    pub property: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// A property of an object variant: whether it is required, and its only value if any.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Property {
    pub name: String,
    pub required: bool,
    pub constant: Option<Value>,
}

/// The wire properties of struct `name`, embedded `allOf` parts included, or `None` when it is
/// not a struct.
pub(crate) fn shape(types: &Types, name: &str) -> Option<Vec<Property>> {
    fn collect(types: &Types, name: &str, depth: usize, out: &mut Vec<Property>) -> Option<()> {
        if depth > 16 {
            return None;
        }
        match &types.get(name)?.data {
            TypeData::Struct { fields, .. } => {
                for field in fields {
                    match &field.r#type {
                        FieldType::SchemaRef { name, .. } if field.flatten => {
                            collect(types, name, depth + 1, out)?
                        }
                        _ => out.push(property(types, field)),
                    }
                }
                Some(())
            }
            TypeData::Alias { target } => match &**target {
                FieldType::SchemaRef { name, .. } => collect(types, name, depth + 1, out),
                _ => None,
            },
            _ => None,
        }
    }
    let mut out = Vec::new();
    collect(types, name, 0, &mut out)?;
    Some(out.into_iter().unique_by(|p| p.name.clone()).collect())
}

fn property(types: &Types, field: &Field) -> Property {
    let enum_constant = || match &field.r#type {
        FieldType::SchemaRef { name, .. } if !field.nullable => match &types.get(name)?.data {
            TypeData::StringEnum { values, .. } if values.len() == 1 => {
                Some(Value::String(values[0].clone()))
            }
            _ => None,
        },
        _ => None,
    };
    Property {
        name: field.name.clone(),
        required: field.required,
        constant: field.constant.clone().or_else(enum_constant),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Test {
    Equals(Value),
    Present,
}

/// Decision rules telling apart objects of the given shapes: for each shape its rank, the
/// order in which rules are checked, and its conditions. `None` when some shapes cannot be
/// told apart by the constants and required properties they declare.
pub(crate) fn infer(shapes: &[Vec<Property>]) -> Option<Vec<(usize, Vec<Condition>)>> {
    let conditions: Vec<Vec<(String, Test)>> = shapes
        .iter()
        .map(|shape| {
            let mut conds: Vec<(String, Test)> = shape
                .iter()
                .filter(|p| p.required)
                .map(|p| {
                    let test = p.constant.clone().map_or(Test::Present, Test::Equals);
                    (p.name.clone(), test)
                })
                .collect();
            conds.sort_by_key(|(_, test)| matches!(test, Test::Present));
            conds
        })
        .collect();
    // Whether an object of `shape` fails `cond`: it lacks the property or has another constant.
    let fails = |shape: &[Property], (name, test): &(String, Test)| match shape
        .iter()
        .find(|p| &p.name == name)
    {
        None => true,
        Some(p) => match (test, &p.constant) {
            (Test::Equals(value), Some(other)) => value != other,
            _ => false,
        },
    };
    let excludes =
        |conds: &[&(String, Test)], shape: &[Property]| conds.iter().any(|&c| fails(shape, c));
    let n = shapes.len();
    // `a` is checked after `b` when objects of `b` may pass every condition of `a`.
    let after: Vec<Vec<usize>> = (0..n)
        .map(|a| {
            let all: Vec<_> = conditions[a].iter().collect();
            (0..n)
                .filter(|&b| b != a && !excludes(&all, &shapes[b]))
                .collect()
        })
        .collect();
    let equalities = |a: usize| {
        conditions[a]
            .iter()
            .filter(|(_, t)| matches!(t, Test::Equals(_)))
            .count()
    };
    let mut order = Vec::with_capacity(n);
    let mut done = vec![false; n];
    while order.len() < n {
        let next = (0..n)
            .filter(|&a| !done[a] && after[a].iter().all(|&b| done[b]))
            .max_by_key(|&a| (equalities(a), std::cmp::Reverse(a)))?;
        done[next] = true;
        order.push(next);
    }
    let mut rules = vec![(0, Vec::new()); n];
    for (rank, &a) in order.iter().enumerate() {
        let later: Vec<&Vec<Property>> = order[rank + 1..].iter().map(|&b| &shapes[b]).collect();
        let conds: Vec<&(String, Test)> = conditions[a].iter().collect();
        let chosen = minimal(&conds, |subset| {
            later.iter().all(|shape| excludes(subset, shape))
        });
        let when = chosen
            .into_iter()
            .map(|(property, test)| Condition {
                property: property.clone(),
                value: match test {
                    Test::Equals(value) => Some(value.clone()),
                    Test::Present => None,
                },
            })
            .collect();
        rules[a] = (rank, when);
    }
    Some(rules)
}

/// The smallest non-empty subset of `conds` (equalities first) that `enough` accepts, found
/// among subsets of at most three, else greedily.
fn minimal<'a>(
    conds: &[&'a (String, Test)],
    enough: impl Fn(&[&'a (String, Test)]) -> bool,
) -> Vec<&'a (String, Test)> {
    if conds.is_empty() {
        return Vec::new();
    }
    for size in 1..=3.min(conds.len()) {
        if let Some(subset) = conds
            .iter()
            .copied()
            .combinations(size)
            .find(|subset| enough(subset))
        {
            return subset;
        }
    }
    let mut chosen = Vec::new();
    for &cond in conds {
        chosen.push(cond);
        if enough(&chosen) {
            break;
        }
    }
    chosen
}

/// Best-match data of a shape: its required properties and every property it declares.
pub(crate) fn best_match(shape: &[Property]) -> (Vec<String>, Vec<String>) {
    let required = shape
        .iter()
        .filter(|p| p.required)
        .map(|p| p.name.clone())
        .collect();
    let properties = shape.iter().map(|p| p.name.clone()).collect();
    (required, properties)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn prop(name: &str, required: bool, constant: Option<Value>) -> Property {
        Property {
            name: name.into(),
            required,
            constant,
        }
    }

    type Rule = (usize, Vec<(String, Option<Value>)>);

    fn rules(shapes: &[Vec<Property>]) -> Option<Vec<Rule>> {
        infer(shapes).map(|rules| {
            rules
                .into_iter()
                .map(|(rank, when)| {
                    let when = when.into_iter().map(|c| (c.property, c.value)).collect();
                    (rank, when)
                })
                .collect()
        })
    }

    #[test]
    fn a_constant_and_an_extra_flag_tell_deleted_objects_apart() {
        let customer = vec![
            prop("id", true, None),
            prop("object", true, Some(json!("customer"))),
            prop("email", false, None),
        ];
        let deleted = vec![
            prop("deleted", true, Some(json!(true))),
            prop("id", true, None),
            prop("object", true, Some(json!("customer"))),
        ];
        assert_eq!(
            rules(&[customer, deleted]).unwrap(),
            [
                (1, vec![("object".into(), Some(json!("customer")))]),
                (0, vec![("deleted".into(), Some(json!(true)))]),
            ]
        );
    }

    #[test]
    fn distinct_constants_decide_alone() {
        let a = vec![prop("type", true, Some(json!("a"))), prop("x", true, None)];
        let b = vec![prop("type", true, Some(json!("b"))), prop("y", true, None)];
        assert_eq!(
            rules(&[a, b]).unwrap(),
            [
                (0, vec![("type".into(), Some(json!("a")))]),
                (1, vec![("type".into(), Some(json!("b")))]),
            ]
        );
    }

    #[test]
    fn required_properties_of_one_variant_decide() {
        let a = vec![prop("url", true, None), prop("name", false, None)];
        let b = vec![prop("file_id", true, None), prop("name", false, None)];
        assert_eq!(
            rules(&[a, b]).unwrap(),
            [
                (0, vec![("url".into(), None)]),
                (1, vec![("file_id".into(), None)]),
            ]
        );
    }

    #[test]
    fn indistinguishable_shapes_have_no_rules() {
        let a = vec![prop("name", true, None)];
        let b = vec![prop("name", true, None), prop("size", false, None)];
        assert!(rules(&[a.clone(), b]).is_none());
        assert!(rules(&[vec![prop("x", false, None)], vec![prop("y", false, None)]]).is_none());
    }
}
