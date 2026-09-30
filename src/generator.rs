use std::{collections::BTreeSet, str::FromStr};

use anyhow::{Context as _, bail, ensure};
use camino::{Utf8Path, Utf8PathBuf};
use fs_err as fs;
use heck::{ToLowerCamelCase, ToSnakeCase as _, ToUpperCamelCase as _};
use minijinja::{Template, context};
use serde::Deserialize;

use crate::{
    api::{
        Api, Resource, Types,
        types::{Type, TypeData},
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
}

pub(crate) fn generate_with_output_context(
    mut api: Api,
    tpl_name: String,
    output_dir: &Utf8Path,
    no_postprocess: bool,
    sdk: serde_json::Value,
    output_context: Option<&str>,
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

    if tpl_file_ext != "rs" {
        api.inline_aliases()?;
    }
    if tpl_file_ext == "java" {
        api.inline_string_alias_bodies()?;
    }
    if tpl_file_ext == "cs" {
        let streaming: Vec<&str> = api
            .resources
            .values()
            .flat_map(Resource::streaming_operations)
            .collect();
        ensure!(
            streaming.is_empty(),
            "multipart uploads, binary uploads and event streams are not supported in C# yet, \
             leave them out with `exclude = [\"{}\"]` in the [csharp] table",
            streaming.join("\", \"")
        );
        api.inline_flattened_fields()?;
    }

    let tpl_kind = match tpl_base_name {
        "api_resource" => TemplateKind::ApiResource,
        "operation_options" => TemplateKind::OperationOptions,
        "api_summary" | "component_type_summary" | "summary" => TemplateKind::Summary,
        "component_type" => TemplateKind::Type,
        _ => bail!(
            "template file basename must be one of 'api_resource', 'api_summary', \
             'component_type', 'component_type_summary', 'summary'",
        ),
    };

    let tpl_source = fs::read_to_string(tpl_path)?;

    let mut minijinja_env = template::env_with_dir(
        Utf8Path::new(tpl_path)
            .parent()
            .with_context(|| format!("invalid template path `{tpl_path}`"))?,
    )?;
    minijinja_env.add_global("sdk", minijinja::Value::from_serialize(sdk));
    minijinja_env.add_global(
        "output_dir",
        output_context.unwrap_or(output_dir.as_str()).to_owned(),
    );
    minijinja_env.add_global("_perseid_actual_output_dir", output_dir.as_str().to_owned());
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
    };

    if !no_postprocess {
        let postprocessor = Postprocessor::from_ext(tpl_file_ext, output_dir, &generated_paths);
        postprocessor.run_postprocessor()?;
    }

    Ok(generated_paths)
}

struct Generator<'a> {
    tpl: Template<'a, 'a>,
    tpl_file_ext: &'a str,
    output_dir: &'a Utf8Path,
}

impl Generator<'_> {
    fn generate_api_resources_options(self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        self.generate_api_resources_options_inner(api.resources.values())
    }

    fn generate_api_resources_options_inner<'a>(
        &self,
        resources: impl Iterator<Item = &'a Resource>,
    ) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let mut generated_paths = vec![];
        for resource in resources {
            let referenced_components = resource.referenced_components();
            for operation in &resource.operations {
                if operation.has_query_or_header_params() {
                    generated_paths.extend_from_slice(&self.render_tpl(
                        Some(&format!("{}_{}_Options", resource.name, operation.name)),
                        context! { operation, resource, referenced_components },
                    )?);
                }
            }

            generated_paths.extend_from_slice(
                &self.generate_api_resources_options_inner(resource.subresources.values())?,
            );
        }

        Ok(generated_paths)
    }

    fn generate_api_resources(self, api: Api) -> anyhow::Result<Vec<Utf8PathBuf>> {
        self.generate_api_resources_inner(api.resources.values())
    }

    fn generate_api_resources_inner<'a>(
        &self,
        resources: impl Iterator<Item = &'a Resource>,
    ) -> anyhow::Result<Vec<Utf8PathBuf>> {
        let mut generated_paths = vec![];

        for resource in resources {
            let referenced_components = resource.referenced_components();
            generated_paths.extend_from_slice(&self.render_tpl(
                Some(&resource.name),
                context! { resource, referenced_components },
            )?);
            generated_paths.extend_from_slice(
                &self.generate_api_resources_inner(resource.subresources.values())?,
            );
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
        for (name, ty) in &api.types {
            let mut referenced_components = ty.referenced_components();
            // A recursive type refers to itself, which is not an import.
            referenced_components.remove(name.as_str());
            let recursive_refs = recursive_refs(&api.types, ty);
            let patch_body = patch_bodies.contains(name.as_str());
            let inherited_fields = ty.inherited_fields(&api.types);
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
                    patch_body,
                    inherited_fields,
                    non_struct_refs,
                    output_dir,
                    type_names => type_names.clone(),
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

        generated_paths.push(file_path.clone());
        fs::write(&file_path, rendered_data)?;

        if let Some(extra_generated_file) = state.get_temp("extra_generated_file") {
            let extra_generated_filepath =
                Utf8PathBuf::from_str(extra_generated_file.as_str().unwrap())?;
            generated_paths.push(extra_generated_filepath);
        }

        Ok(generated_paths)
    }
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
fn go_file_stem(stem: String) -> String {
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
