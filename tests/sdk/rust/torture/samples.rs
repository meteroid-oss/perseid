//! Decodes every schema-derived sample of every model with the generated types and encodes it
//! again (see tests/sdk/run.sh, which writes samples.json with `perseid samples` and the
//! registry of the model types with samples_registry.py).
//!
//! The only normalizations are the documented ones of the Rust SDK:
//! - date-times compare as instants, since the models keep them as UTC;
//! - an optional nullable property that is `null` decodes to `None` and is left out when
//!   encoding (only PATCH bodies tell `null` from absent), so a `null` the output lacks is fine.
//!
//! Integers, decimals, strings, unknown properties and everything else must come back exactly.
use std::collections::BTreeMap;

use chrono::DateTime;
use serde_json::Value;

type Codec = fn(&str) -> Result<Value, String>;

/// Decodes `text` as a `T`, then encodes the value.
fn codec<T: serde::de::DeserializeOwned + serde::Serialize>(text: &str) -> Result<Value, String> {
    let model: T = serde_json::from_str(text).map_err(|error| format!("decode failed: {error}"))?;
    serde_json::to_value(&model).map_err(|error| format!("encode failed: {error}"))
}

// `static CODECS: &[(&str, Codec)]`: the type of every schema, by schema name.
include!("samples_registry.in");

const SAMPLES: &str = include_str!("samples.json");

/// The first difference between what was sampled and what came back, if any.
fn difference(expected: &Value, actual: &Value, path: &str, dropped: &mut usize) -> Option<String> {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                match actual.get(key) {
                    Some(other) => {
                        if let Some(found) = difference(value, other, &format!("{path}/{key}"), dropped) {
                            return Some(found);
                        }
                    }
                    None if value.is_null() => *dropped += 1,
                    None => return Some(format!("{path}/{key}: lost, was {value}")),
                }
            }
            actual
                .iter()
                .find(|(key, _)| !expected.contains_key(*key))
                .map(|(key, value)| format!("{path}/{key}: added, is {value}"))
        }
        (Value::Array(expected), Value::Array(actual)) => {
            if expected.len() != actual.len() {
                return Some(format!("{path}: {} items became {}", expected.len(), actual.len()));
            }
            expected
                .iter()
                .zip(actual)
                .enumerate()
                .find_map(|(i, (a, b))| difference(a, b, &format!("{path}/{i}"), dropped))
        }
        (Value::String(a), Value::String(b)) if a != b => {
            match (DateTime::parse_from_rfc3339(a), DateTime::parse_from_rfc3339(b)) {
                (Ok(a), Ok(b)) if a == b => None,
                _ => Some(format!("{path}: {a:?} became {b:?}")),
            }
        }
        (a, b) if a == b => None,
        (a, b) => Some(format!("{path}: {a} became {b}")),
    }
}

#[test]
fn every_sample_of_every_model_round_trips() {
    let samples: Value = serde_json::from_str(SAMPLES).unwrap();
    let samples = samples.as_object().unwrap();
    let codecs: BTreeMap<&str, Codec> = CODECS.iter().copied().collect();
    assert_eq!(codecs.len(), samples.len(), "every model has a registered type");

    let (mut checked, mut dropped) = (0, 0);
    let mut failures = Vec::new();
    for (schema, model) in samples {
        let codec = codecs[schema.as_str()];
        let list = model["samples"].as_array().unwrap();
        assert!(!list.is_empty(), "{schema} has no sample");
        for sample in list {
            let (name, json) = (sample["name"].as_str().unwrap(), &sample["json"]);
            checked += 1;
            let fail = |reason: String| format!("{schema} sample `{name}`: {reason}\n    input: {json}");
            let encoded = match codec(&json.to_string()) {
                Ok(encoded) => encoded,
                Err(error) => {
                    failures.push(fail(error));
                    continue;
                }
            };
            if let Some(reason) = difference(json, &encoded, "", &mut dropped) {
                failures.push(fail(format!("{reason}\n    output: {encoded}")));
                continue;
            }
            // What the SDK wrote is a fixed point: reading it back gives it again.
            match codec(&encoded.to_string()) {
                Ok(again) if again == encoded => {}
                Ok(again) => failures.push(fail(format!("not stable, re-encoded as {again}"))),
                Err(error) => failures.push(fail(format!("own output: {error}"))),
            }
        }
    }
    println!("{checked} samples of {} models, {dropped} null properties left out", samples.len());
    assert!(failures.is_empty(), "{} of {checked} samples differ:\n{}", failures.len(), failures.join("\n"));
}
