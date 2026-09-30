use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use aide::openapi::OpenApi;
use anyhow::{Context as _, Result, bail};
use schemars::schema::Schema;
use serde_json::Value;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};

use crate::api::Api;

mod normalize;
mod upgrade;

#[derive(Copy, Clone, Default, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncludeMode {
    #[default]
    OnlyPublic,
    PublicAndInternal,
    OnlyInternal,
    OnlySpecified,
}

pub struct Filters {
    pub include_mode: IncludeMode,
    pub excluded: BTreeSet<String>,
    pub specified: BTreeSet<String>,
    pub pagination: Vec<crate::config::Pagination>,
    /// Type names the SDK's runtime or language already uses, which schemas are renamed from.
    pub reserved: BTreeSet<String>,
    pub method_names: crate::config::MethodNames,
    /// Method names by operation id.
    pub names: BTreeMap<String, String>,
}

/// Largest spec read from a URL: GitHub's REST description is over 10 MB, ureq's default.
const MAX_SPEC_BYTES: u64 = 200 * 1024 * 1024;

/// Reads an OpenAPI document (JSON or YAML) from a path or an http(s) URL, as JSON text, upgraded to 3.1 when it is 3.0.
pub(crate) fn read(location: &str, root: &Path) -> Result<String> {
    let text = if location.starts_with("https://") || location.starts_with("http://") {
        ureq::get(location)
            .call()
            .and_then(|mut r| {
                r.body_mut()
                    .with_config()
                    .limit(MAX_SPEC_BYTES)
                    .read_to_string()
            })
            .with_context(|| format!("downloading {location}"))?
    } else {
        std::fs::read_to_string(root.join(location))
            .with_context(|| format!("reading {location}"))?
    };
    let mut value: Value = if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).map_err(anyhow::Error::from)
    } else {
        serde_norway::from_str::<yaml::Json>(&text)
            .map(|json| json.0)
            .map_err(anyhow::Error::from)
    }
    .with_context(|| format!("parsing {location}"))?;
    upgrade::to_3_1(&mut value).with_context(|| location.to_owned())?;
    Ok(serde_json::to_string(&value)?)
}

pub(crate) fn api(spec: &str, filters: &Filters) -> Result<Api> {
    let mut doc: Value = serde_json::from_str(spec).context("the spec is not valid JSON")?;
    normalize::normalize(&mut doc)?;
    normalize::rename_reserved_schemas(&mut doc, &filters.reserved);
    let raw = doc;
    // `OpenApi` borrows its version string, so it cannot deserialize from a `Value`.
    let doc = serde_json::to_string(&raw)?;
    let mut spec: OpenApi =
        serde_json::from_str(&doc).context("the spec is not a valid OpenAPI 3 document")?;
    let webhooks = webhooks(&spec);
    let Some(paths) = spec.paths.take() else {
        bail!("the spec has no paths");
    };
    Api::new(
        paths,
        &mut spec.components.take().unwrap_or_default(),
        &webhooks,
        &raw,
        filters,
    )
}

fn webhooks(spec: &OpenApi) -> Vec<String> {
    let mut referenced = BTreeSet::new();
    if let Some(webhooks) = spec.extensions.get("x-webhooks").and_then(Value::as_object) {
        for method in webhooks
            .values()
            .filter_map(Value::as_object)
            .flat_map(|m| m.values())
        {
            if let Some(name) =
                method["requestBody"]["content"]["application/json"]["schema"]["$ref"]
                    .as_str()
                    .and_then(|r| r.split('/').next_back())
            {
                referenced.insert(name.to_owned());
            }
        }
    }
    for (_, webhook) in &spec.webhooks {
        let Some(item) = webhook.as_item() else {
            continue;
        };
        for (_, op) in item.iter() {
            if let Some(body) = op.request_body.as_ref().and_then(|b| b.as_item())
                && let Some(json) = body.content.get("application/json")
                && let Some(schema) = &json.schema
                && let Schema::Object(obj) = &schema.json_schema
                && let Some(name) = obj
                    .reference
                    .as_ref()
                    .and_then(|r| r.split('/').next_back())
            {
                referenced.insert(name.to_owned());
            }
        }
    }
    referenced.into_iter().collect()
}

/// Turns any `error!` logged by the engine into a failed generation.
pub(crate) struct FailOnError(pub Arc<AtomicBool>);

impl<S: tracing::Subscriber> Layer<S> for FailOnError {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() == Level::ERROR {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

/// Prints each distinct warning and error once per run, prefixed with the schema, field or
/// operation being read when the event happened.
pub(crate) struct Report;

/// Lines already printed, and per message how many were printed and how many held back.
struct Printed {
    lines: BTreeSet<String>,
    messages: BTreeMap<String, (usize, usize)>,
}

static PRINTED: Mutex<Printed> = Mutex::new(Printed {
    lines: BTreeSet::new(),
    messages: BTreeMap::new(),
});

/// Occurrences of one message printed before the rest are only counted.
const SHOWN_PER_MESSAGE: usize = 5;

/// Prints how many occurrences of each message were held back since the last call.
pub(crate) fn report_held_back() {
    let Ok(mut printed) = PRINTED.lock() else {
        return;
    };
    for (message, (_, held_back)) in printed.messages.iter_mut() {
        if *held_back > 0 {
            eprintln!("warning: {held_back} more like: {message}");
            *held_back = 0;
        }
    }
}

struct Label(String);

#[derive(Default)]
struct Fields {
    message: String,
    name: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_owned(),
            "name" => self.name = value.to_owned(),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "name" => self.name = format!("{value:?}"),
            _ => {}
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Report {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            let label = format!("{} `{}`", attrs.metadata().name(), fields.name);
            span.extensions_mut().insert(Label(label));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > Level::WARN {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let scope: Vec<String> = ctx
            .event_scope(event)
            .into_iter()
            .flat_map(|scope| scope.from_root())
            .filter_map(|span| span.extensions().get::<Label>().map(|l| l.0.clone()))
            .collect();
        let kind = if level == Level::ERROR {
            "error"
        } else {
            "warning"
        };
        let line = match scope.is_empty() {
            true => format!("{kind}: {}", fields.message),
            false => format!("{kind}: {}: {}", scope.join(", "), fields.message),
        };
        let Ok(mut printed) = PRINTED.lock() else {
            return;
        };
        if printed.lines.insert(line.clone()) {
            let (shown, held_back) = printed.messages.entry(fields.message).or_default();
            if *shown < SHOWN_PER_MESSAGE {
                *shown += 1;
                eprintln!("{line}");
            } else {
                *held_back += 1;
            }
        }
    }
}

/// YAML read as JSON, with integers beyond 64 bits as floats and scalar keys as strings.
mod yaml {
    use std::fmt;

    use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
    use serde_json::{Map, Number, Value};

    pub(super) struct Json(pub Value);

    struct Key(String);

    impl<'de> Deserialize<'de> for Json {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(JsonVisitor).map(Json)
        }
    }

    impl<'de> Deserialize<'de> for Key {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            match deserializer.deserialize_any(JsonVisitor)? {
                Value::String(key) => Ok(Key(key)),
                Value::Null => Ok(Key("null".into())),
                key @ (Value::Bool(_) | Value::Number(_)) => Ok(Key(key.to_string())),
                _ => Err(D::Error::custom("mapping keys must be scalars")),
            }
        }
    }

    struct JsonVisitor;

    fn float<E: Error>(value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("numbers must be finite"))
    }

    impl<'de> Visitor<'de> for JsonVisitor {
        type Value = Value;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a YAML value")
        }

        fn visit_bool<E: Error>(self, v: bool) -> Result<Value, E> {
            Ok(Value::Bool(v))
        }

        fn visit_i64<E: Error>(self, v: i64) -> Result<Value, E> {
            Ok(v.into())
        }

        fn visit_u64<E: Error>(self, v: u64) -> Result<Value, E> {
            Ok(v.into())
        }

        fn visit_i128<E: Error>(self, v: i128) -> Result<Value, E> {
            i64::try_from(v).map_or_else(|_| float(v as f64), |v| Ok(v.into()))
        }

        fn visit_u128<E: Error>(self, v: u128) -> Result<Value, E> {
            u64::try_from(v).map_or_else(|_| float(v as f64), |v| Ok(v.into()))
        }

        fn visit_f64<E: Error>(self, v: f64) -> Result<Value, E> {
            float(v)
        }

        fn visit_str<E: Error>(self, v: &str) -> Result<Value, E> {
            Ok(Value::String(v.to_owned()))
        }

        fn visit_string<E: Error>(self, v: String) -> Result<Value, E> {
            Ok(Value::String(v))
        }

        fn visit_unit<E: Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_none<E: Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
            Json::deserialize(d).map(|json| json.0)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
            let mut items = Vec::new();
            while let Some(Json(item)) = seq.next_element()? {
                items.push(item);
            }
            Ok(Value::Array(items))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            let mut object = Map::new();
            while let Some((Key(key), Json(value))) = map.next_entry()? {
                object.insert(key, value);
            }
            Ok(Value::Object(object))
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn big_integers_become_floats_and_keys_strings() {
            let json: super::Json =
                serde_norway::from_str("max: 18446744073709552000\n200: ok\nsmall: -3\n").unwrap();
            assert_eq!(
                json.0,
                serde_json::json!({"max": 18446744073709552000.0, "200": "ok", "small": -3})
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    #[test]
    fn specs_over_ten_megabytes_download() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/spec.json", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            let padding = "a".repeat(11 * 1024 * 1024);
            let body = format!(
                r#"{{"openapi":"3.1.0","info":{{"title":"t","version":"1"}},"x-pad":"{padding}"}}"#
            );
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        });
        let text = super::read(&url, std::path::Path::new(".")).unwrap();
        assert!(text.len() > 11 * 1024 * 1024);
    }
}
