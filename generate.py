#!/usr/bin/env python3
"""Generate configured SDKs using Perseid's shared templates and runtime sources."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

PERSEID_ROOT = Path(__file__).resolve().parent
VERSION = tomllib.loads((PERSEID_ROOT / "Cargo.toml").read_text())["package"]["version"]
LANGUAGES = ("rust", "java", "typescript", "python", "go")


def run(command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)


def safe_path(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()) or Path(relative).is_absolute():
        raise ValueError(f"Output path must stay inside the SDK repository: {relative}")
    return path


def render_runtime(source: str, sdk: dict) -> str:
    def replace(match):
        key = match[1].lower()
        if key not in sdk:
            raise ValueError(f"Missing SDK setting for runtime token {match[0]}")
        return str(sdk[key])

    return re.sub(r"@@([A-Z_]+)@@", replace, source)


def build_binary() -> Path:
    if binary := os.environ.get("PERSEID_BIN"):
        path = Path(binary).resolve()
    else:
        manifest = str(PERSEID_ROOT / "Cargo.toml")
        run(["cargo", "build", "--locked", "--manifest-path", manifest])
        metadata = json.loads(
            subprocess.check_output(
                [
                    "cargo",
                    "metadata",
                    "--no-deps",
                    "--format-version",
                    "1",
                    "--manifest-path",
                    manifest,
                ]
            )
        )
        path = (
            Path(metadata["target_directory"])
            / "debug"
            / ("perseid.exe" if os.name == "nt" else "perseid")
        )
    actual = subprocess.check_output([str(path), "--version"], text=True).strip()
    if actual != f"perseid {VERSION}":
        raise ValueError(
            f"Runtime/templates are Perseid {VERSION}, but binary reports {actual}"
        )
    return path


def load_manifest(path: Path) -> dict[str, list[str]]:
    return json.loads(path.read_text()) if path.exists() else {}


def generate(config_path: Path, languages: list[str], no_format: bool, check: bool):
    config = tomllib.loads(config_path.read_text())
    root = config_path.parent.parent.resolve()
    global_config = config["global"]
    if global_config["perseid_version"] != VERSION:
        raise ValueError(
            f"SDK pins Perseid {global_config['perseid_version']}; checkout is {VERSION}"
        )
    unknown = set(config) - {"global", *LANGUAGES}
    if unknown:
        raise ValueError(f"Unknown configuration sections: {sorted(unknown)}")
    selected = languages or [lang for lang in LANGUAGES if lang in config]
    if set(selected) - set(config):
        raise ValueError("A selected language is not configured in this repository")
    if not selected or not global_config["input_files"]:
        raise ValueError("Configure at least one language and one input spec")
    if global_config.get("prepare") and len(global_config["input_files"]) != 1:
        raise ValueError(
            "Spec preparation currently accepts one input spec per configuration"
        )
    sdk = dict(global_config["sdk"])
    required = {
        "client_name",
        "package_name",
        "rust_crate",
        "java_package",
        "default_base_url",
        "user_agent_prefix",
        "header_prefix",
        "patch_nullable",
    }
    if missing := required - sdk.keys():
        raise ValueError(f"Missing SDK configuration: {sorted(missing)}")
    os.environ["PERSEID_DIR"] = str(PERSEID_ROOT)
    if version_file := global_config.get("version_file"):
        sdk["version"] = safe_path(root, version_file).read_text().strip()
    manifest_path = root / "codegen/generated_files.json"
    previous = load_manifest(manifest_path)
    binary = build_binary()
    runtime_manifest = json.loads((PERSEID_ROOT / "runtime/manifest.json").read_text())
    results = {}
    with tempfile.TemporaryDirectory(prefix="perseid-") as temp:
        stage = Path(temp)
        templates = stage / "_templates"
        shutil.copytree(PERSEID_ROOT / "templates", templates)
        if overrides := global_config.get("template_overrides"):
            shutil.copytree(safe_path(root, overrides), templates, dirs_exist_ok=True)
        input_files = []
        for index, source in enumerate(global_config["input_files"]):
            source = safe_path(root, source)
            if preparation := global_config.get("prepare"):
                from scripts.prepare_spec import prepare

                destination = stage / f"_input_{index}.json"
                prepare(
                    source,
                    destination,
                    report=stage / "_coverage.json",
                    exclude_unsupported=preparation.get("exclude_unsupported", False),
                    normalize_tags=preparation.get("normalize_tags", False),
                )
                input_files.append(destination)
            else:
                input_files.append(source)
        for language in selected:
            lang = config[language]
            context = {**sdk, **lang.get("sdk", {})}
            context_path = stage / "_context.json"
            context_path.write_text(json.dumps(context))
            paths = []
            for task in lang["task"]:
                out = safe_path(stage, task["output_dir"])
                out.mkdir(parents=True, exist_ok=True)
                template = safe_path(
                    templates, task["template"].removeprefix("templates/")
                )
                command = [
                    str(binary),
                    "--context-file",
                    str(context_path),
                    "generate",
                    "--no-postprocess",
                    "--template",
                    str(template),
                    "--output-dir",
                    task["output_dir"],
                ]
                for source in input_files:
                    command += ["--input-file", str(source)]
                command += lang.get("extra_codegen_args", []) + task.get(
                    "extra_codegen_args", []
                )
                run(command, cwd=stage)
                paths += json.loads((stage / ".generated_paths.json").read_text())
            if runtime_dir := lang.get("runtime_output_dir"):
                overrides = lang.get("runtime_overrides", {})
                unknown = set(overrides) - set(runtime_manifest[language])
                if unknown:
                    raise ValueError(
                        f"Unknown runtime overrides for {language}: {sorted(unknown)}"
                    )
                for source_name, output_name in runtime_manifest[language].items():
                    source = (
                        safe_path(root, overrides[source_name])
                        if source_name in overrides
                        else PERSEID_ROOT / "runtime" / language / source_name
                    )
                    relative = str(
                        Path(runtime_dir) / render_runtime(output_name, context)
                    )
                    output = safe_path(stage, relative)
                    output.parent.mkdir(parents=True, exist_ok=True)
                    output.write_text(render_runtime(source.read_text(), context))
                    paths.append(relative)
            if len(paths) != len(set(paths)):
                raise ValueError(f"Duplicate generated paths for {language}")
            for path in paths:
                if "@generated" not in "\n".join(
                    safe_path(stage, path).read_text().splitlines()[:3]
                ):
                    raise ValueError(f"Missing generated marker: {path}")
            results[language] = sorted(paths)
        all_paths = [p for paths in results.values() for p in paths]
        if len(all_paths) != len(set(all_paths)):
            raise ValueError("Two languages generated the same output path")
        # Compilation/rendering must finish successfully before touching SDK output.
        for path in all_paths:
            destination = safe_path(root, path)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(safe_path(stage, path), destination)
        old_files = set()
        if isinstance(previous, dict):
            old_files = {p for lang in selected for p in previous.get(lang, [])}
        elif len(selected) == len([k for k in config if k in LANGUAGES]):
            # Migration from the legacy task-list manifest.
            old_files = {p for group in previous for p in group}
        retained = {
            p
            for lang, paths in (previous.items() if isinstance(previous, dict) else [])
            if lang not in selected
            for p in paths
        }
        for path in old_files - set(all_paths) - retained:
            target = safe_path(root, path)
            if target.exists() and "@generated" in "\n".join(
                target.read_text().splitlines()[:3]
            ):
                target.unlink()
        merged = {**(previous if isinstance(previous, dict) else {}), **results}
        manifest_path.write_text(json.dumps(merged, indent=2, sort_keys=True) + "\n")
        if (stage / "_coverage.json").exists():
            coverage = json.loads((stage / "_coverage.json").read_text())
            coverage["source"] = global_config["input_files"][0]
            (root / "codegen/coverage.json").write_text(
                json.dumps(coverage, indent=2) + "\n"
            )
    if not no_format:
        for language in selected:
            for command in config[language].get("format_commands", []):
                run(["bash", "-c", command], cwd=root)
    if check:
        for language in selected:
            for command in config[language].get("check_commands", []):
                run(["bash", "-c", command], cwd=root)
    print(
        f"Perseid {VERSION}: generated {len(all_paths)} files for {', '.join(selected)}",
        flush=True,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=Path("codegen/codegen.toml"))
    parser.add_argument("--language", action="append", choices=LANGUAGES, default=[])
    parser.add_argument(
        "--local",
        action="store_true",
        help="Compatibility flag; Perseid runs locally by default",
    )
    parser.add_argument("--no-format", action="store_true")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Run the SDK checks configured for selected languages",
    )
    parser.add_argument("--version", action="version", version=f"perseid {VERSION}")
    args = parser.parse_args()
    generate(args.config.resolve(), args.language, args.no_format, args.check)


if __name__ == "__main__":
    main()
