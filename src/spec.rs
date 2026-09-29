use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use aide::openapi::OpenApi;
use anyhow::{Context as _, Result, bail};
use schemars::schema::Schema;
use serde_json::Value;
use tracing::{Event, Level};
use tracing_subscriber::layer::{Context, Layer};

use crate::api::Api;

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
    let mut spec: OpenApi =
        serde_json::from_str(spec).context("the spec is not a valid OpenAPI 3 document")?;
    let webhooks = webhooks(&spec);
    let Some(paths) = spec.paths.take() else {
        bail!("the spec has no paths");
    };
    Api::new(
        paths,
        &mut spec.components.take().unwrap_or_default(),
        &webhooks,
        filters.include_mode,
        &filters.excluded,
        &filters.specified,
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
