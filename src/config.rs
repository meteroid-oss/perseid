use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use heck::{ToKebabCase, ToSnakeCase, ToUpperCamelCase};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::spec::{Filters, IncludeMode};

pub const FILE: &str = "perseid.toml";
pub const LANGUAGES: [&str; 6] = ["rust", "typescript", "python", "go", "java", "csharp"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Path (relative to perseid.toml) or http(s) URL of the OpenAPI document.
    pub spec: String,
    /// Client name: `Acme` gives the `Acme` client and the `acme` package.
    pub name: String,
    pub base_url: Option<String>,
    pub version: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    pub patch_nullable: Option<bool>,
    /// Installs the Standard Webhooks signature verifier in every SDK.
    pub webhooks: Option<bool>,
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
    /// Directory mirroring `templates/<lang>/…` and `runtime/<lang>/…` to override built-ins.
    /// Defaults to `.perseid`.
    pub overrides: Option<String>,
    #[serde(default)]
    pub include: IncludeMode,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub only: Vec<String>,
    /// Paginated list operations, detected from their query parameter and response shape.
    #[serde(default, deserialize_with = "one_or_many")]
    pub pagination: Vec<Pagination>,
    pub rust: Option<Target>,
    pub typescript: Option<Target>,
    pub python: Option<Target>,
    pub go: Option<Target>,
    pub java: Option<Target>,
    pub csharp: Option<Target>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Output directory, relative to the repository the SDK lives in.
    pub path: Option<String>,
    /// `owner/name` of a GitHub repository to generate into instead of this one.
    pub repo: Option<String>,
    /// Crate, npm, PyPI or Go package name, Java package or C# root namespace.
    pub package: Option<String>,
    /// Go module path.
    pub module: Option<String>,
    /// TypeScript modules re-exported from the entry point.
    #[serde(default)]
    pub exports: Vec<String>,
    pub base_url: Option<String>,
    pub version: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    pub patch_nullable: Option<bool>,
    pub webhooks: Option<bool>,
    /// TypeScript type of int64 values: `number` (the default), `bigint` or `string`.
    pub int64: Option<String>,
    /// Go: spell initialisms the Go way (`CustomerID`, not `CustomerId`).
    pub initialisms: Option<bool>,
    /// Operation ids left out of this SDK only, on top of the top-level `exclude`.
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
}

/// How a list operation pages, as `x-pagination` on an operation or `[pagination]` in perseid.toml.
/// Exactly one of `cursor`, `page` or `offset` names the query parameter selecting the page;
/// response values are dotted paths of JSON property names.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    pub cursor: Option<String>,
    pub page: Option<String>,
    pub offset: Option<String>,
    /// The array of items, `data` by default.
    pub items: Option<String>,
    /// Cursor of the next page in the response.
    pub next_cursor: Option<String>,
    /// Field of the last item that is the next cursor, e.g. `id` for `starting_after`.
    pub item_cursor: Option<String>,
    /// Boolean telling whether more pages follow.
    pub has_more: Option<String>,
    pub total_pages: Option<String>,
    /// Total number of items, for offset pagination.
    pub total: Option<String>,
    /// Number of the first page, 1 by default.
    pub first_page: Option<i64>,
    /// perseid.toml only: operations this rule must apply to, instead of every matching one.
    #[serde(default)]
    pub operations: Vec<String>,
}

fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Pagination>, D::Error> {
    use serde::de::Error;
    match toml::Value::deserialize(d)? {
        toml::Value::Array(many) => many
            .into_iter()
            .map(|one| one.try_into().map_err(D::Error::custom))
            .collect(),
        one => Ok(vec![one.try_into().map_err(D::Error::custom)?]),
    }
}

pub struct Sdk<'a> {
    pub language: &'static str,
    pub repo: Option<&'a str>,
    pub path: String,
    target: &'a Target,
}

impl Config {
    pub fn load(path: &Path) -> Result<(Self, PathBuf)> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "reading {} (run `perseid init` to create one)",
                path.display()
            )
        })?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        ensure!(
            config.name == config.name.to_upper_camel_case(),
            "`name` must be UpperCamelCase, e.g. \"{}\"",
            config.name.to_upper_camel_case()
        );
        let others = [&config.rust, &config.python, &config.go, &config.java];
        ensure!(
            others
                .iter()
                .all(|t| t.as_ref().is_none_or(|t| t.int64.is_none())),
            "`int64` is only supported in [typescript]"
        );
        if let Some(int64) = config.typescript.as_ref().and_then(|t| t.int64.as_deref()) {
            ensure!(
                ["number", "bigint", "string"].contains(&int64),
                "`int64` must be \"number\", \"bigint\" or \"string\", not {int64:?}"
            );
        }
        let root = std::path::absolute(path)?.parent().unwrap().to_owned();
        Ok((config, root))
    }

    pub fn overrides_dir(&self, root: &Path) -> PathBuf {
        root.join(self.overrides.as_deref().unwrap_or(".perseid"))
    }

    /// The filters of one SDK: the shared ones, plus the operations its target excludes.
    pub fn filters_for(&self, sdk: &Sdk) -> Filters {
        let mut filters = self.filters();
        filters.excluded.extend(sdk.target.exclude.iter().cloned());
        filters.reserved = reserved_type_names(sdk.language, &self.name);
        filters
    }

    pub fn filters(&self) -> Filters {
        Filters {
            include_mode: if self.only.is_empty() {
                self.include
            } else {
                IncludeMode::OnlySpecified
            },
            excluded: self.exclude.iter().cloned().collect(),
            specified: self.only.iter().cloned().collect(),
            pagination: self.pagination.clone(),
            reserved: BTreeSet::new(),
        }
    }

    pub fn sdks(&self, selected: &[String]) -> Result<Vec<Sdk<'_>>> {
        for name in selected {
            ensure!(
                LANGUAGES.contains(&name.as_str()),
                "unknown language `{name}`"
            );
        }
        let targets = [
            &self.rust,
            &self.typescript,
            &self.python,
            &self.go,
            &self.java,
            &self.csharp,
        ];
        let sdks = LANGUAGES
            .into_iter()
            .zip(targets)
            .filter_map(|(language, target)| Some((language, target.as_ref()?)))
            .filter(|(language, _)| selected.is_empty() || selected.iter().any(|s| s == language))
            .map(|(language, target)| Sdk {
                language,
                repo: target.repo.as_deref(),
                path: target
                    .path
                    .clone()
                    .unwrap_or_else(|| if target.repo.is_some() { "." } else { language }.into()),
                target,
            })
            .collect::<Vec<_>>();
        ensure!(
            !sdks.is_empty(),
            "nothing to generate: add a [rust], [typescript], [python], [go], [java] or [csharp] table"
        );
        Ok(sdks)
    }

    /// Values exposed to templates as `sdk`, for an SDK checked out at `dir`.
    pub fn context(&self, sdk: &Sdk, dir: &Path) -> Value {
        let (language, target) = (sdk.language, sdk.target);
        let snake = self.name.to_snake_case();
        let kebab = self.name.to_kebab_case();
        let package = target.package.clone().unwrap_or_else(|| match language {
            "java" => format!("com.{}", snake.replace('_', "")),
            "typescript" => kebab.clone(),
            "csharp" => self.name.clone(),
            _ => snake.clone(),
        });
        let version = target.version.clone().or_else(|| self.version.clone());
        let version = version
            .or_else(|| manifest_version(dir))
            .unwrap_or_else(|| "0.1.0".into());
        let pick = |own: &Option<String>, shared: &Option<String>, default: &str| {
            own.clone()
                .or_else(|| shared.clone())
                .unwrap_or_else(|| default.into())
        };
        let mut context = json!({
            "client_name": self.name,
            "package_name": if language == "typescript" { &snake } else { &package },
            "rust_crate": if language == "rust" { package.replace('-', "_") } else { snake.clone() },
            "java_package": if language == "java" { package.clone() } else { format!("com.{snake}") },
            "npm_package": if language == "typescript" { &package } else { &kebab },
            "go_module": target.module,
            "default_base_url": pick(&target.base_url, &self.base_url, "http://localhost"),
            "user_agent_prefix": pick(&target.user_agent, &self.user_agent, &kebab),
            "header_prefix": pick(&target.header_prefix, &self.header_prefix, &kebab),
            "patch_nullable": target.patch_nullable.or(self.patch_nullable).unwrap_or(false),
            "webhooks": target.webhooks.or(self.webhooks).unwrap_or(false),
            "int64": target.int64.as_deref().unwrap_or("number"),
            "go_initialisms": target.initialisms.unwrap_or(false),
            "version": version,
            "extra_exports": target.exports,
        });
        let map = context.as_object_mut().unwrap();
        if language == "rust" {
            map.insert("chrono".into(), rust_depends_on(dir, "chrono").into());
        }
        map.extend(self.context.clone());
        map.extend(target.context.clone());
        context
    }
}

/// Rust dates are `chrono` types when the crate depends on it (or has no manifest yet), so
/// SDKs predating it keep their string dates.
fn rust_depends_on(dir: &Path, dependency: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
        return true;
    };
    toml::from_str::<toml::Value>(&text)
        .ok()
        .and_then(|manifest| manifest.get("dependencies")?.get(dependency).cloned())
        .is_some()
}

/// The SDK's own package manifest owns its version, so release tooling keeps working.
pub(crate) fn manifest_version(dir: &Path) -> Option<String> {
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).ok();
    if let Some(json) = read("package.json") {
        let value: Value = serde_json::from_str(&json).ok()?;
        return value["version"].as_str().map(str::to_owned);
    }
    for (file, table) in [("Cargo.toml", "package"), ("pyproject.toml", "project")] {
        if let Some(text) = read(file) {
            let value: toml::Value = toml::from_str(&text).ok()?;
            return value
                .get(table)?
                .get("version")?
                .as_str()
                .map(str::to_owned);
        }
    }
    if let Some(properties) = read("gradle.properties") {
        let version = properties.lines().find_map(|l| {
            let (key, value) = l.split_once('=')?;
            ["VERSION_NAME", "version"]
                .contains(&key.trim())
                .then(|| value.trim().to_owned())
        });
        if version.is_some() {
            return version;
        }
    }
    if let Some(version) = csproj_version(dir) {
        return Some(version);
    }
    if let Some(go) = read("version.go") {
        return go.lines().find_map(|l| {
            let value = l.trim().strip_prefix("const Version = ")?;
            Some(value.split('"').nth(1)?.to_owned())
        });
    }
    read("version.txt").map(|v| v.trim().to_owned())
}

/// `<Version>` of the first `.csproj` in `dir` or one level below, as `perseid init csharp` lays out.
fn csproj_version(dir: &Path) -> Option<String> {
    let entries = |dir: &Path| {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| Some(e.ok()?.path()))
            .collect();
        paths.sort();
        paths
    };
    let top = entries(dir);
    let nested = top.iter().filter(|p| p.is_dir()).flat_map(|d| entries(d));
    top.iter()
        .cloned()
        .chain(nested)
        .filter(|p| p.extension().is_some_and(|e| e == "csproj"))
        .find_map(|project| {
            let text = std::fs::read_to_string(project).ok()?;
            let start = text.find("<Version>")? + "<Version>".len();
            let end = start + text[start..].find("</Version>")?;
            Some(text[start..end].trim().to_owned())
        })
}

/// Type names that generated code uses unqualified next to the models, per language.
fn reserved_type_names(language: &str, client: &str) -> BTreeSet<String> {
    let names: &[&str] = match language {
        "rust" => &["Box", "Option", "Result", "String", "Vec"],
        "typescript" => &[
            "Array",
            "Blob",
            "Date",
            "EventStream",
            "Headers",
            "Map",
            "Middleware",
            "MultipartBody",
            "Promise",
            "Record",
            "RequestOptions",
            "Security",
            "SecurityScheme",
            "Set",
            "Uint8Array",
            "Upload",
            "UploadBody",
            "XOR",
        ],
        // Go declares everything in one directory, where file names count too.
        "go" => &[
            "BasicAuth",
            "Client",
            "Collections",
            "Errors",
            "EventStream",
            "Middleware",
            "Nullable",
            "Options",
            "Pager",
            "Request",
            "RequestAuth",
            "RequestOption",
            "RequestPager",
            "RequestStreaming",
            "RequiredMap",
            "RequiredSlice",
            "RoundTripperFunc",
            "SseEvent",
            "Upload",
            "Version",
            "Webhooks",
        ],
        "java" => &[
            "ApiException",
            "ArrayList",
            "BigDecimal",
            "Boolean",
            "Double",
            "EventStream",
            "Float",
            "HashMap",
            "Headers",
            "HttpUrl",
            "IOException",
            "Integer",
            "LinkedHashSet",
            "List",
            "Long",
            "Map",
            "Multipart",
            "Object",
            "Objects",
            "OffsetDateTime",
            "Optional",
            "Override",
            "Paginator",
            "Set",
            "String",
            "TypeReference",
            "URI",
            "Upload",
            "Utils",
            "Void",
        ],
        // Ours, and those of the namespaces resources import, which would become ambiguous.
        "csharp" => &[
            "Action",
            "ApiAuth",
            "ApiException",
            "ApiRequest",
            "ApiTransport",
            "Array",
            "Attribute",
            "Barrier",
            "BasicCredentials",
            "Buffer",
            "CancellationToken",
            "Comparer",
            "Console",
            "Convert",
            "Credentials",
            "DateTime",
            "DateTimeOffset",
            "Default",
            "Delegate",
            "Dictionary",
            "Enum",
            "Environment",
            "EventStream",
            "Exception",
            "Func",
            "Guid",
            "HashSet",
            "HttpClient",
            "HttpContent",
            "HttpMethod",
            "HttpRequestMessage",
            "HttpResponseMessage",
            "Index",
            "Interlocked",
            "Json",
            "JsonArray",
            "JsonNode",
            "JsonObject",
            "JsonValue",
            "KeyValuePair",
            "Lazy",
            "LinkedList",
            "List",
            "Lock",
            "Math",
            "MaybeUnset",
            "Monitor",
            "MultipartBody",
            "Mutex",
            "Nullable",
            "Object",
            "Options",
            "Pagination",
            "Paginator",
            "Parallel",
            "Queue",
            "Random",
            "Range",
            "RequestOptions",
            "SecurityScheme",
            "Semaphore",
            "SortedSet",
            "SseEvent",
            "Stack",
            "String",
            "StringContent",
            "Task",
            "Thread",
            "TimeSpan",
            "Timer",
            "Tuple",
            "Type",
            "Unrecognized",
            "Upload",
            "Uri",
            "ValueTask",
            "Version",
            "Volatile",
            "Webhook",
        ],
        _ => &[],
    };
    let own: &[&str] = match language {
        "typescript" => &["Request", "RequestContext"],
        "java" => &["HttpClient", "Options"],
        "csharp" => &["Client", "ClientOptions", "JsonContext"],
        "go" => &[""],
        _ => &[],
    };
    names
        .iter()
        .map(|n| (*n).to_owned())
        .chain(own.iter().map(|suffix| format!("{client}{suffix}")))
        .collect()
}
