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
}

/// Reads an OpenAPI document (JSON or YAML) from a path or an http(s) URL, as JSON text, upgraded to 3.1 when it is 3.0.
pub(crate) fn read(location: &str, root: &Path) -> Result<String> {
    let text = if location.starts_with("https://") || location.starts_with("http://") {
        ureq::get(location)
            .call()
            .and_then(|mut r| r.body_mut().read_to_string())
            .with_context(|| format!("downloading {location}"))?
    } else {
        std::fs::read_to_string(root.join(location))
            .with_context(|| format!("reading {location}"))?
    };
    let mut value: Value = if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).map_err(anyhow::Error::from)
    } else {
        serde_norway::from_str(&text).map_err(anyhow::Error::from)
    }
    .with_context(|| format!("parsing {location}"))?;
    upgrade::to_3_1(&mut value).with_context(|| location.to_owned())?;
    Ok(serde_json::to_string(&value)?)
}

pub(crate) fn api(spec: &str, filters: &Filters) -> Result<Api> {
    let mut doc: Value = serde_json::from_str(spec).context("the spec is not valid JSON")?;
    normalize::normalize(&mut doc)?;
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
