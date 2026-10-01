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
    /// Path (relative to perseid.toml), http(s) URL, or `github:owner/repo/path` of a spec another
    /// repository pushes here.
    pub spec: String,
    /// Default repository of every SDK: `owner/name-{lang}` gives each its own, `owner/name` holds
    /// them all in folders named after their language.
    pub repo: Option<String>,
    /// `owner/name` of a repository receiving the spec, which generates the SDKs itself.
    pub push_spec: Option<String>,
    /// Command producing the spec in the API repository's CI, when it isn't committed.
    pub generate: Option<String>,
    /// Client name: `Acme` gives the `Acme` client and the `acme` package.
    pub name: String,
    pub base_url: Option<String>,
    pub version: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    /// Installs the Standard Webhooks signature verifier in every SDK.
    pub webhooks: Option<bool>,
    /// Method names by operation id, over the resource-style names.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// Default request timeout, in seconds.
    pub timeout: Option<u64>,
    /// How unions decode objects that no rule tells apart.
    pub untagged_unions: Option<UntaggedUnions>,
    /// SPDX license expression of the packages.
    pub license: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
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
    pub webhooks: Option<bool>,
    /// TypeScript type of int64 values: `number` (the default), `bigint` or `string`.
    pub int64: Option<String>,
    /// Method names by operation id, over the top-level `[names]`.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    pub timeout: Option<u64>,
    pub untagged_unions: Option<UntaggedUnions>,
    /// Operation ids left out of this SDK only, on top of the top-level `exclude`.
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
}

/// How a union decodes an object when several variants are objects that neither a
/// discriminator nor the constants and required properties they declare tell apart.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UntaggedUnions {
    /// Untyped JSON.
    #[default]
    Json,
    /// The variant whose required properties are all present and which knows the most
    /// properties, the first declared on ties.
    BestMatch,
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
    pub repo: Option<String>,
    pub path: String,
    target: &'a Target,
}

/// Where the spec comes from, as `spec` says.
#[derive(Debug, PartialEq, Eq)]
pub enum Source<'a> {
    File(&'a str),
    Url(&'a str),
    /// A file of another GitHub repository, pushed here as a snapshot next to perseid.toml.
    GitHub {
        repo: String,
        path: &'a str,
    },
}

impl Source<'_> {
    pub fn parse(spec: &str) -> Result<Source<'_>> {
        if spec.starts_with("http://") || spec.starts_with("https://") {
            return Ok(Source::Url(spec));
        }
        let Some(rest) = spec.strip_prefix("github:") else {
            return Ok(Source::File(spec));
        };
        let mut parts = rest.splitn(3, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(name), Some(path))
                if !owner.is_empty() && !name.is_empty() && !path.is_empty() =>
            {
                Ok(Source::GitHub {
                    repo: format!("{owner}/{name}"),
                    path,
                })
            }
            _ => anyhow::bail!(
                "`spec = \"{spec}\"` must read github:owner/repo/path/to/openapi.json"
            ),
        }
    }
}

/// The file holding the spec another repository pushes, named after it: `openapi.yaml`, …
pub fn snapshot(path: &str) -> String {
    let extension = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| ["json", "yaml", "yml"].contains(e))
        .unwrap_or("json");
    format!("openapi.{extension}")
}

/// The conventional repository suffix of a language's SDK, `{lang}` in `repo`.
pub fn suffix(language: &str) -> &str {
    match language {
        "typescript" => "node",
        "csharp" => "dotnet",
        other => other,
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<(Self, PathBuf)> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "reading {} (run `perseid init` to create one)",
                path.display()
            )
        })?;
        let table: toml::Table =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        removed_keys(&table).with_context(|| format!("in {}", path.display()))?;
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
        let source = Source::parse(&config.spec)?;
        ensure!(
            config.push_spec.is_none() || matches!(source, Source::File(_)),
            "`push_spec` sends a spec file of this repository: `spec` must be its path"
        );
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
        filters.names.extend(sdk.target.names.clone());
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
            names: self.names.clone(),
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
        let all: Vec<_> = LANGUAGES
            .into_iter()
            .zip(targets)
            .filter_map(|(language, target)| Some((language, target.as_ref()?)))
            .map(|(language, target)| {
                let repo = target.repo.clone().or_else(|| {
                    let repo = self.repo.as_deref()?;
                    Some(repo.replace("{lang}", suffix(language)))
                });
                (language, target, repo)
            })
            .collect();
        let shared = |repo: &str| all.iter().filter(|a| a.2.as_deref() == Some(repo)).count() > 1;
        let sdks = all
            .iter()
            .filter(|(language, ..)| selected.is_empty() || selected.iter().any(|s| s == language))
            .map(|(language, target, repo)| Sdk {
                language,
                path: target.path.clone().unwrap_or_else(|| match repo {
                    Some(repo) if !shared(repo) => ".".into(),
                    _ => (*language).into(),
                }),
                repo: repo.clone(),
                target,
            })
            .collect::<Vec<_>>();
        ensure!(
            !sdks.is_empty(),
            "nothing to generate: add a [rust], [typescript], [python], [go], [java] or [csharp] table"
        );
        Ok(sdks)
    }

    pub fn source(&self) -> Source<'_> {
        Source::parse(&self.spec).unwrap_or(Source::File(&self.spec))
    }

    /// The Go module path of the repository and folder the SDK lives in.
    fn go_module(&self, sdk: &Sdk) -> String {
        let repo = sdk.repo.clone().or_else(|| {
            let url = self.repository.as_deref()?;
            Some(url.strip_prefix("https://github.com/")?.to_owned())
        });
        match (repo, sdk.path.as_str()) {
            (Some(repo), ".") => format!("github.com/{repo}"),
            (Some(repo), path) => format!("github.com/{repo}/{path}"),
            (None, _) => {
                let name = self.name.to_kebab_case();
                format!("github.com/{name}/{name}-go")
            }
        }
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
            "go_module": target.module.clone().unwrap_or_else(|| self.go_module(sdk)),
            "default_base_url": pick(&target.base_url, &self.base_url, "http://localhost"),
            "user_agent_prefix": pick(&target.user_agent, &self.user_agent, &kebab),
            "header_prefix": pick(&target.header_prefix, &self.header_prefix, &kebab),
            "webhooks": target.webhooks.or(self.webhooks).unwrap_or(false),
            "int64": target.int64.as_deref().unwrap_or("number"),
            "version": version,
            "extra_exports": target.exports,
            "timeout": target.timeout.or(self.timeout).unwrap_or(60),
            "untagged_unions": match target.untagged_unions.or(self.untagged_unions).unwrap_or_default() {
                UntaggedUnions::Json => "json",
                UntaggedUnions::BestMatch => "best-match",
            },
            "license": self.license,
            "repository": self.repository,
            "homepage": self.homepage,
            "description": self.description.clone().unwrap_or_else(|| format!("{} API client", self.name)),
            "authors": self.authors,
        });
        let map = context.as_object_mut().unwrap();
        if language == "java" {
            map.insert(
                "java_internal_package".into(),
                format!("{package}.internal").into(),
            );
        }
        map.extend(self.context.clone());
        map.extend(target.context.clone());
        context
    }
}

/// Keys of earlier versions, whose behaviour is now the only one.
const REMOVED: [(&str, &str); 5] = [
    (
        "method_names",
        "methods are named after their resource; `[names]` renames one",
    ),
    ("typed_unions", "unions are always typed"),
    (
        "patch_nullable",
        "nullable optional PATCH fields can always be cleared",
    ),
    (
        "initialisms",
        "Go names always spell initialisms the Go way",
    ),
    ("edition", "Java SDKs always have edition 2's API"),
];

fn removed_keys(table: &toml::Table) -> Result<()> {
    let tables = std::iter::once(("", table)).chain(
        LANGUAGES
            .into_iter()
            .filter_map(|l| Some((l, table.get(l)?.as_table()?))),
    );
    for (language, table) in tables {
        for (key, why) in REMOVED {
            if table.contains_key(key) {
                let at = match language {
                    "" => String::new(),
                    language => format!("[{language}] "),
                };
                anyhow::bail!("{at}`{key}` was removed ({why}): delete it");
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn context(toml: &str, language: &str) -> Value {
        let config: Config = toml::from_str(toml).unwrap();
        let sdk = config.sdks(&[language.to_owned()]).unwrap().remove(0);
        config.context(&sdk, Path::new("/nonexistent"))
    }

    #[test]
    fn timeout_and_package_metadata_reach_templates() {
        let toml = "spec = \"s\"\nname = \"Acme\"\ntimeout = 15\nlicense = \"MIT\"\n\
                    authors = [\"A <a@x.dev>\"]\n[go]\ntimeout = 30\n[rust]\n";
        let go = context(toml, "go");
        assert_eq!(
            (go["timeout"].clone(), go["license"].clone()),
            (json!(30), json!("MIT"))
        );
        let rust = context(toml, "rust");
        assert_eq!(rust["timeout"], 15);
        assert_eq!(rust["authors"], json!(["A <a@x.dev>"]));
        assert_eq!(rust["description"], "Acme API client");
        assert_eq!(
            context("spec = \"s\"\nname = \"Acme\"\n[rust]\n", "rust")["timeout"],
            60
        );
    }

    fn layout(toml: &str) -> Vec<(String, Option<String>, String)> {
        let config: Config = toml::from_str(toml).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        sdks.iter()
            .map(|s| (s.language.to_owned(), s.repo.clone(), s.path.clone()))
            .collect()
    }

    #[test]
    fn a_repo_pattern_gives_each_sdk_its_own_repository() {
        let toml =
            "spec = \"s\"\nname = \"Acme\"\nrepo = \"acme/api-{lang}\"\n[typescript]\n[csharp]\n";
        let repo = |r: &str| Some(r.to_owned());
        assert_eq!(
            layout(toml),
            [
                ("typescript".into(), repo("acme/api-node"), ".".into()),
                ("csharp".into(), repo("acme/api-dotnet"), ".".into()),
            ]
        );
        let config: Config = toml::from_str(&format!("{toml}[go]\n")).unwrap();
        let go = config.sdks(&["go".into()]).unwrap().remove(0);
        let module = config.context(&go, Path::new("/nonexistent"))["go_module"].clone();
        assert_eq!(module, "github.com/acme/api-go");
    }

    #[test]
    fn a_shared_repo_holds_each_sdk_in_a_folder_unless_overridden() {
        let toml = "spec = \"s\"\nname = \"Acme\"\nrepo = \"acme/sdks\"\n[python]\n[go]\n\
                    [typescript]\nrepo = \"acme/js\"\n[rust]\npath = \"crates/acme\"\n";
        let repo = |r: &str| Some(r.to_owned());
        assert_eq!(
            layout(toml),
            [
                ("rust".into(), repo("acme/sdks"), "crates/acme".into()),
                ("typescript".into(), repo("acme/js"), ".".into()),
                ("python".into(), repo("acme/sdks"), "python".into()),
                ("go".into(), repo("acme/sdks"), "go".into()),
            ]
        );
        let config: Config = toml::from_str(toml).unwrap();
        let go = config.sdks(&["go".into()]).unwrap().remove(0);
        let module = config.context(&go, Path::new("/nonexistent"))["go_module"].clone();
        assert_eq!(module, "github.com/acme/sdks/go");
        assert_eq!(
            layout("spec = \"s\"\nname = \"Acme\"\n[rust]\n"),
            [("rust".into(), None, "rust".into())]
        );
    }

    #[test]
    fn specs_come_from_a_file_a_url_or_another_repository() {
        assert_eq!(
            Source::parse("api/openapi.yaml").unwrap(),
            Source::File("api/openapi.yaml")
        );
        assert_eq!(
            Source::parse("github:acme/api/spec/openapi.yaml").unwrap(),
            Source::GitHub {
                repo: "acme/api".into(),
                path: "spec/openapi.yaml"
            }
        );
        assert!(Source::parse("github:acme/api").is_err());
        assert_eq!(snapshot("spec/openapi.yaml"), "openapi.yaml");
        assert_eq!(snapshot("swagger"), "openapi.json");
    }

    #[test]
    fn name_overrides_are_per_language() {
        let toml = "spec = \"s\"\nname = \"Acme\"\n\
                    [names]\na = \"x\"\n[go]\nnames = { a = \"y\" }\n[rust]\n";
        let config: Config = toml::from_str(toml).unwrap();
        let sdks = config.sdks(&[]).unwrap();
        assert_eq!(config.filters_for(&sdks[0]).names["a"], "x");
        assert_eq!(config.filters_for(&sdks[1]).names["a"], "y");
    }

    #[test]
    fn removed_keys_say_what_to_do() {
        let error = |toml: &str| {
            removed_keys(&toml.parse().unwrap())
                .unwrap_err()
                .to_string()
        };
        assert!(error("method_names = \"resource\"").contains("`method_names` was removed"));
        let java = error("[java]\nedition = 2");
        assert!(
            java.contains("[java] `edition`") && java.contains("delete it"),
            "{java}"
        );
        assert!(removed_keys(&"[go]\nmodule = \"m\"".parse().unwrap()).is_ok());
    }
}
