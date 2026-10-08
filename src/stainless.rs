//! `perseid init --from stainless.yml`: the perseid.toml settings a Stainless config maps to, and
//! the keys of it perseid has no equivalent for, each with why.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Result, ensure};
use heck::ToUpperCamelCase;
use serde_json::Value;

use crate::{
    api::{naming::snake, resources::Operation},
    config::{LANGUAGES, Pagination},
    spec::{Filters, IncludeMode},
};

/// perseid's request timeout, in seconds, when `timeout` is unset.
const DEFAULT_TIMEOUT: u64 = 60;

/// What a stainless.yml maps to, and what it doesn't.
#[derive(Default)]
pub(crate) struct Import {
    pub name: Option<String>,
    /// `organization.name` as written, which `authors` names.
    pub organization: Option<String>,
    /// SDKs of the targets stainless.yml generates, in `LANGUAGES` order.
    pub sdks: Vec<String>,
    pub targets: BTreeMap<&'static str, Target>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub contact: Option<String>,
    pub base_url: Option<String>,
    pub timeout: Option<u64>,
    pub idempotency_keys: bool,
    /// The prefix of the environment variables the clients read, as `read_env` names them.
    pub env_prefix: Option<String>,
    /// Each rule with the name of the Stainless scheme it comes from.
    pub pagination: Vec<(String, Pagination)>,
    pub methods: BTreeMap<String, String>,
    pub exclude: Vec<String>,
    /// What was imported, for the summary.
    pub mapped: Vec<String>,
    /// `(key, why)` of what has no perseid equivalent.
    pub skipped: Vec<(String, String)>,
    /// What the user should check before generating.
    pub warnings: Vec<String>,
    endpoints: Vec<Method>,
    /// `unspecified_endpoints`, as `(verb, path)`.
    unspecified: Vec<(String, String)>,
    security_schemes: Option<Value>,
    security: Option<Value>,
}

/// The settings of one language table.
#[derive(Default)]
pub(crate) struct Target {
    pub package: Option<String>,
    pub repo: Option<String>,
    pub module: Option<String>,
    pub exclude: Vec<String>,
}

/// A method of a Stainless resource.
struct Method {
    /// `users.feeds.list_items`.
    dotted: String,
    resource: String,
    name: String,
    verb: String,
    path: String,
    /// `paginated: false`, or `true` or a scheme name.
    paginated: Option<bool>,
    /// The SDKs it is left out of.
    skipped_in: BTreeSet<&'static str>,
}

/// Stainless target names, as perseid languages.
fn language(target: &str) -> Option<&'static str> {
    match target {
        "node" | "typescript" => Some("typescript"),
        "python" => Some("python"),
        "go" => Some("go"),
        "java" => Some("java"),
        "csharp" => Some("csharp"),
        _ => None,
    }
}

/// Reads the Stainless config at `location`, a path under `root` or a URL.
pub(crate) fn read(location: &str, root: &Path) -> Result<Import> {
    let doc = crate::spec::load(location, root)?;
    ensure!(
        doc.is_object(),
        "{location} is not a Stainless config: it holds no mapping"
    );
    ensure!(
        !doc["openapi"].is_string() && !doc["openapi"].is_number() && doc.get("paths").is_none(),
        "{location} is an OpenAPI document: pass it as --spec, and the Stainless config (stainless.yml) as --from"
    );
    Ok(Import::from_config(&doc))
}

impl Import {
    fn skip(&mut self, key: impl Into<String>, why: impl Into<String>) {
        self.skipped.push((key.into(), why.into()));
    }

    fn from_config(doc: &Value) -> Self {
        let mut import = Import::default();
        for (key, value) in doc.as_object().into_iter().flatten() {
            match key.as_str() {
                "organization" => import.organization(value, &doc["custom_casings"]),
                "targets" => import.targets(value),
                "environments" => import.environments(value),
                "client_settings" => import.client_settings(value),
                "pagination" => import.pagination(value),
                "resources" => import.resources(value),
                "settings" => import.settings(value),
                "query_settings" => import.query_settings(value),
                "security_schemes" => import.security_schemes = Some(value.clone()),
                "security" => import.security = Some(value.clone()),
                "unspecified_endpoints" => {
                    import.unspecified = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| endpoint_of(e.as_str()?))
                        .collect();
                }
                "custom_casings" => import.skip(
                    key,
                    "perseid spells names its own way in each language; only the client name took them",
                ),
                "edition" => import.skip(
                    key,
                    "perseid has no editions: a perseid upgrade that changes the SDKs comes as a pull request",
                ),
                "readme" => import.skip(
                    key,
                    "perseid writes the README examples from the spec's operations",
                ),
                "openapi" => import.skip(
                    key,
                    "apply transforms to the spec itself; perseid reads it as is",
                ),
                "diagnostics" => import.skip(key, "perseid warns about the spec as it generates"),
                "codeflow" => import.skip(
                    key,
                    "the perseid Action opens the pull requests and release-please releases them, see docs/ci.md",
                ),
                "streaming" => import.skip(
                    key,
                    "perseid streams `text/event-stream` responses as the spec declares them",
                ),
                "multipart_settings" => {
                    import.skip(key, "perseid sends list fields of multipart bodies one part per item")
                }
                _ => import.skip(key, "no perseid.toml equivalent"),
            }
        }
        if doc["query_settings"].get("array_format").is_none() {
            import.skip(
                "query_settings.array_format",
                "Stainless sends list parameters comma-separated (`a=1,2`) by default: perseid repeats them (`a=1&a=2`) unless the spec sets `explode: false` on them",
            );
        }
        if import.endpoints.is_empty() && doc["unspecified_endpoints"].is_array() {
            import.skip(
                "unspecified_endpoints",
                "perseid generates every operation of the spec: list their operation ids in `exclude`",
            );
        }
        import
    }

    fn organization(&mut self, org: &Value, casings: &Value) {
        for (key, value) in org.as_object().into_iter().flatten() {
            let text = value.as_str().map(str::trim).filter(|s| !s.is_empty());
            let at = format!("organization.{key}");
            match key.as_str() {
                "name" => {
                    match text.map(|name| crate::client_name::parse(&client_name(name, casings))) {
                        Some(Ok(name)) => {
                            self.name = Some(name);
                            self.organization = text.map(str::to_owned);
                            self.mapped.push("name".into());
                        }
                        Some(Err(error)) => self.skip(at, error),
                        None => {}
                    }
                }
                "docs" => {
                    self.homepage = text.map(|url| match url.contains("://") {
                        true => url.to_owned(),
                        false => format!("https://{url}"),
                    });
                }
                "contact" => self.contact = text.map(str::to_owned),
                "github_org" => self.skip(
                    at,
                    "perseid needs no organization: `repo` and each SDK's `repo` name repositories",
                ),
                "upload_spec" => self.skip(at, "perseid ships no copy of the spec in the SDKs"),
                "security_contact" | "security_policy_url" | "security_policy_terms" => self.skip(
                    at,
                    "perseid writes no security policy: add a SECURITY.md to the SDK repositories",
                ),
                _ => self.skip(at, "no perseid.toml equivalent"),
            }
        }
    }

    fn targets(&mut self, targets: &Value) {
        let mut found: BTreeMap<&'static str, (&str, &Value)> = BTreeMap::new();
        for (key, target) in targets.as_object().into_iter().flatten() {
            let at = format!("targets.{key}");
            let Some(language) = language(key) else {
                self.skip(at, unsupported_target(key));
                continue;
            };
            if target["skip"] == true {
                self.skip(at, "`skip: true`: left out of `sdks`");
                continue;
            }
            match found.get(language) {
                Some((other, _)) if *other == "typescript" => {
                    self.skip(at, "the `typescript` target is imported instead");
                    continue;
                }
                Some((other, _)) => self.skip(
                    format!("targets.{other}"),
                    format!("the `{key}` target is imported instead"),
                ),
                None => {}
            }
            found.insert(language, (key, target));
        }
        let mut shared: BTreeMap<&str, (Vec<&str>, &'static str)> = BTreeMap::new();
        for (language, (key, target)) in found {
            for (field, why) in self.target(language, key, target) {
                shared.entry(field).or_insert((Vec::new(), why)).0.push(key);
            }
        }
        for (field, (keys, why)) in shared {
            let keys = match keys.as_slice() {
                [one] => (*one).to_owned(),
                _ => format!("{{{}}}", keys.join(",")),
            };
            self.skip(format!("targets.{keys}.{field}"), why);
        }
        self.sdks = LANGUAGES
            .iter()
            .filter(|l| self.targets.contains_key(*l))
            .map(|l| (*l).to_owned())
            .collect();
        if !self.sdks.is_empty() {
            self.mapped.push("sdks".into());
        }
        if self.targets.values().any(|t| t.package.is_some()) {
            self.mapped.push("packages".into());
        }
        if self.targets.values().any(|t| t.repo.is_some()) {
            self.mapped.push("repositories".into());
        }
    }

    /// Imports a target, giving the fields that no target maps, with why.
    fn target<'a>(
        &mut self,
        language: &'static str,
        key: &str,
        target: &'a Value,
    ) -> Vec<(&'a str, &'static str)> {
        let mut unmapped = Vec::new();
        let mut out = Target::default();
        let package_key = match language {
            "java" => "reverse_domain",
            _ => "package_name",
        };
        let text = |k: &str| {
            target[k]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        out.package = text(package_key);
        if let Some(production) = text("production_repo") {
            let at = format!("targets.{key}.production_repo");
            let (repo, branch) = production.split_once('#').unwrap_or((&production, ""));
            match repo_name(repo) {
                true => out.repo = Some(repo.to_owned()),
                false => self.skip(
                    &at,
                    format!("`{repo}` is no GitHub owner/name: set `repo` under [{language}]"),
                ),
            }
            if !branch.is_empty() {
                self.skip(
                    at,
                    format!("perseid opens its pull requests against the default branch of the repository, not `{branch}`"),
                );
            }
        }
        for (k, value) in target.as_object().into_iter().flatten() {
            let at = format!("targets.{key}.{k}");
            match k.as_str() {
                k if k == package_key || k == "production_repo" => {}
                "skip" => {}
                "project_name" => {
                    let project = value.as_str().unwrap_or_default();
                    if Some(project) != out.package.as_deref() {
                        self.skip(
                            at,
                            format!(
                                "perseid publishes the Python package under its import name: after the first generation, set `name = \"{project}\"` in the SDK's pyproject.toml"
                            ),
                        );
                    }
                }
                "options" => {
                    for (option, value) in value.as_object().into_iter().flatten() {
                        match (option.as_str(), value.as_str()) {
                            ("go_module_path_override", Some(module)) => {
                                out.module = Some(module.to_owned());
                            }
                            _ => self.skip(format!("{at}.{option}"), target_option(option)),
                        }
                    }
                }
                field => unmapped.push((
                    field,
                    match field {
                        "staging_repo" => "perseid opens its pull requests in the SDK repository itself",
                        "publish" => "sdk-release.yml publishes each SDK on release with trusted publishing, see docs/ci.md#publishing",
                        "edition" => "perseid has no editions",
                        "keep_files" => "nothing to set: perseid only rewrites or deletes the files it marked @generated",
                        "readme_title" => "perseid titles the README after `name`",
                        _ => "no perseid.toml equivalent",
                    },
                )),
            }
        }
        if language == "go" && out.module.is_none() {
            self.skip(
                format!("targets.{key}"),
                "check the Go module path perseid derives from the repository: set `module` under [go] if it had a /vN suffix",
            );
        }
        if language == "java" {
            self.skip(
                format!("targets.{key}"),
                "perseid publishes the Maven artifact named after `name`: check `POM_ARTIFACT_ID` in the SDK's gradle.properties after the first generation",
            );
        }
        self.targets.insert(language, out);
        unmapped
    }

    fn environments(&mut self, environments: &Value) {
        let mut entries = environments.as_object().into_iter().flatten();
        if let Some((name, url)) = entries.next() {
            match url.as_str() {
                Some(url) => {
                    self.base_url = Some(url.trim_end_matches('/').to_owned());
                    self.mapped.push("base_url".into());
                }
                None => self.skip(format!("environments.{name}"), "not a URL"),
            }
        }
        for (name, url) in entries {
            self.skip(
                format!("environments.{name}"),
                format!(
                    "perseid clients have one default base URL: pass {} as `baseURL`/`base_url`, or set the `_BASE_URL` environment variable",
                    url.as_str().unwrap_or("its URL")
                ),
            );
        }
    }

    fn client_settings(&mut self, settings: &Value) {
        for (key, value) in settings.as_object().into_iter().flatten() {
            let at = format!("client_settings.{key}");
            match key.as_str() {
                "opts" => {
                    for (name, opt) in value.as_object().into_iter().flatten() {
                        self.option(name, opt);
                    }
                }
                "default_timeout" => match seconds(value).map(|s| s.ceil().max(1.0) as u64) {
                    Some(DEFAULT_TIMEOUT) => {}
                    Some(seconds) => {
                        self.timeout = Some(seconds);
                        self.mapped.push("timeout".into());
                    }
                    None => self.skip(at, "not a number of milliseconds nor an ISO 8601 duration"),
                },
                "idempotency" => {
                    self.idempotency_keys = true;
                    self.mapped.push("idempotency_keys".into());
                    let header = value["header"].as_str().unwrap_or("Idempotency-Key");
                    if !header.eq_ignore_ascii_case("Idempotency-Key") {
                        self.skip(
                            format!("{at}.header"),
                            format!("perseid sends `Idempotency-Key`, not `{header}`"),
                        );
                    }
                }
                "default_retries" => {
                    let defaults = [
                        ("max_retries", 2.0),
                        ("initial_delay_seconds", 0.5),
                        ("max_delay_seconds", 8.0),
                    ];
                    for (field, setting) in value.as_object().into_iter().flatten() {
                        let default = defaults.iter().find(|(f, _)| f == field).map(|(_, d)| *d);
                        if setting.as_f64() != default {
                            self.skip(
                                format!("{at}.{field}"),
                                "perseid retries twice, from 0.5s up to 8s: set `maxRetries`/`max_retries` on the client to change it",
                            );
                        }
                    }
                }
                "default_env_prefix" => match value.as_str().map(|p| p.trim_end_matches('_')) {
                    Some(prefix) if !prefix.is_empty() => self.env_prefix(&at, prefix),
                    _ => self.skip(at, "not an environment variable prefix"),
                },
                "default_max_retries" if value.as_u64() == Some(2) => {}
                "default_max_retries" => self.skip(
                    at,
                    "perseid retries twice: set `maxRetries`/`max_retries` on the client to change it",
                ),
                _ => self.skip(at, "no perseid.toml equivalent"),
            }
        }
    }

    /// A client option: credentials read from the environment, or a value sent with every request.
    fn option(&mut self, name: &str, opt: &Value) {
        let at = format!("client_settings.opts.{name}");
        let env = opt["read_env"].as_str();
        let Some(auth) = opt.get("auth") else {
            let sent = [
                "send_in_header",
                "send_as_query_param",
                "send_as_path_param",
                "send_as_body_param",
            ]
            .into_iter()
            .find_map(|k| Some((k, opt[k].as_str()?)));
            let why = match sent {
                Some(("send_in_header", header)) => format!(
                    "perseid clients take no custom options: send `{header}` with the default headers (`defaultHeaders`, `default_headers`)"
                ),
                Some((_, param)) => {
                    format!("perseid clients take no custom options: pass `{param}` on each call")
                }
                None => "perseid clients take no custom options".to_owned(),
            };
            self.skip(at, why);
            return;
        };
        let role = auth["role"].as_str().unwrap_or("value");
        let suffix = match role {
            "client_id" => Some("_CLIENT_ID"),
            "client_secret" => Some("_CLIENT_SECRET"),
            "value" => Some("_API_KEY"),
            _ => None,
        };
        match (env, suffix) {
            (None, _) => {}
            (Some(env), Some(suffix)) if env.ends_with(suffix) && env.len() > suffix.len() => {
                self.env_prefix(&format!("{at}.read_env"), &env[..env.len() - suffix.len()]);
            }
            (Some(env), Some(suffix)) => self.skip(
                format!("{at}.read_env"),
                format!("perseid reads `<PREFIX>{suffix}`, not `{env}`"),
            ),
            (Some(env), None) => self.skip(
                format!("{at}.read_env"),
                format!("perseid reads no environment variable for the {role} of HTTP basic auth: `{env}` is not read, pass `basicAuth`/`basic_auth`"),
            ),
        }
        let scheme = auth["security_scheme"]
            .as_str()
            .unwrap_or("its security scheme");
        for key in ["send_in_header", "send_as_query_param"] {
            if opt.get(key).is_some() {
                self.skip(
                    format!("{at}.{key}"),
                    format!("perseid sends credentials as the spec's `{scheme}` security scheme declares: check its `in` and `name`"),
                );
            }
        }
    }

    /// Takes `prefix` as that of every environment variable, unless another one was.
    fn env_prefix(&mut self, at: &str, prefix: &str) {
        match &self.env_prefix {
            None => self.env_prefix = Some(prefix.to_owned()),
            Some(other) if other == prefix => {}
            Some(other) => self.skip(
                at,
                format!("perseid reads every variable with one prefix, {other}_, not {prefix}_"),
            ),
        }
    }

    fn pagination(&mut self, schemes: &Value) {
        let schemes = match schemes {
            Value::Array(schemes) => schemes.iter().collect::<Vec<_>>(),
            scheme => vec![scheme],
        };
        for (i, scheme) in schemes.into_iter().enumerate() {
            let name = scheme["name"]
                .as_str()
                .map_or_else(|| i.to_string(), str::to_owned);
            let at = format!("pagination.{name}");
            match rule(scheme) {
                Ok((rule, ignored)) => {
                    for (key, why) in ignored {
                        self.skip(format!("{at}.{key}"), why);
                    }
                    self.pagination.push((name, rule));
                }
                Err(why) => self.skip(at, why),
            }
        }
        if !self.pagination.is_empty() {
            self.mapped
                .push(count(self.pagination.len(), "pagination rule"));
        }
    }

    fn resources(&mut self, resources: &Value) {
        let mut ignored: BTreeMap<String, (usize, &'static str, &'static str)> = BTreeMap::new();
        let mut renamed = Vec::new();
        let mut stack: Vec<(String, &Value, BTreeSet<&'static str>)> = resources
            .as_object()
            .into_iter()
            .flatten()
            .rev()
            .map(|(name, r)| (name.clone(), r, skipped_in(r, &BTreeSet::new())))
            .collect();
        while let Some((dotted, resource, skipped)) = stack.pop() {
            for (key, value) in resource.as_object().into_iter().flatten() {
                match key.as_str() {
                    "models" => {
                        for (model, schema) in value.as_object().into_iter().flatten() {
                            let schema = schema.as_str().or(schema["openapi_uri"].as_str());
                            let schema = schema.map(|s| s.rsplit('/').next().unwrap_or(s));
                            let wanted = model.to_upper_camel_case();
                            if let Some(schema) = schema
                                && schema.to_upper_camel_case() != wanted
                            {
                                renamed
                                    .push(format!("{wanted} is {}", schema.to_upper_camel_case()));
                            }
                        }
                    }
                    "methods" => {
                        for (name, method) in value.as_object().into_iter().flatten() {
                            self.method(&dotted, name, method, &skipped, &mut ignored);
                        }
                    }
                    "subresources" => {
                        let subs: Vec<_> = value.as_object().into_iter().flatten().collect();
                        for (name, sub) in subs.into_iter().rev() {
                            let skipped = skipped_in(sub, &skipped);
                            stack.push((format!("{dotted}.{name}"), sub, skipped));
                        }
                    }
                    "skip" | "only" => {}
                    other => {
                        ignored
                            .entry(format!("resources.*.{other}"))
                            .or_insert((0, "resource", resource_key(other)))
                            .0 += 1;
                    }
                }
            }
        }
        if !renamed.is_empty() {
            self.skip(
                "resources.*.models",
                format!(
                    "perseid names types after their schema, without resource namespaces: {}",
                    renamed.join(", ")
                ),
            );
        }
        for (key, (n, noun, why)) in ignored {
            self.skip(format!("{key} ({})", count(n, noun)), why);
        }
    }

    fn method(
        &mut self,
        resource: &str,
        name: &str,
        method: &Value,
        skipped: &BTreeSet<&'static str>,
        ignored: &mut BTreeMap<String, (usize, &'static str, &'static str)>,
    ) {
        let dotted = format!("{resource}.{name}");
        let endpoint = method.as_str().or(method["endpoint"].as_str());
        let Some((verb, path)) = endpoint.and_then(endpoint_of) else {
            let why = match method["type"].as_str() {
                Some("webhook_unwrap") => {
                    "`webhooks = true` installs perseid's Standard Webhooks verifier instead"
                }
                Some("websocket") => "perseid generates no WebSocket clients",
                _ => "no endpoint: perseid generates methods of the spec's operations only",
            };
            self.skip(format!("resources.{dotted}"), why);
            return;
        };
        let paginated = match &method["paginated"] {
            Value::Bool(paginated) => Some(*paginated),
            Value::String(_) => Some(true),
            _ => None,
        };
        for (key, _) in method.as_object().into_iter().flatten() {
            if !["endpoint", "paginated", "skip", "only", "type"].contains(&key.as_str()) {
                ignored
                    .entry(format!("resources.*.methods.*.{key}"))
                    .or_insert((0, "method", method_key(key)))
                    .0 += 1;
            }
        }
        if method["type"].as_str().is_some_and(|t| t != "http") {
            self.skip(
                format!("resources.{dotted}.type"),
                "imported as a plain method of the operation",
            );
        }
        self.endpoints.push(Method {
            dotted,
            resource: resource.to_owned(),
            name: name.to_owned(),
            verb,
            path,
            paginated,
            skipped_in: skipped_in(method, skipped),
        });
    }

    fn settings(&mut self, settings: &Value) {
        for (key, value) in settings.as_object().into_iter().flatten() {
            let at = format!("settings.{key}");
            match (key.as_str(), value.as_str()) {
                ("license", Some(license)) => {
                    self.license = Some(license.to_owned());
                    self.mapped.push("license".into());
                }
                ("disable_mock_tests", _) => self.skip(
                    at,
                    "perseid's generated tests run against a mock server they start: `tests = false` leaves them out",
                ),
                _ => self.skip(at, "no perseid.toml equivalent"),
            }
        }
    }

    fn query_settings(&mut self, settings: &Value) {
        for (key, value) in settings.as_object().into_iter().flatten() {
            let at = format!("query_settings.{key}");
            match (key.as_str(), value.as_str().unwrap_or_default()) {
                ("nested_format", "brackets") | ("array_format", "repeat") => {}
                ("nested_format", _) => {
                    self.skip(at, "perseid sends nested objects as `a[b]=c`")
                }
                ("array_format", "comma") => self.skip(
                    at,
                    "perseid repeats list parameters (`a=1&a=2`) unless the spec sets `explode: false` on them",
                ),
                ("array_format", _) => self.skip(
                    at,
                    "perseid repeats list parameters (`a=1&a=2`), with `a[]=1` only for `style: deepObject`",
                ),
                _ => self.skip(at, "no perseid.toml equivalent"),
            }
        }
    }

    /// Without a spec, nothing tells the operations the resources and pagination rules refer to.
    pub(crate) fn without_spec(&mut self) {
        if !self.endpoints.is_empty() {
            self.skip(
                "resources",
                "method names and the operations stainless.yml leaves out need the spec: delete perseid.toml, then run `perseid init` again with `--spec <path|url>`",
            );
        }
        for (key, value) in [
            ("security_schemes", &self.security_schemes),
            ("security", &self.security),
        ] {
            if value.is_some() {
                self.skipped.push((
                    key.to_owned(),
                    "perseid reads them from the spec: move them to its `components.securitySchemes` and `security`".to_owned(),
                ));
            }
        }
    }

    /// Maps the resources to the operations of `spec` (JSON, as `spec::read` gives it): method
    /// names, operations stainless.yml leaves out, and the pagination each method gets.
    pub(crate) fn with_spec(&mut self, spec: &str) {
        let raw: Value = serde_json::from_str(spec).unwrap_or_default();
        self.check_security(&raw);
        if self.endpoints.is_empty() {
            return;
        }
        let filters =
            |names: &BTreeMap<String, String>, pagination: &[(String, Pagination)]| Filters {
                include_mode: IncludeMode::OnlyPublic,
                excluded: BTreeSet::new(),
                specified: BTreeSet::new(),
                pagination: pagination.iter().map(|(_, p)| p.clone()).collect(),
                detect_pagination: true,
                reserved: BTreeSet::new(),
                names: names.clone(),
                uuid_strings: false,
            };
        let api = match quietly(|| {
            crate::spec::api(spec, &filters(&BTreeMap::new(), &self.pagination))
        }) {
            Ok(api) => api,
            Err(error) => {
                self.skip(
                    "resources",
                    format!("the spec can't be read, so no method is mapped: {error:#}"),
                );
                return;
            }
        };
        let generated_ops = operations(&api);
        let by_endpoint: BTreeMap<(String, String), &(String, &Operation)> = generated_ops
            .iter()
            .map(|entry| ((entry.1.method.clone(), shape(&entry.1.path)), entry))
            .collect();
        let find =
            |verb: &str, path: &str| by_endpoint.get(&(verb.to_owned(), shape(path))).copied();
        let mut used = BTreeSet::new();
        let mut names = BTreeMap::new();
        let mut excluded: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        let mut mismatches = Vec::new();
        let mut missing = Groups::default();
        for method in &self.endpoints {
            let at = format!("resources.{}", method.dotted);
            let Some((resource, op)) = find(&method.verb, &method.path) else {
                missing.add(
                    &method.resource,
                    "no operation perseid generates: not in the spec, or `x-internal`".into(),
                    format!("{} {}", method.verb, method.path),
                );
                continue;
            };
            used.insert(op.id.clone());
            for language in &method.skipped_in {
                excluded.entry(language).or_default().push(op.id.clone());
            }
            if same_resource(&method.resource, resource) && snake(&op.name) != snake(&method.name) {
                names.insert(op.id.clone(), method.name.clone());
            }
            match (method.paginated, op.paginated()) {
                (Some(false), true) => mismatches.push((
                    at.clone(),
                    format!("`paginated: false`, but perseid pages `{}`: set `x-pagination: false` on it in the spec", op.id),
                )),
                (Some(true), false) if !self.pagination.is_empty() => mismatches.push((
                    at.clone(),
                    format!("no pagination rule matches `{}`: its method returns the response body", op.id),
                )),
                _ => {}
            }
        }
        let total = self.endpoints.len();
        let matched = total - missing.count();
        let trusted = matched * 5 >= total * 4;
        drop_clashes(&generated_ops, &mut names);
        let renamed = match quietly(|| crate::spec::api(spec, &filters(&names, &self.pagination))) {
            Ok(api) => Some(api),
            Err(error) => {
                self.skip(
                    "resources",
                    format!("the method names of stainless.yml clash in perseid, so none is kept: {error:#}"),
                );
                names.clear();
                None
            }
        };
        let generated: BTreeMap<String, (String, String)> =
            operations(renamed.as_ref().unwrap_or(&api))
                .into_iter()
                .map(|(resource, op)| (op.id.clone(), (resource, snake(&op.name))))
                .collect();
        let mut moved = Groups::default();
        for method in &self.endpoints {
            let Some((_, op)) = find(&method.verb, &method.path) else {
                continue;
            };
            let Some((resource, name)) = generated.get(&op.id) else {
                continue;
            };
            if *resource == method.resource && *name == snake(&method.name) {
                continue;
            }
            let why = if method.resource == "$client" {
                "methods of the client: perseid has none, it groups operations by their first tag"
                    .to_owned()
            } else if method.resource.contains('.') {
                "nested resource: perseid has one level of resources, named after the operations' first tag".to_owned()
            } else if *resource == format!("{}_api", snake(&method.resource)) {
                format!(
                    "perseid names the resource `{resource}`, as a type is named like `{}`",
                    method.resource
                )
            } else if !same_resource(&method.resource, resource) {
                format!(
                    "perseid groups operations by their first tag: tag them `{}` in the spec",
                    method.resource
                )
            } else {
                "the names clash with other methods of the resource in perseid".to_owned()
            };
            moved.add(&method.resource, why, format!("{resource}.{name}"));
        }
        self.skipped.extend(mismatches);
        self.skipped.extend(moved.lines());
        match trusted {
            true => self.skipped.extend(missing.lines()),
            false => self.warnings.push(format!(
                "only {matched} of the {total} endpoints of stainless.yml are operations of the spec{}: stainless.yml may have been written for another spec. Only its `skip` and `unspecified_endpoints` are excluded: the operations it doesn't list are generated",
                missing.first().map(|e| format!(" (`{e}` is not)")).unwrap_or_default()
            )),
        }
        if !names.is_empty() {
            self.mapped.push(count(names.len(), "method name"));
        }
        self.methods = names;
        let left_out = |id: &String| {
            !self.sdks.is_empty()
                && self
                    .sdks
                    .iter()
                    .all(|l| excluded.get(l.as_str()).is_some_and(|ids| ids.contains(id)))
        };
        let everywhere: BTreeSet<String> = excluded
            .values()
            .flatten()
            .filter(|id| left_out(id))
            .cloned()
            .collect();
        let unspecified: BTreeSet<&String> = self
            .unspecified
            .iter()
            .filter_map(|(verb, path)| Some(&find(verb, path)?.1.id))
            .collect();
        self.exclude = generated_ops
            .iter()
            .map(|(_, op)| &op.id)
            .filter(|id| {
                everywhere.contains(*id)
                    || unspecified.contains(id)
                    || (trusted && !used.contains(*id))
            })
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for (language, ids) in excluded {
            if let Some(target) = self.targets.get_mut(language) {
                let ids: BTreeSet<String> = ids
                    .into_iter()
                    .filter(|id| !everywhere.contains(id))
                    .collect();
                target.exclude = ids.into_iter().collect();
            }
        }
        if !self.exclude.is_empty() {
            self.mapped.push(format!(
                "exclude ({})",
                count(self.exclude.len(), "operation")
            ));
        }
    }

    fn check_security(&mut self, spec: &Value) {
        if let Some(schemes) = &self.security_schemes {
            let declared = &spec["components"]["securitySchemes"];
            let missing: Vec<&str> = schemes
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, _)| name.as_str())
                .filter(|name| declared.get(*name).is_none())
                .collect();
            if !missing.is_empty() {
                self.skip(
                    "security_schemes",
                    format!(
                        "perseid reads security schemes from the spec, which lacks `{}`: add them to its `components.securitySchemes`",
                        missing.join("`, `")
                    ),
                );
            }
        }
        if let Some(security) = &self.security
            && spec.get("security") != Some(security)
        {
            self.skip(
                "security",
                "perseid reads the default requirement from the spec's `security`, which differs: copy it there",
            );
        }
    }

    /// The top-level keys of perseid.toml, after `base_url`.
    pub(crate) fn top_level(&self) -> String {
        let mut out = String::new();
        if let Some(timeout) = self.timeout {
            out += &format!("timeout = {timeout}\n");
        }
        if self.idempotency_keys {
            out += "idempotency_keys = true\n";
        }
        if !self.exclude.is_empty() {
            out += &format!("exclude = {}\n", list(&self.exclude));
        }
        out
    }

    /// The `[methods]`, `[context]` and `[[pagination]]` tables.
    pub(crate) fn tables(&self, env_prefix: Option<&str>) -> String {
        let mut out = String::new();
        if !self.methods.is_empty() {
            out += "\n[methods]\n";
            for (id, name) in &self.methods {
                out += &format!("{} = {}\n", key(id), quote(name));
            }
        }
        if let Some(prefix) = env_prefix {
            out += &format!("\n[context]\nenv_prefix = {}\n", quote(prefix));
        }
        for (name, rule) in &self.pagination {
            out += &format!("\n[[pagination]]                      # {name}\n");
            let fields = [
                ("cursor", &rule.cursor),
                ("page", &rule.page),
                ("offset", &rule.offset),
                ("items", &rule.items),
                ("next_cursor", &rule.next_cursor),
                ("item_cursor", &rule.item_cursor),
                ("before", &rule.before),
                ("has_more", &rule.has_more),
                ("total_pages", &rule.total_pages),
                ("total", &rule.total),
            ];
            for (field, value) in fields {
                if let Some(value) = value {
                    out += &format!("{field} = {}\n", quote(value));
                }
            }
        }
        out
    }
}

/// The client class Stainless names after `organization.name`: `onebusaway-sdk` is
/// `OnebusawaySDK`, and `custom_casings` spell words their way.
fn client_name(org: &str, casings: &Value) -> String {
    org.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|word| {
            let lower = word.to_lowercase();
            let casing = &casings[&lower];
            match casing["pascal"].as_str() {
                Some(pascal) => pascal.to_owned(),
                None if casing["initialism"] == true
                    || ["ai", "api", "sdk"].contains(&lower.as_str()) =>
                {
                    word.to_uppercase()
                }
                None => {
                    let mut chars = word.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                }
            }
        })
        .collect()
}

/// `get /v1/users` as `("get", "/v1/users")`.
fn endpoint_of(text: &str) -> Option<(String, String)> {
    let (verb, path) = text.trim().split_once(' ')?;
    Some((verb.to_lowercase(), path.trim().to_owned()))
}

/// Whether `repo` reads as a GitHub `owner/name`.
fn repo_name(repo: &str) -> bool {
    let parts: Vec<&str> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
}

/// Seconds of a Stainless duration: milliseconds, or ISO 8601 such as `PT1M30S`.
fn seconds(value: &Value) -> Option<f64> {
    if let Some(ms) = value.as_f64() {
        return Some(ms / 1000.0);
    }
    let text = value.as_str()?.trim().to_uppercase();
    let rest = text.strip_prefix('P')?;
    let (days, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0.0;
    for (part, units) in [
        (days, &[('D', 86_400.0)][..]),
        (time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)][..]),
    ] {
        let mut rest = part;
        while !rest.is_empty() {
            let end = rest.find(|c: char| c.is_ascii_alphabetic())?;
            let number: f64 = rest[..end].parse().ok()?;
            let unit = rest[end..].chars().next()?;
            total += number * units.iter().find(|(u, _)| *u == unit)?.1;
            rest = &rest[end + 1..];
        }
    }
    Some(total)
}

/// One `x-stainless-pagination-property` purpose, at a dotted path.
struct Property<'a> {
    path: String,
    purpose: &'a str,
    schema: &'a Value,
}

/// The properties of an object schema, and of the objects within, with their purposes.
fn properties<'a>(props: &'a Value, prefix: &str, out: &mut Vec<Property<'a>>) {
    for (name, schema) in props.as_object().into_iter().flatten() {
        let marker = &schema["x-stainless-pagination-property"];
        let purpose = marker.as_str().or(marker["purpose"].as_str()).unwrap_or("");
        let path = format!("{prefix}{name}");
        out.push(Property {
            path: path.clone(),
            purpose,
            schema,
        });
        if schema.get("properties").is_some() {
            properties(&schema["properties"], &format!("{path}."), out);
        }
    }
}

/// The perseid rule of a Stainless pagination scheme, with the purposes it leaves out; or why
/// none fits.
fn rule(scheme: &Value) -> Result<(Pagination, Vec<(String, String)>), String> {
    let kind = scheme["type"].as_str().unwrap_or_default();
    if scheme["param_location"].as_str() == Some("body") {
        return Err("perseid reads the page from a query parameter, not the request body".into());
    }
    let mut request = Vec::new();
    properties(&scheme["request"], "", &mut request);
    request.retain(|p| !p.path.contains('.') || !p.purpose.is_empty());
    let mut response = Vec::new();
    properties(&scheme["response"], "", &mut response);
    let typed = |p: &&Property, ty: &str| p.schema["type"].as_str() == Some(ty);
    let param = |purpose: &str, names: &[&str], ty: &str| {
        request
            .iter()
            .find(|p| p.purpose == purpose)
            .or_else(|| request.iter().find(|p| names.contains(&p.path.as_str())))
            .or_else(|| {
                request
                    .iter()
                    .find(|p| typed(p, ty) && p.purpose.is_empty())
            })
            .map(|p| p.path.clone())
    };
    let field = |purpose: &str| {
        response
            .iter()
            .find(|p| p.purpose == purpose)
            .map(|p| p.path.clone())
    };
    let items = field("items").or_else(|| {
        response
            .iter()
            .find(|p| !p.path.contains('.') && typed(p, "array"))
            .map(|p| p.path.clone())
    });
    let has_more = field("has_next_page").or_else(|| {
        response
            .iter()
            .find(|p| {
                ["has_more", "has_next_page"].contains(&p.path.as_str()) && typed(p, "boolean")
            })
            .map(|p| p.path.clone())
    });
    let mut rule = Pagination {
        items: items.clone().filter(|i| i != "data"),
        has_more,
        ..Pagination::default()
    };
    let mut used = vec!["items", "has_next_page"];
    match kind {
        "cursor" => {
            rule.cursor = param(
                "next_cursor_param",
                &["cursor", "after", "starting_after", "page_token"],
                "string",
            );
            rule.next_cursor = field("next_cursor_field").or_else(|| {
                response
                    .iter()
                    .find(|p| {
                        ["next_cursor", "next_page_token", "cursor"].contains(&p.path.as_str())
                    })
                    .map(|p| p.path.clone())
            });
            if rule.next_cursor.is_none() {
                return Err("no `next_cursor_field` in the response".into());
            }
            used.extend(["next_cursor_param", "next_cursor_field"]);
        }
        "cursor_id" => {
            rule.cursor = param(
                "next_cursor_id_param",
                &["after", "starting_after", "cursor"],
                "string",
            );
            let item = items
                .as_deref()
                .and_then(|items| response.iter().find(|p| p.path == items))
                .map(|p| &p.schema["items"]["properties"]);
            let mut fields = Vec::new();
            if let Some(item) = item {
                properties(item, "", &mut fields);
            }
            rule.item_cursor = Some(
                fields
                    .iter()
                    .find(|p| p.purpose == "cursor_item_id")
                    .map_or_else(|| "id".to_owned(), |p| p.path.clone()),
            );
            rule.before = request
                .iter()
                .find(|p| p.purpose == "previous_cursor_id_param" && typed(p, "string"))
                .map(|p| p.path.clone());
            used.extend([
                "next_cursor_id_param",
                "cursor_item_id",
                "previous_cursor_id_param",
            ]);
        }
        "offset" => {
            rule.offset = param("offset_count_param", &["offset", "skip"], "integer");
            rule.total = field("offset_total_count_field");
            used.extend(["offset_count_param", "offset_total_count_field"]);
        }
        "page_number" => {
            rule.page = param("page_number_param", &["page", "page_number"], "integer");
            rule.total_pages = field("total_page_count_field");
            used.extend(["page_number_param", "total_page_count_field"]);
        }
        "cursor_url" => {
            return Err(
                "perseid has no pagination by next-page URL: its list methods return the first page"
                    .into(),
            );
        }
        "fake_page" => {
            return Err(
                "an unpaginated list: perseid returns the response body, whose items are a property of it"
                    .into(),
            );
        }
        other => return Err(format!("unknown pagination type `{other}`")),
    }
    if rule.cursor.is_none() && rule.page.is_none() && rule.offset.is_none() {
        return Err("no query parameter selects the page".into());
    }
    if let Some(param) = rule
        .cursor
        .iter()
        .chain(&rule.page)
        .chain(&rule.offset)
        .next()
        && param.contains('.')
    {
        return Err(format!(
            "`{param}` is a nested parameter: perseid pages with a top-level query parameter"
        ));
    }
    let ignored = request
        .iter()
        .map(|p| ("request", p))
        .chain(response.iter().map(|p| ("response", p)))
        .filter(|(_, p)| !p.purpose.is_empty() && !used.contains(&p.purpose))
        .map(|(side, p)| {
            let why = match p.purpose {
                purpose if purpose.starts_with("previous_") => {
                    "perseid pages backwards only from an item cursor"
                }
                _ => "perseid doesn't need it to page",
            };
            (
                format!("{side}.{}", p.path),
                format!("{} ({why})", p.purpose),
            )
        })
        .collect();
    Ok((rule, ignored))
}

/// The SDKs a resource or method is left out of, on top of its parent's.
fn skipped_in(node: &Value, parent: &BTreeSet<&'static str>) -> BTreeSet<&'static str> {
    let mut out = parent.clone();
    let languages = |list: &Value| -> BTreeSet<&'static str> {
        list.as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| language(l.as_str()?))
            .collect()
    };
    match &node["skip"] {
        Value::Bool(true) => out.extend(LANGUAGES),
        list @ Value::Array(_) => out.extend(languages(list)),
        _ => {}
    }
    if node["only"].is_array() {
        let only = languages(&node["only"]);
        out.extend(LANGUAGES.into_iter().filter(|l| !only.contains(l)));
    }
    out
}

/// The generated operations, with their resource.
fn operations(api: &crate::api::Api) -> Vec<(String, &Operation)> {
    let mut out = Vec::new();
    let mut stack: Vec<&crate::api::Resource> = api.resources.values().collect();
    while let Some(resource) = stack.pop() {
        stack.extend(resource.subresources.values());
        for op in resource.operations.iter().filter(|op| !op.stream) {
            out.push((resource.name.clone(), op));
        }
    }
    out
}

/// Drops the names that two methods of a perseid resource would share.
fn drop_clashes(operations: &[(String, &Operation)], names: &mut BTreeMap<String, String>) {
    loop {
        let mut seen: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
        for (resource, op) in operations {
            let name = names
                .get(&op.id)
                .map_or_else(|| snake(&op.name), |n| snake(n));
            seen.entry((resource.clone(), name))
                .or_default()
                .push(&op.id);
        }
        let clashing: Vec<String> = seen
            .values()
            .filter(|ids| ids.len() > 1)
            .flatten()
            .filter(|id| names.contains_key(**id))
            .map(|id| (*id).to_owned())
            .collect();
        if clashing.is_empty() {
            return;
        }
        for id in clashing {
            names.remove(&id);
        }
    }
}

/// Whether the methods of the Stainless resource `stainless` are those of the perseid resource
/// `perseid`: its last segment is named alike, or `<name>_api` as a type is named like it.
fn same_resource(stainless: &str, perseid: &str) -> bool {
    let name = snake(stainless.rsplit('.').next().unwrap_or(stainless));
    !stainless.starts_with('$') && (perseid == name || perseid == format!("{name}_api"))
}

/// Methods of Stainless resources, grouped by resource and by why they differ in perseid.
#[derive(Default)]
struct Groups(Vec<(String, String, Vec<String>)>);

impl Groups {
    fn add(&mut self, resource: &str, why: String, item: String) {
        match self
            .0
            .iter_mut()
            .find(|(r, w, _)| r == resource && *w == why)
        {
            Some((_, _, items)) => items.push(item),
            None => self.0.push((resource.to_owned(), why, vec![item])),
        }
    }

    fn count(&self) -> usize {
        self.0.iter().map(|(_, _, items)| items.len()).sum()
    }

    fn first(&self) -> Option<&String> {
        self.0.first()?.2.first()
    }

    /// `(key, why)` lines of the report, one per group.
    fn lines(self) -> impl Iterator<Item = (String, String)> {
        self.0.into_iter().map(|(resource, why, items)| {
            (
                format!("resources.{resource} ({})", count(items.len(), "method")),
                format!("{why}: {}", items.join(", ")),
            )
        })
    }
}

/// A path with its parameters unnamed, as `/users/{}`.
fn shape(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in path.trim_end_matches('/').chars() {
        match c {
            '{' => {
                in_param = true;
                out.push_str("{}");
            }
            '}' => in_param = false,
            c if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

/// Runs `f` without the warnings reading a spec logs.
fn quietly<T>(f: impl FnOnce() -> T) -> T {
    tracing::subscriber::with_default(tracing_subscriber::registry(), f)
}

fn count(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// A TOML key, quoted unless bare.
fn key(text: &str) -> String {
    match !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        true => text.to_owned(),
        false => quote(text),
    }
}

/// A TOML array of strings, one per line past three.
pub(crate) fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| quote(i)).collect();
    match quoted.len() {
        0..=3 => format!("[{}]", quoted.join(", ")),
        _ => format!("[\n  {},\n]", quoted.join(",\n  ")),
    }
}

fn unsupported_target(target: &str) -> String {
    match target {
        "kotlin" => "perseid generates no Kotlin SDK: Kotlin code can use the Java SDK".into(),
        "ruby" | "php" => format!("perseid generates no {} SDK", target.to_upper_camel_case()),
        "cli" => "perseid generates no CLI".into(),
        "terraform" => "perseid generates no Terraform provider".into(),
        "openapi" => "perseid publishes no processed spec".into(),
        "sql" => "perseid generates no SQL extension".into(),
        other => format!("perseid generates no `{other}` SDK"),
    }
}

fn target_option(option: &str) -> String {
    match option {
        "mcp_server" => "perseid generates no MCP server".into(),
        "node_migration" | "enable_v2" | "code_style" => {
            "perseid has one generator per language, without migration modes".into()
        }
        _ => "no perseid.toml equivalent".into(),
    }
}

fn resource_key(key: &str) -> &'static str {
    match key {
        "description" => "perseid documents resources from the spec's tags",
        "deprecated" => "perseid marks the operations `deprecated: true` in the spec",
        "standalone_api" | "use_namespace_in_type_names" => {
            "perseid names types after their schema, without resource namespaces"
        }
        "custom" => "write the code by hand next to the generated files: perseid keeps it",
        "mcp" | "terraform" | "cli" => "perseid generates no MCP server, Terraform provider or CLI",
        _ => "no perseid equivalent",
    }
}

fn method_key(key: &str) -> &'static str {
    match key {
        "positional_params" => {
            "perseid passes path parameters positionally, then the body and query as objects (TypeScript) or keyword arguments (Python)"
        }
        "body_param_name" => "perseid passes a body that is not an object as `body`",
        "unwrap_response" => "perseid methods return the whole response body",
        "streaming" => "perseid streams `text/event-stream` responses as the spec declares them",
        "default_request_options" => "per-method timeouts and retries: pass them on the call",
        "deprecated" => "perseid marks the operations `deprecated: true` in the spec",
        "docs" | "description" => {
            "perseid documents methods from the spec's summary and description"
        }
        "skip_test_reason" => "perseid's generated tests run against their own mock server",
        "mcp" | "cli" | "terraform" => "perseid generates no MCP server, CLI or Terraform provider",
        _ => "no perseid equivalent",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn client_names_follow_stainless_casing() {
        let none = json!({});
        assert_eq!(client_name("knock", &none), "Knock");
        assert_eq!(client_name("onebusaway-sdk", &none), "OnebusawaySDK");
        assert_eq!(client_name("Lightspark Grid", &none), "LightsparkGrid");
        let casings = json!({ "lumaai": { "pascal": "LumaAI" }, "qa": { "initialism": true } });
        assert_eq!(client_name("lumaai", &casings), "LumaAI");
        assert_eq!(client_name("qa-tools", &casings), "QATools");
    }

    #[test]
    fn timeouts_are_milliseconds_or_iso_durations() {
        assert_eq!(seconds(&json!(60000)), Some(60.0));
        assert_eq!(seconds(&json!("PT10M")), Some(600.0));
        assert_eq!(seconds(&json!("PT1M30S")), Some(90.0));
        assert_eq!(seconds(&json!("P1D")), Some(86_400.0));
        assert_eq!(seconds(&json!("10 minutes")), None);
    }

    #[test]
    fn pagination_schemes_become_rules() {
        let scheme = json!({
            "type": "cursor",
            "request": {
                "after": { "type": "string", "x-stainless-pagination-property": { "purpose": "next_cursor_param" } },
                "before": { "type": "string", "x-stainless-pagination-property": { "purpose": "previous_cursor_param" } },
                "page_size": { "type": "integer" }
            },
            "response": {
                "entries": { "type": "array", "items": { "type": "object" } },
                "page_info": { "type": "object", "properties": {
                    "after": { "type": "string", "x-stainless-pagination-property": { "purpose": "next_cursor_field" } }
                }}
            }
        });
        let (rule, ignored) = rule(&scheme).unwrap();
        assert_eq!(rule.cursor.as_deref(), Some("after"));
        assert_eq!(rule.items.as_deref(), Some("entries"));
        assert_eq!(rule.next_cursor.as_deref(), Some("page_info.after"));
        assert_eq!(
            ignored,
            [(
                "request.before".to_owned(),
                "previous_cursor_param (perseid pages backwards only from an item cursor)"
                    .to_owned()
            )]
        );
        let ids = json!({
            "type": "cursor_id",
            "request": {
                "after": { "type": "string" },
                "before": { "type": "string", "x-stainless-pagination-property": { "purpose": "previous_cursor_id_param" } },
                "limit": { "type": "integer" }
            },
            "response": { "data": { "type": "array" }, "has_more": { "type": "boolean" } }
        });
        let (rule, ignored) = super::rule(&ids).unwrap();
        assert_eq!(
            (
                rule.cursor.as_deref(),
                rule.item_cursor.as_deref(),
                rule.before.as_deref(),
                rule.items,
                rule.has_more.as_deref()
            ),
            (
                Some("after"),
                Some("id"),
                Some("before"),
                None,
                Some("has_more")
            )
        );
        assert!(ignored.is_empty(), "{ignored:?}");
        let offset = json!({
            "type": "offset",
            "request": { "offset": { "type": "integer", "x-stainless-pagination-property": "offset_count_param" } },
            "response": { "items": { "type": "array" }, "total": { "type": "integer", "x-stainless-pagination-property": { "purpose": "offset_total_count_field" } } }
        });
        let (rule, _) = super::rule(&offset).unwrap();
        assert_eq!(
            (rule.offset.as_deref(), rule.total.as_deref()),
            (Some("offset"), Some("total"))
        );
        assert!(super::rule(&json!({ "type": "cursor_url" })).is_err());
        let nested = json!({
            "type": "cursor",
            "request": { "query_options.cursor": { "type": "string", "x-stainless-pagination-property": { "purpose": "next_cursor_param" } } },
            "response": { "next_cursor": { "type": "string" }, "channels": { "type": "array" } }
        });
        assert!(super::rule(&nested).unwrap_err().contains("nested"));
    }

    #[test]
    fn skips_cascade_to_the_languages_left_out() {
        let all = skipped_in(&json!({ "skip": true }), &BTreeSet::new());
        assert_eq!(all.len(), LANGUAGES.len());
        let node = skipped_in(&json!({ "skip": ["node", "ruby"] }), &BTreeSet::new());
        assert_eq!(node, BTreeSet::from(["typescript"]));
        let only = skipped_in(&json!({ "only": ["python"] }), &node);
        assert!(!only.contains("python") && only.contains("go"));
    }

    #[test]
    fn production_repos_lose_their_branch_and_must_read_owner_name() {
        let import = Import::from_config(&json!({ "targets": {
            "typescript": { "production_repo": "AcmeOrg/acme-typescript#master" },
            "python": { "production_repo": "acme/py\\sdk" },
        }}));
        assert_eq!(
            import.targets["typescript"].repo.as_deref(),
            Some("AcmeOrg/acme-typescript")
        );
        assert_eq!(import.targets["python"].repo, None);
        let why = |key: &str| {
            import
                .skipped
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, why)| why.clone())
                .unwrap_or_default()
        };
        assert!(why("targets.typescript.production_repo").contains("not `master`"));
        assert!(why("targets.python.production_repo").contains("no GitHub owner/name"));
    }

    #[test]
    fn client_settings_map_the_env_prefix_and_leave_defaults_out() {
        let import = Import::from_config(&json!({ "client_settings": {
            "default_env_prefix": "ACME_",
            "default_timeout": 60000,
        }}));
        assert_eq!(import.env_prefix.as_deref(), Some("ACME"));
        assert_eq!(import.timeout, None);
        let array_format = import
            .skipped
            .iter()
            .find(|(k, _)| k == "query_settings.array_format");
        assert!(array_format.is_some_and(|(_, why)| why.contains("comma-separated")));
        let explicit =
            Import::from_config(&json!({ "query_settings": { "array_format": "repeat" } }));
        assert!(explicit.skipped.is_empty(), "{:?}", explicit.skipped);
    }

    #[test]
    fn a_bare_openapi_section_is_no_openapi_document() {
        let dir = tempfile::tempdir().unwrap();
        let config = "organization:\n  name: acme\nopenapi:\n  # transforms: []\n";
        std::fs::write(dir.path().join("stainless.yml"), config).unwrap();
        assert!(read("stainless.yml", dir.path()).is_ok());
        std::fs::write(dir.path().join("openapi.yml"), "openapi: 3.1.0\n").unwrap();
        assert!(read("openapi.yml", dir.path()).is_err());
    }

    #[test]
    fn client_and_nested_methods_rename_no_resource_named_otherwise() {
        let op = |id: &str, tag: &str| json!({ "operationId": id, "tags": [tag], "responses": { "204": { "description": "ok" } } });
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "Acme", "version": "1" },
            "paths": {
                "/users": { "get": op("listUsers", "users") },
                "/health": { "get": op("healthCheck", "users") },
                "/sessions/{id}/peers": {
                    "post": op("addSessionPeers", "sessions"),
                    "delete": op("removeSessionPeers", "sessions"),
                },
            },
        });
        let mut import = Import::from_config(&json!({ "resources": {
            "users": { "methods": { "all": "get /users" } },
            "$client": { "methods": { "health": "get /health" } },
            "sessions": { "subresources": { "peers": { "methods": {
                "add": "post /sessions/{session_id}/peers",
                "remove": "delete /sessions/{session_id}/peers",
            }}}},
        }}));
        import.with_spec(&spec.to_string());
        assert_eq!(
            import.methods,
            BTreeMap::from([("listUsers".to_owned(), "all".to_owned())])
        );
        let keys: Vec<&str> = import.skipped.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"resources.$client (1 method)"), "{keys:?}");
        assert!(
            keys.contains(&"resources.sessions.peers (2 methods)"),
            "{keys:?}"
        );
        assert!(import.exclude.is_empty() && import.warnings.is_empty());
    }

    #[test]
    fn nested_resources_rename_the_methods_of_the_perseid_resource_named_like_them() {
        let op = |id: &str, tag: &str| json!({ "operationId": id, "tags": [tag], "responses": { "204": { "description": "ok" } } });
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "Acme", "version": "1" },
            "paths": {
                "/workspaces/{id}/peers": { "post": op("getOrCreatePeer", "peers") },
                "/workspaces/{id}/sessions/{sid}/peers": { "post": op("addSessionPeers", "sessions") },
            },
        });
        let mut import = Import::from_config(
            &json!({ "resources": { "workspaces": { "subresources": {
                "peers": { "methods": { "get_or_create": "post /workspaces/{workspace_id}/peers" } },
                "sessions": { "subresources": { "peers": { "methods": {
                    "add": "post /workspaces/{workspace_id}/sessions/{session_id}/peers",
                }}}},
            }}}}),
        );
        import.with_spec(&spec.to_string());
        assert_eq!(
            import.methods,
            BTreeMap::from([("getOrCreatePeer".to_owned(), "get_or_create".to_owned())])
        );
        let keys: Vec<&str> = import.skipped.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            keys.contains(&"resources.workspaces.sessions.peers (1 method)"),
            "{keys:?}"
        );
    }

    #[test]
    fn a_config_written_for_another_spec_excludes_only_what_it_skips() {
        let op = |id: &str| json!({ "operationId": id, "responses": { "204": { "description": "ok" } } });
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "Acme", "version": "1" },
            "paths": {
                "/v3/users": { "get": op("listUsers") },
                "/v3/teams": { "get": op("listTeams") },
                "/v3/notify": { "post": op("notify") },
            },
        });
        let mut import = Import::from_config(&json!({
            "resources": { "users": { "methods": {
                "list": "get /v3/users",
                "get": "get /v2/users/{id}",
                "update": "put /v2/users/{id}",
                "delete": "delete /v2/users/{id}",
                "create": "post /v2/users",
            }}},
            "unspecified_endpoints": ["post /v3/notify"],
        }));
        import.with_spec(&spec.to_string());
        assert_eq!(import.exclude, ["notify"]);
        assert!(
            import
                .warnings
                .iter()
                .any(|w| w.contains("only 1 of the 5 endpoints")),
            "{:?}",
            import.warnings
        );
    }

    #[test]
    fn paths_match_whatever_their_parameters_are_named() {
        assert_eq!(
            shape("/v1/users/{user_id}/feeds/{id}/"),
            "/v1/users/{}/feeds/{}"
        );
    }
}
