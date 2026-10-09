use std::{collections::BTreeSet, str::FromStr};

use anyhow::{Context as _, bail};
use camino::{Utf8Path, Utf8PathBuf};
use fs_err as fs;
use heck::{ToLowerCamelCase, ToSnakeCase as _, ToUpperCamelCase as _};
use minijinja::{Template, context};
use serde::Deserialize;

use crate::{
    api::{
        Api, Resource, Types,
        resources::request_and_response_roots,
        types::{self, EnumVariantType, StructEnumRepr, Type, TypeData},
    },
    postprocessing::Postprocessor,
    template,
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TemplateKind {
    #[default]
    ApiResource,
    OperationOptions,
    Type,
    Summary,
    Test,
    TestSummary,
    RoundTrips,
}

/// The names a template's file may take, which say what it renders per.
pub(crate) const TEMPLATES: [&str; 10] = [
    "api_resource",
    "component_type",
    "operation_options",
    "api_summary",
    "component_type_summary",
    "api_reference",
    "summary",
    "api_test",
    "api_test_summary",
    "api_round_trips",
];

/// Renders `tpl_name` with the API model as the SDK of `language` (`rs`, `ts`, `py`...) sees it,
/// which is also the extension of the code it renders, unless it renders documentation.
/// `globals` reach the template by name: `sdk`, the SDK's context, and `pack` for a pack.
pub(crate) fn generate_with_output_context(
    mut api: Api,
    tpl_name: String,
    output_dir: &Utf8Path,
    no_postprocess: bool,
    globals: serde_json::Map<String, serde_json::Value>,
    output_context: Option<&str>,
    language: &str,
) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let (name_without_jinja_suffix, tpl_path) = match tpl_name.strip_suffix(".jinja") {
        Some(basename) => (basename, &tpl_name),
        None => (tpl_name.as_str(), &format!("{tpl_name}.jinja")),
    };

    let (tpl_base_name, tpl_file_ext) = Utf8Path::new(name_without_jinja_suffix)
        .file_name()
        .context("template name must not end in '/'")?
        .rsplit_once(".")
        .context("template name must contain '.'")?;

    let sdk = globals.get("sdk").cloned().unwrap_or_default();
    for_language(&mut api, &sdk, language)?;

    let tpl_kind = match tpl_base_name {
        "api_resource" => TemplateKind::ApiResource,
        "operation_options" => TemplateKind::OperationOptions,
        "api_summary" | "component_type_summary" | "api_reference" | "summary" => {
            TemplateKind::Summary
        }
        "api_test" => TemplateKind::Test,
        "api_test_summary" => TemplateKind::TestSummary,
        "api_round_trips" => TemplateKind::RoundTrips,
        "component_type" => TemplateKind::Type,
        _ => bail!(
            "template file basename must be one of {}",
            TEMPLATES.join(", ")
        ),
    };

    let tpl_source = fs::read_to_string(tpl_path)?;

    let mut minijinja_env = template::env_with_dir(
        Utf8Path::new(tpl_path)
            .parent()
            .with_context(|| format!("invalid template path `{tpl_path}`"))?,
    )?;
    for (name, value) in globals {
        minijinja_env.add_global(name, minijinja::Value::from_serialize(value));
    }
    minijinja_env.add_global(
        "output_dir",
        output_context.unwrap_or(output_dir.as_str()).to_owned(),
    );
    minijinja_env.add_global("_perseid_actual_output_dir", output_dir.as_str().to_owned());
    minijinja_env.add_global(
        "param_types",
        minijinja::Value::from_serialize(param_types(&api)),
    );
    let request_only: BTreeSet<String> = (request_only_schemas(&api).into_iter())
        .map(|name| name.to_upper_camel_case())
        .collect();
    minijinja_env.add_global(
        "request_only_types",
        minijinja::Value::from_serialize(request_only),
    );
    let tri_state: BTreeSet<String> = (tri_state_schemas(&api).into_iter())
        .map(|name| name.to_upper_camel_case())
        .collect();
    minijinja_env.add_global(
        "tri_state_types",
        minijinja::Value::from_serialize(tri_state),
    );
    minijinja_env.add_template(tpl_path, &tpl_source)?;
    let tpl = minijinja_env.get_template(tpl_path)?;

    fs::create_dir_all(output_dir)?;

    let generator = Generator {
        tpl,
        tpl_file_ext,
        output_dir,
    };

    let generated_paths = match tpl_kind {
        TemplateKind::OperationOptions => generator.generate_api_resources_options(api)?,
        TemplateKind::ApiResource => generator.generate_api_resources(api)?,
        TemplateKind::Type => generator.generate_types(api, output_dir)?,
        TemplateKind::Summary => generator.generate_summary(api)?,
        TemplateKind::Test => generator.generate_api_tests(api)?,
        TemplateKind::TestSummary => {
            // The resources with a test file, which some languages declare.
            let tested: Vec<&str> = (api.resources.values())
                .filter(|r| !crate::testcases::cases(&api.types, r).is_empty())
                .map(|r| r.name.as_str())
                .collect();
            match tested.is_empty() {
                true => vec![],
                false => generator.render_tpl(None, context! { api, tested })?,
            }
        }
        TemplateKind::RoundTrips => match api.types.is_empty() {
            true => vec![],
            false => generator.render_tpl(None, context! { types => api.types })?,
        },
    };

    if !no_postprocess {
        let postprocessor = Postprocessor::from_ext(tpl_file_ext, output_dir, &generated_paths);
        postprocessor.run_postprocessor()?;
    }

    Ok(generated_paths)
}

/// Shapes `api` as the templates of `language` (`rs`, `ts`...) receive it.
pub(crate) fn for_language(
    api: &mut Api,
    sdk: &serde_json::Value,
    language: &str,
) -> anyhow::Result<()> {
    if language != "rs" {
        api.inline_aliases()?;
    }
    api.settle_object_unions(sdk);
    if language == "java" {
        api.inline_string_alias_bodies()?;
    }
    if matches!(language, "cs" | "go" | "rs" | "py") {
        api.inline_flattened_fields(language != "py")?;
    }
    Ok(())
}

struct Generator<'a> {
    tpl: Template<'a, 'a>,
    tpl_file_ext: &'a str,
    output_dir: &'a Utf8Path,
}

impl Generator<'_> {
    fn generate_api_resources_options(self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let errors = &errors_context(&api);
        let mut generated_paths = vec![];
        for resource in api.resources.values() {
            let referenced_components = resource.referenced_components();
            for operation in &resource.operations {
                if operation.has_query_or_header_params() {
                    generated_paths.extend_from_slice(&self.render_tpl(
                        Some(&format!("{}_{}_Options", resource.name, operation.name)),
                        context! { operation, resource, referenced_components, ..errors.clone() },
                    )?);
                }
            }
        }
        Ok(generated_paths)
    }

    fn generate_api_resources(self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        // Every schema, for templates that read the fields of a request body.
        let types = minijinja::Value::from_serialize(&api.types);
        let shared = context! { types, ..errors_context(&api) };
        let mut generated_paths = vec![];
        for resource in api.resources.values() {
            let referenced_components = resource.referenced_components();
            generated_paths.extend_from_slice(&self.render_tpl(
                Some(&resource.name),
                context! { resource, referenced_components, ..shared.clone() },
            )?);
        }
        Ok(generated_paths)
    }

    fn generate_api_tests(self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let mut generated_paths = vec![];
        for resource in api.resources.values() {
            let cases = crate::testcases::cases(&api.types, resource);
            if !cases.is_empty() {
                generated_paths.extend_from_slice(
                    &self.render_tpl(Some(&resource.name), context! { resource, cases })?,
                );
            }
        }
        Ok(generated_paths)
    }

    fn generate_types(self, api: Api, output_dir: &Utf8Path) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let mut generated_paths = vec![];

        let output_dir = output_dir.as_str();
        let patch_bodies: BTreeSet<&str> = api
            .resources
            .values()
            .flat_map(Resource::patch_bodies)
            .collect();
        let type_names = minijinja::Value::from_serialize(
            api.types
                .keys()
                .map(|name| (name.to_upper_camel_case(), true))
                .collect::<std::collections::BTreeMap<_, _>>(),
        );
        let request_schemas = request_schemas(&api);
        let request_only_schemas = request_only_schemas(&api);
        let tri_state_schemas = tri_state_schemas(&api);
        // The enums with a `Literal` alias of their values (no schema takes its name), and the open ones.
        let enum_literals: BTreeSet<String> = (api.types.values())
            .filter(|t| {
                matches!(
                    t.data,
                    TypeData::StringEnum { .. } | TypeData::IntegerEnum { .. }
                )
            })
            .map(|t| t.name.to_upper_camel_case())
            .filter(|name| {
                !api.types
                    .keys()
                    .any(|k| k.to_upper_camel_case() == format!("{name}Literal"))
            })
            .collect();
        let open_enums: BTreeSet<String> = (api.types.values())
            .filter(|t| matches!(t.data, TypeData::StringEnum { open: true, .. }))
            .map(|t| t.name.to_upper_camel_case())
            .collect();
        let closed_enums: BTreeSet<String> = (api.types.values())
            .filter(|t| matches!(t.data, TypeData::StringEnum { open: false, .. }))
            .map(|t| t.name.to_upper_camel_case())
            .collect();
        let errors = errors_context(&api);
        let recursive_aliases = types::recursive_aliases(&api.types);
        let rust_sizes = template::rust::Sizes::new(&api.types);
        let convertible_types = match self.tpl_file_ext {
            "rs" => template::rust::convertible_types(&api.types),
            _ => BTreeSet::new(),
        };
        let alias_targets = match self.tpl_file_ext {
            "rs" => template::rust::alias_targets(&api.types, &recursive_aliases),
            _ => std::collections::BTreeMap::new(),
        };
        for (name, ty) in &api.types {
            let mut referenced_components = ty.referenced_components();
            // A recursive type refers to itself, which is not an import.
            referenced_components.remove(name.as_str());
            let recursive_refs = recursive_refs(&api.types, ty);
            let union_refs = ty.union_refs();
            let patch_body = patch_bodies.contains(name.as_str());
            let inherited_fields = ty.inherited_fields(&api.types);
            let declared_tags = declared_tags(&api.types, ty);
            let boxed_variants = match self.tpl_file_ext {
                "rs" => rust_sizes.boxed_variants(ty),
                _ => BTreeSet::new(),
            };
            // Type names, as templates render them, of the schemas `ty` embeds or unites that
            // are not objects (a union, say).
            let non_struct_refs: BTreeSet<String> = ty
                .direct_refs()
                .into_iter()
                .filter(|r| {
                    !matches!(
                        api.types.get(*r).map(|t| &t.data),
                        Some(TypeData::Struct { .. })
                    )
                })
                .map(|r| r.to_upper_camel_case())
                .collect();
            generated_paths.extend_from_slice(&self.render_tpl(
                Some(name),
                context! {
                    type => ty,
                    referenced_components,
                    recursive_refs,
                    recursive_alias => recursive_aliases.contains(name),
                    union_refs,
                    patch_body,
                    inherited_fields,
                    declared_tags,
                    boxed_variants,
                    non_struct_refs,
                    output_dir,
                    type_names => type_names.clone(),
                    is_error_schema => api.error_schemas.contains(name),
                    request_schema => request_schemas.contains(name.as_str()),
                    request_only => request_only_schemas.contains(name.as_str()),
                    tri_state => tri_state_schemas.contains(name.as_str()),
                    enum_literals => &enum_literals,
                    open_enums => &open_enums,
                    convertible_types => &convertible_types,
                    alias_targets => &alias_targets,
                    closed_enums => &closed_enums,
                    variant_tags => variant_tags(&api.types, ty),
                    ..errors.clone()
                },
            )?);
        }

        Ok(generated_paths)
    }

    fn generate_summary(&self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        self.render_tpl(None, context! { api })
    }

    fn render_tpl(
        &self,
        output_name: Option<&str>,
        ctx: minijinja::Value,
    ) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let mut generated_paths = vec![];

        let tpl_file_ext = self.tpl_file_ext;
        let basename = match (output_name, tpl_file_ext) {
            (Some(name), "ts") => name.to_lower_camel_case(),
            (Some(name), "cs" | "java" | "kt" | "php") => name.to_upper_camel_case(),
            (Some(name), "go") => go_file_stem(name.to_snake_case()),
            (Some(name), _) => name.to_snake_case(),
            (None, "py") => "__init__".to_owned(),
            (None, "rs") => "mod".to_owned(),
            (None, "cs" | "java" | "kt") => "Summary".to_owned(),
            (None, "ts") => "index".to_owned(),
            (None, "go") => "models".to_owned(),
            (None, "rb") => "client".to_owned(),
            (None, "php") => "Client".to_owned(),
            (None, "md") => "api".to_owned(),
            (None, _) => "summary".to_owned(),
        };

        let (rendered_data, state) = self.tpl.render_and_return_state(ctx)?;

        // Skip writing file if template output is empty (e.g., string_alias in Java)
        if rendered_data.trim().is_empty() {
            return Ok(generated_paths);
        }

        let file_path = match state.get_temp("summary_filename") {
            Some(summary_filename) => {
                let path = crate::fsx::relative(
                    self.output_dir.as_std_path(),
                    summary_filename
                        .as_str()
                        .context("Invalid summary filename")?,
                )?;
                Utf8PathBuf::from_path_buf(path)
                    .map_err(|_| anyhow::anyhow!("Non-UTF8 summary path"))?
            }
            None => self.output_dir.join(format!("{basename}.{tpl_file_ext}")),
        };

        // rustfmt refuses items it cannot fit in its width when they keep trailing whitespace.
        let rendered_data = match tpl_file_ext {
            "rs" => {
                rendered_data
                    .lines()
                    .map(str::trim_end)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            }
            _ => rendered_data,
        };
        generated_paths.push(file_path.clone());
        fs::write(&file_path, rendered_data)?;

        if let Some(extra_generated_files) = state.get_temp("extra_generated_file") {
            for file in extra_generated_files.as_str().unwrap().lines() {
                generated_paths.push(Utf8PathBuf::from_str(file)?);
            }
        }

        Ok(generated_paths)
    }
}

/// `error_schemas` and `default_error` of the API, for templates rendering a part of it.
fn errors_context(api: &Api) -> minijinja::Value {
    context! { error_schemas => api.error_schemas, default_error => api.default_error }
}

/// Schemas a request can carry: the ones operations send and every schema they reach.
fn request_schemas(api: &Api) -> BTreeSet<&str> {
    reachable(api, request_and_response_roots(&api.resources).0)
}

/// Schemas only requests carry: none the SDK decodes (responses, errors, events) reaches them,
/// nor any schema outside requests, such as a webhook payload.
fn request_only_schemas(api: &Api) -> BTreeSet<&str> {
    let requests = request_schemas(api);
    let mut received = request_and_response_roots(&api.resources).1;
    received.extend(
        (api.resources.values().flat_map(|r| &r.operations))
            .filter_map(|op| op.event_schema_name.as_deref()),
    );
    received.extend(
        api.types
            .keys()
            .map(String::as_str)
            .filter(|name| !requests.contains(name)),
    );
    let received = reachable(api, received);
    requests.difference(&received).copied().collect()
}

/// Schemas whose optional nullable fields tell `null` from absent: the request-only ones, where
/// sending `null` clears a value, and PATCH bodies, even when responses carry them too.
fn tri_state_schemas(api: &Api) -> BTreeSet<&str> {
    let mut schemas = request_only_schemas(api);
    schemas.extend(api.resources.values().flat_map(Resource::patch_bodies));
    schemas
}

/// The class names of the models requests carry that SDKs also take as their JSON, typed: Python's
/// `PetParam` dicts. Those are the structs, and the plain unions of them whose variants all hold
/// the discriminator, so that a dict names its variant.
fn param_types(api: &Api) -> BTreeSet<String> {
    let requests = request_schemas(api);
    let names: BTreeSet<String> = api.types.keys().map(|k| k.to_upper_camel_case()).collect();
    let free = |name: &str| !names.contains(&format!("{}Param", name.to_upper_camel_case()));
    let mut params = BTreeSet::new();
    for name in requests.iter().copied().filter(|n| free(n)) {
        // A `TypedDict` cannot name a key `""`.
        if let Some(TypeData::Struct { fields, .. }) = api.types.get(name).map(|t| &t.data)
            && !fields.iter().any(|f| f.flatten || f.name.is_empty())
        {
            params.insert(name.to_upper_camel_case());
        }
    }
    let holds = |variant: &str, field: &str| {
        matches!(
            api.types.get(variant).map(|t| &t.data),
            Some(TypeData::Struct { fields, .. }) if fields.iter().any(|f| f.name == field)
        )
    };
    for name in requests.iter().copied().filter(|n| free(n)) {
        let Some(TypeData::StructEnum {
            discriminator_field,
            repr: StructEnumRepr::InternallyTagged { variants },
            fields,
        }) = api.types.get(name).map(|t| &t.data)
        else {
            continue;
        };
        let refs: Vec<Option<&str>> = (variants.iter())
            .map(|v| match &v.content {
                EnumVariantType::Ref {
                    schema_ref: Some(target),
                    ..
                } => Some(target.as_str()),
                _ => None,
            })
            .collect();
        let distinct = refs.iter().collect::<BTreeSet<_>>().len() == refs.len();
        let typed = refs.iter().all(|r| {
            r.is_some_and(|r| {
                params.contains(&r.to_upper_camel_case()) && holds(r, discriminator_field)
            })
        });
        if fields.is_empty() && distinct && typed {
            params.insert(name.to_upper_camel_case());
        }
    }
    params
}

/// The tags the variants of the union `ty` declare instead of the one naming them, by the latter:
/// OpenAI's `InputMessage` variant is sent as `"type": "message"`.
fn declared_tags<'a>(
    types: &'a Types,
    ty: &'a Type,
) -> std::collections::BTreeMap<&'a str, &'a str> {
    let TypeData::StructEnum {
        discriminator_field,
        repr: StructEnumRepr::InternallyTagged { variants },
        ..
    } = &ty.data
    else {
        return Default::default();
    };
    (variants.iter())
        .filter_map(|v| match &v.content {
            EnumVariantType::Ref {
                schema_ref: Some(target),
                ..
            } => types::declared_tag(types, target, discriminator_field, &v.name)
                .map(|tag| (v.name.as_str(), tag)),
            _ => None,
        })
        .collect()
}

/// `roots` and every schema they reach.
fn reachable<'a>(api: &'a Api, roots: BTreeSet<&'a str>) -> BTreeSet<&'a str> {
    let mut stack: Vec<&str> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(name) = stack.pop() {
        if seen.insert(name) {
            let ty = api.types.get(name);
            stack.extend(ty.into_iter().flat_map(|ty| {
                let mut refs = ty.referenced_components();
                refs.extend(ty.union_refs());
                refs
            }));
        }
    }
    seen
}

/// How a struct variant of an internally tagged union carries its tag.
#[derive(serde::Serialize)]
struct VariantTag {
    /// The tag the struct declares, which several variants may share, else the variant's.
    tag: String,
    /// The struct's tag property is optional, for callers to leave out.
    optional: bool,
    /// The struct's required and known properties, to tell apart the variants of one tag.
    required: Vec<String>,
    known: Vec<String>,
}

/// The `VariantTag` of each struct variant of the internally tagged union `ty`, by variant name.
fn variant_tags(types: &Types, ty: &Type) -> std::collections::BTreeMap<String, VariantTag> {
    let TypeData::StructEnum {
        discriminator_field: field,
        repr: StructEnumRepr::InternallyTagged { variants },
        ..
    } = &ty.data
    else {
        return Default::default();
    };
    let mut tags = std::collections::BTreeMap::new();
    for variant in variants {
        let EnumVariantType::Ref {
            schema_ref: Some(target),
            ..
        } = &variant.content
        else {
            continue;
        };
        let (mut required, mut known, mut stack, mut seen) =
            (vec![], vec![], vec![target.as_str()], BTreeSet::new());
        while let Some(name) = stack.pop() {
            let Some(TypeData::Struct { fields, .. }) = types.get(name).map(|t| &t.data) else {
                continue;
            };
            for f in fields {
                match f.r#type.referenced_schema() {
                    Some(base) if f.flatten => {
                        if seen.insert(base) {
                            stack.push(base);
                        }
                    }
                    _ if f.flatten => {}
                    _ => {
                        known.push(f.name.clone());
                        if f.required {
                            required.push(f.name.clone());
                        }
                    }
                }
            }
        }
        let tag = types::declared_tag(types, target, field, &variant.name).unwrap_or(&variant.name);
        tags.insert(
            variant.name.clone(),
            VariantTag {
                tag: tag.to_owned(),
                optional: known.contains(field) && !required.contains(field),
                required,
                known,
            },
        );
    }
    tags
}

/// Schemas `ty` holds by value that lead back to it, so a language without indirection by
/// default (Rust) must box them.
fn recursive_refs<'a>(types: &'a Types, ty: &'a Type) -> BTreeSet<&'a str> {
    let leads_back = |start: &'a str| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(name) = stack.pop() {
            if name == ty.name {
                return true;
            }
            if seen.insert(name) {
                stack.extend(types.get(name).into_iter().flat_map(Type::direct_refs));
            }
        }
        false
    };
    ty.direct_refs()
        .into_iter()
        .filter(|r| leads_back(r))
        .collect()
}

/// Go only builds `x_windows.go`, `x_arm64.go` or `x_test.go` for that OS, arch or test run.
pub(crate) fn go_file_stem(stem: String) -> String {
    const CONSTRAINTS: &[&str] = &[
        "aix",
        "android",
        "darwin",
        "dragonfly",
        "freebsd",
        "hurd",
        "illumos",
        "ios",
        "js",
        "linux",
        "nacl",
        "netbsd",
        "openbsd",
        "plan9",
        "solaris",
        "wasip1",
        "windows",
        "zos",
        "386",
        "amd64",
        "amd64p32",
        "arm",
        "armbe",
        "arm64",
        "arm64be",
        "loong64",
        "mips",
        "mipsle",
        "mips64",
        "mips64le",
        "mips64p32",
        "mips64p32le",
        "ppc",
        "ppc64",
        "ppc64le",
        "riscv",
        "riscv64",
        "s390",
        "s390x",
        "sparc",
        "sparc64",
        "wasm",
        "test",
    ];
    match stem.rsplit_once('_') {
        Some((_, last)) if CONSTRAINTS.contains(&last) => format!("{stem}_gen"),
        _ => stem,
    }
}

#[cfg(test)]
mod tests {
    use super::go_file_stem;

    #[test]
    fn go_file_names_never_look_like_build_constraints() {
        assert_eq!(go_file_stem("usage_windows".into()), "usage_windows_gen");
        assert_eq!(go_file_stem("widget_test".into()), "widget_test_gen");
        assert_eq!(go_file_stem("windows".into()), "windows");
        assert_eq!(go_file_stem("usage_window".into()), "usage_window");
    }
}
