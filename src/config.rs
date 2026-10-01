use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use heck::{ToKebabCase, ToSnakeCase, ToUpperCamelCase};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::spec::{Filters, IncludeMode};

pub const FILE: &str = "perseid.toml";
pub const LANGUAGES: [&str; 6] = ["rust", "typescript", "python", "go", "java", "csharp"];
pub const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/meteroid-oss/perseid/main/perseid.schema.json";

/// perseid.toml: where the OpenAPI spec comes from, and the SDKs generated from it.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "perseid.toml")]
pub struct Config {
    /// The OpenAPI document (3.0 or 3.1, JSON or YAML): a path relative to perseid.toml, an
    /// http(s) URL, or `github:owner/repo/path` for a spec another repository pushes here.
    pub spec: String,
    /// Client name, in any form (`Acme`, `acme-api`, `Acme API`): the `AcmeApi` client, the
    /// `acme_api` and `acme-api` packages.
    #[serde(deserialize_with = "client_name")]
    #[schemars(with = "String")]
    pub name: String,
    /// Default repository of every SDK: `owner/name-{lang}` gives each its own (`{lang}` is
    /// `rust`, `node`, `python`, `go`, `java` or `dotnet`), `owner/name` holds them all in folders
    /// named after their language. Without it, the SDKs live next to perseid.toml.
    pub repo: Option<String>,
    /// `owner/name` of a separate repository this one sends its spec to, which generates the
    /// SDKs from its own perseid.toml.
    pub sdks_repo: Option<String>,
    /// When the spec is pushed to the repository generating the SDKs (with `sdks_repo`, or a
    /// `github:` spec).
    pub push_on: Option<PushOn>,
    /// Tags pushing the spec with `push_on = "tag"`, as a GitHub Actions glob: `v*` by default.
    pub push_tags: Option<String>,
    /// Command writing the spec in the API repository's CI when it isn't committed, run from its
    /// root before pushing the spec.
    pub generate: Option<String>,
    /// Package metadata written into the manifests `perseid init` creates.
    #[serde(default)]
    pub package: Package,
    /// API base URL the clients default to.
    pub base_url: Option<String>,
    /// Prefix of the SDK's own headers, such as `{prefix}-idempotency-key`: the kebab-case
    /// `name` by default.
    pub header_prefix: Option<String>,
    /// Prefix of the `User-Agent` header: the kebab-case `name` by default.
    pub user_agent: Option<String>,
    /// Installs the Standard Webhooks signature verifier in every SDK.
    pub webhooks: Option<bool>,
    /// Default request timeout, in seconds: 60 by default.
    pub timeout: Option<u64>,
    /// How unions decode objects that no rule tells apart.
    pub untagged_unions: Option<UntaggedUnions>,
    /// Method names by operation id, over the resource-style names.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// Also generates the operations marked `x-internal: true`.
    #[serde(default)]
    pub internal: bool,
    /// Operation ids to generate, leaving out every other operation.
    #[serde(default)]
    pub only: Vec<String>,
    /// Operation ids left out of every SDK.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Paginated list operations, detected from their query parameter and response shape.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany")]
    pub pagination: Vec<Pagination>,
    /// Directory mirroring `templates/<lang>/…` and `runtime/<lang>/…` to override built-ins:
    /// `.perseid` by default.
    pub overrides: Option<String>,
    /// Values exposed to templates as `sdk.*`.
    #[serde(default)]
    pub context: BTreeMap<String, Value>,
    /// The Rust SDK, a crate.
    #[serde(default, deserialize_with = "target::<Rust, _>")]
    #[schemars(with = "Option<Rust>")]
    pub rust: Option<Target>,
    /// The TypeScript SDK, an npm package.
    #[serde(default, deserialize_with = "target::<TypeScript, _>")]
    #[schemars(with = "Option<TypeScript>")]
    pub typescript: Option<Target>,
    /// The Python SDK, a PyPI package.
    #[serde(default, deserialize_with = "target::<Python, _>")]
    #[schemars(with = "Option<Python>")]
    pub python: Option<Target>,
    /// The Go SDK, a module.
    #[serde(default, deserialize_with = "target::<Go, _>")]
    #[schemars(with = "Option<Go>")]
    pub go: Option<Target>,
    /// The Java SDK, a Maven package.
    #[serde(default, deserialize_with = "target::<Java, _>")]
    #[schemars(with = "Option<Java>")]
    pub java: Option<Target>,
    /// The C# SDK, a NuGet package.
    #[serde(default, deserialize_with = "target::<CSharp, _>")]
    #[schemars(with = "Option<CSharp>")]
    pub csharp: Option<Target>,
}

/// Package metadata written into the manifests `perseid init` creates.
#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// One-line summary: `{name} API client` by default.
    pub description: Option<String>,
    /// SPDX license expression.
    pub license: Option<String>,
    /// Project website.
    pub homepage: Option<String>,
    /// URL of the source repository.
    pub repository: Option<String>,
    /// `Name <email>` of each author.
    #[serde(default)]
    pub authors: Vec<String>,
}

/// When the API repository pushes its spec.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PushOn {
    /// Every push to the default branch changing the spec.
    #[default]
    Change,
    /// Every published GitHub release, with the spec of its tag.
    Release,
    /// Every tag matching `push_tags`.
    Tag,
}

/// One SDK, as its language table sets it up; the shared settings default to the top-level ones.
#[derive(Default)]
pub struct Target {
    pub path: Option<String>,
    pub repo: Option<String>,
    pub package: Option<String>,
    pub base_url: Option<String>,
    pub header_prefix: Option<String>,
    pub user_agent: Option<String>,
    pub webhooks: Option<bool>,
    pub timeout: Option<u64>,
    pub untagged_unions: Option<UntaggedUnions>,
    pub names: BTreeMap<String, String>,
    pub exclude: Vec<String>,
    pub context: BTreeMap<String, Value>,
    pub module: Option<String>,
    pub exports: Vec<String>,
    pub int64: Option<Int64>,
    pub flat_unions: bool,
}

/// A language table: the settings every SDK takes, then the language's own.
macro_rules! language {
    ($name:ident, $package:literal { $($(#[$doc:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            /// Output directory, relative to the repository the SDK lives in: the language's
            /// name, or the root of a repository of its own.
            path: Option<String>,
            /// `owner/name` of a GitHub repository to generate into, over the top-level `repo`.
            repo: Option<String>,
            #[doc = $package]
            package: Option<String>,
            /// API base URL this client defaults to, over the top-level one.
            base_url: Option<String>,
            /// Prefix of this SDK's own headers, over the top-level one.
            header_prefix: Option<String>,
            /// Prefix of this SDK's `User-Agent` header, over the top-level one.
            user_agent: Option<String>,
            /// Installs the Standard Webhooks signature verifier, over the top-level setting.
            webhooks: Option<bool>,
            /// Default request timeout in seconds, over the top-level one.
            timeout: Option<u64>,
            /// How unions decode objects that no rule tells apart, over the top-level setting.
            untagged_unions: Option<UntaggedUnions>,
            /// Method names by operation id, over the top-level `[names]`.
            #[serde(default)]
            names: BTreeMap<String, String>,
            /// Operation ids left out of this SDK only, on top of the top-level `exclude`.
            #[serde(default)]
            exclude: Vec<String>,
            /// Values exposed to templates as `sdk.*`, over the top-level `[context]`.
            #[serde(default)]
            context: BTreeMap<String, Value>,
            $($(#[$doc])* #[serde(default)] $field: $ty,)*
        }

        impl From<$name> for Target {
            fn from(t: $name) -> Self {
                Target {
                    path: t.path,
                    repo: t.repo,
                    package: t.package,
                    base_url: t.base_url,
                    header_prefix: t.header_prefix,
                    user_agent: t.user_agent,
                    webhooks: t.webhooks,
                    timeout: t.timeout,
                    untagged_unions: t.untagged_unions,
                    names: t.names,
                    exclude: t.exclude,
                    context: t.context,
                    $($field: t.$field.into(),)*
                    ..Target::default()
                }
            }
        }
    };
}

language!(Rust, "Crate name: the snake_case `name` by default." {});
language!(TypeScript, "npm package name: the kebab-case `name` by default." {
    /// Modules re-exported from the entry point.
    exports: Vec<String>,
    /// Type of int64 values.
    int64: Option<Int64>,
});
language!(Python, "Python package name: the snake_case `name` by default." {
    /// Types a union as `Circle | Square` instead of a wrapper model.
    flat_unions: bool,
});
language!(Go, "Go package name: the snake_case `name` by default." {
    /// Module path: `github.com/{repo}/{path}` of the repository the SDK lives in by default.
    module: Option<String>,
});
language!(Java, "Java package: `com.{name}` by default." {});
language!(CSharp, "Root namespace and NuGet package: `name` by default." {});

fn target<'de, T, D>(d: D) -> Result<Option<Target>, D::Error>
where
    T: Deserialize<'de> + Into<Target>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(T::deserialize(d)?.into()))
}

/// TypeScript type of int64 values.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Int64 {
    /// `number`, exact up to 2^53.
    #[default]
    Number,
    /// `bigint`.
    Bigint,
    /// `string`.
    String,
}

/// How a union decodes an object when several variants are objects that neither a
/// discriminator nor the constants and required properties they declare tell apart.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    /// Query parameter taking the cursor of the page.
    pub cursor: Option<String>,
    /// Query parameter taking the page number.
    pub page: Option<String>,
    /// Query parameter taking the index of the first item.
    pub offset: Option<String>,
    /// The array of items, `data` by default.
    pub items: Option<String>,
    /// Cursor of the next page in the response.
    pub next_cursor: Option<String>,
    /// Field of the last item that is the next cursor, e.g. `id` for `starting_after`.
    pub item_cursor: Option<String>,
    /// Boolean telling whether more pages follow.
    pub has_more: Option<String>,
    /// Number of pages.
    pub total_pages: Option<String>,
    /// Total number of items, for offset pagination.
    pub total: Option<String>,
    /// Number of the first page, 1 by default.
    pub first_page: Option<i64>,
    /// perseid.toml only: operations this rule must apply to, instead of every matching one.
    #[serde(default)]
    pub operations: Vec<String>,
}

/// One pagination rule, or several.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum OneOrMany {
    One(Box<Pagination>),
    Many(Vec<Pagination>),
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

/// `name` as the UpperCamelCase identifier the client is named after, kept as written when it
/// already is one (`AcmeAPI`).
fn client_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let name = String::deserialize(d)?;
    let camel = match name.chars().all(|c| c.is_ascii_alphanumeric()) {
        true if name.starts_with(|c: char| c.is_ascii_uppercase()) => name.clone(),
        _ => name.to_upper_camel_case(),
    };
    match camel.starts_with(|c: char| c.is_ascii_alphabetic())
        && camel.chars().all(|c| c.is_ascii_alphanumeric())
    {
        true => Ok(camel),
        false => Err(serde::de::Error::custom(format!(
            "`name = {name:?}` must start with a letter and hold ASCII letters, digits, spaces, `-` or `_`"
        ))),
    }
}

/// The JSON Schema of perseid.toml, for editors.
pub fn json_schema() -> String {
    let settings = schemars::r#gen::SchemaSettings::draft07().with(|s| {
        s.option_nullable = false;
        s.option_add_null_type = false;
    });
    let schema = settings.into_generator().into_root_schema_for::<Config>();
    serde_json::to_string_pretty(&schema).unwrap_or_default() + "\n"
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
        config.validate()?;
        let root = std::path::absolute(path)?.parent().unwrap().to_owned();
        Ok((config, root))
    }

    fn validate(&self) -> Result<()> {
        let source = Source::parse(&self.spec)?;
        ensure!(
            self.sdks_repo.is_none() || matches!(source, Source::File(_)),
            "`sdks_repo` sends a spec file of this repository: `spec` must be its path"
        );
        ensure!(
            self.push_on.is_none() || self.pushed(),
            "`push_on` says when the spec is pushed: set `sdks_repo`, or a `github:` spec"
        );
        ensure!(
            self.push_tags.is_none() || self.push_on == Some(PushOn::Tag),
            "`push_tags` needs `push_on = \"tag\"`"
        );
        ensure!(
            !self.internal || self.only.is_empty(),
            "`only` lists every operation generated: `internal` can't add any, delete it"
        );
        Ok(())
    }

    /// Whether another repository than this one's receives or sends the spec.
    fn pushed(&self) -> bool {
        self.sdks_repo.is_some() || matches!(self.source(), Source::GitHub { .. })
    }

    /// The tags pushing the spec with `push_on = "tag"`.
    pub fn push_tags(&self) -> &str {
        self.push_tags.as_deref().unwrap_or("v*")
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
            include_mode: match (self.only.is_empty(), self.internal) {
                (false, _) => IncludeMode::OnlySpecified,
                (true, true) => IncludeMode::PublicAndInternal,
                (true, false) => IncludeMode::OnlyPublic,
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
            let url = self.package.repository.as_deref()?;
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
        let version = manifest_version(dir).unwrap_or_else(|| "0.1.0".into());
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
            "int64": match target.int64.unwrap_or_default() {
                Int64::Number => "number",
                Int64::Bigint => "bigint",
                Int64::String => "string",
            },
            "version": version,
            "extra_exports": target.exports,
            "timeout": target.timeout.or(self.timeout).unwrap_or(60),
            "untagged_unions": match target.untagged_unions.or(self.untagged_unions).unwrap_or_default() {
                UntaggedUnions::Json => "json",
                UntaggedUnions::BestMatch => "best-match",
            },
            "license": self.package.license,
            "repository": self.package.repository,
            "homepage": self.package.homepage,
            "description": self.package.description.clone().unwrap_or_else(|| format!("{} API client", self.name)),
            "authors": self.package.authors,
        });
        let map = context.as_object_mut().unwrap();
        if language == "python" {
            map.insert("flat_unions".into(), target.flat_unions.into());
        }
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

/// Keys of earlier versions, and what replaces them.
const OUTDATED: [(&str, &str); 15] = [
    (
        "method_names",
        "was removed (methods are named after their resource; `[names]` renames one): delete it",
    ),
    (
        "typed_unions",
        "was removed (unions are always typed): delete it",
    ),
    (
        "patch_nullable",
        "was removed (nullable optional PATCH fields can always be cleared): delete it",
    ),
    (
        "initialisms",
        "was removed (Go names always spell initialisms the Go way): delete it",
    ),
    (
        "edition",
        "was removed (Java SDKs always have edition 2's API): delete it",
    ),
    (
        "version",
        "was removed (each SDK's package manifest owns its version, which release-please bumps): delete it",
    ),
    ("push_spec", "was renamed: write `sdks_repo = …` instead"),
    (
        "include",
        "was removed: `internal = true` also generates x-internal operations, `only = [...]` lists the operations to generate",
    ),
    ("description", "moved to the [package] table"),
    ("license", "moved to the [package] table"),
    ("homepage", "moved to the [package] table"),
    ("repository", "moved to the [package] table"),
    ("authors", "moved to the [package] table"),
    ("int64", "is only supported in [typescript]"),
    ("flat_unions", "is only supported in [python]"),
];

fn removed_keys(table: &toml::Table) -> Result<()> {
    let tables = std::iter::once(("", table)).chain(
        LANGUAGES
            .into_iter()
            .filter_map(|l| Some((l, table.get(l)?.as_table()?))),
    );
    for (language, table) in tables {
        for (key, why) in OUTDATED {
            let own = matches!(
                (language, key),
                ("typescript", "int64") | ("python", "flat_unions")
            );
            if table.contains_key(key) && !own {
                let at = match language {
                    "" => String::new(),
                    language => format!("[{language}] "),
                };
                anyhow::bail!("{at}`{key}` {why}");
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
        let toml = "spec = \"s\"\nname = \"Acme\"\ntimeout = 15\n[package]\nlicense = \"MIT\"\n\
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
        assert_eq!(
            error("push_spec = \"acme/sdks\""),
            "`push_spec` was renamed: write `sdks_repo = …` instead"
        );
        assert_eq!(
            error("repository = \"https://github.com/acme/api\""),
            "`repository` moved to the [package] table"
        );
        assert!(error("include = \"public-and-internal\"").contains("`internal = true`"));
        assert!(error("[rust]\nversion = \"1.0.0\"").starts_with("[rust] `version` was removed"));
        assert!(error("[go]\nflat_unions = true").contains("only supported in [python]"));
        let own = "[python]\nflat_unions = true\n[typescript]\nint64 = \"bigint\"\n";
        assert!(removed_keys(&own.parse().unwrap()).is_ok());
    }

    fn load(toml: &str) -> Result<Config> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE);
        std::fs::write(&path, toml)?;
        Ok(Config::load(&path)?.0)
    }

    #[test]
    fn names_in_any_case_are_upper_camel_case() {
        for (name, client) in [
            ("acme-api", "AcmeApi"),
            ("acme_api", "AcmeApi"),
            ("Acme API", "AcmeApi"),
            ("AcmeAPI", "AcmeAPI"),
            ("Meteroid", "Meteroid"),
        ] {
            let config = load(&format!("spec = \"s\"\nname = \"{name}\"\n")).unwrap();
            assert_eq!(config.name, client);
        }
        let toml = "spec = \"s\"\nname = \"acme api\"\n[typescript]\n[python]\n";
        let typescript = context(toml, "typescript");
        assert_eq!(typescript["npm_package"], "acme-api");
        assert_eq!(context(toml, "python")["package_name"], "acme_api");
        let error = load("spec = \"s\"\nname = \"3d API\"\n").err().unwrap();
        assert!(
            format!("{error:#}").contains("must start with a letter"),
            "{error:#}"
        );
    }

    #[test]
    fn spec_pushes_need_another_repository() {
        let pushed = "spec = \"openapi.json\"\nname = \"A\"\nsdks_repo = \"a/sdks\"\n";
        assert_eq!(load(pushed).unwrap().push_on, None);
        let tags = format!("{pushed}push_on = \"tag\"\npush_tags = \"api-v*\"\n");
        assert_eq!(load(&tags).unwrap().push_tags(), "api-v*");
        let received =
            "spec = \"github:a/api/openapi.json\"\nname = \"A\"\npush_on = \"release\"\n";
        assert_eq!(load(received).unwrap().push_on, Some(PushOn::Release));
        let error = |toml: &str| format!("{:#}", load(toml).err().unwrap());
        assert!(
            error("spec = \"openapi.json\"\nname = \"A\"\npush_on = \"release\"\n")
                .contains("set `sdks_repo`")
        );
        assert!(
            error(&format!("{pushed}push_tags = \"v*\"\n")).contains("needs `push_on = \"tag\"`")
        );
        assert!(error(&format!("{pushed}push_on = \"merge\"\n")).contains("unknown variant"));
    }

    #[test]
    fn filters_keep_public_operations_unless_told_otherwise() {
        let filters = |toml: &str| {
            let config = load(&format!("spec = \"s\"\nname = \"A\"\n{toml}")).unwrap();
            config.filters().include_mode
        };
        assert!(matches!(filters(""), IncludeMode::OnlyPublic));
        assert!(matches!(
            filters("internal = true"),
            IncludeMode::PublicAndInternal
        ));
        assert!(matches!(
            filters("only = [\"a\"]"),
            IncludeMode::OnlySpecified
        ));
        let error = load("spec = \"s\"\nname = \"A\"\ninternal = true\nonly = [\"a\"]\n");
        assert!(format!("{:#}", error.err().unwrap()).contains("`internal` can't add any"));
    }

    #[test]
    fn language_keys_belong_to_their_table() {
        let toml = "spec = \"s\"\nname = \"A\"\n[python]\nflat_unions = true\n\
                    [typescript]\nint64 = \"bigint\"\n";
        assert_eq!(context(toml, "python")["flat_unions"], true);
        assert_eq!(context(toml, "typescript")["int64"], "bigint");
        assert!(load("spec = \"s\"\nname = \"A\"\n[typescript]\nint64 = \"long\"\n").is_err());
        assert!(load("spec = \"s\"\nname = \"A\"\n[java]\nmodule = \"m\"\n").is_err());
    }

    #[test]
    fn the_committed_json_schema_is_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("perseid.schema.json");
        let schema = json_schema();
        if std::env::var_os("PERSEID_WRITE_SCHEMA").is_some() {
            std::fs::write(&path, &schema).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == schema,
            "perseid.schema.json is stale: run `cargo run -- schema > perseid.schema.json`"
        );
        let schema: Value = serde_json::from_str(&schema).unwrap();
        assert_eq!(schema["$schema"], "http://json-schema.org/draft-07/schema#");
        assert_eq!(schema["additionalProperties"], false);
        let typescript = &schema["definitions"]["TypeScript"];
        assert_eq!(typescript["additionalProperties"], false);
        assert!(typescript["properties"]["int64"].is_object());
        assert!(schema["definitions"]["Rust"]["properties"]["int64"].is_null());
    }
}
