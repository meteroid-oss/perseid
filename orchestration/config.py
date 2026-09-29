from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import re
import tomllib

from generate import LANGUAGES, PERSEID_ROOT, VERSION
from .common import IGNORED, digest, json_bytes, relative


def keys(value: dict, allowed: set[str], where: str):
    if not isinstance(value, dict):
        raise ValueError(f"{where} must be a table")
    if unknown := value.keys() - allowed:
        raise ValueError(f"Unknown {where} settings: {sorted(unknown)}")


def identifier(value: str):
    if not isinstance(value, str) or not re.fullmatch(r"[a-z][a-z0-9_-]*", value):
        raise ValueError(f"Expected a lowercase identifier, got {value!r}")


def commands(value, where):
    if not isinstance(value, list) or not all(isinstance(c, str) and c for c in value):
        raise ValueError(f"{where} must be an array of shell commands")


@dataclass
class Project:
    path: Path
    data: dict

    @classmethod
    def load(cls, path: Path):
        path = path.resolve()
        data = tomllib.loads(path.read_text())
        keys(
            data,
            {"name", "perseid_version", "source", "sdk", "targets", "release"},
            "project",
        )
        identifier(data["name"])
        if data["perseid_version"] != VERSION:
            raise ValueError(
                f"Project pins Perseid {data['perseid_version']}; installed assets are {VERSION}"
            )
        source = data["source"]
        keys(
            source,
            {"file", "git", "ref", "path", "url", "command", "snapshot"},
            "source",
        )
        if len(source.keys() & {"file", "git", "url", "command"}) != 1:
            raise ValueError("Configure exactly one source: file, git, url, or command")
        if "git" in source and not source.get("path"):
            raise ValueError("A Git source requires path")
        if "git" not in source and source.keys() & {"ref", "path"}:
            raise ValueError("source.ref/path require source.git")
        if "command" in source and (
            not isinstance(source["command"], list)
            or not source["command"]
            or not all(isinstance(s, str) for s in source["command"])
        ):
            raise ValueError(
                "source.command must be an argument array; output JSON to stdout"
            )
        project = cls(path, data)
        if project.snapshot in {path, project.lock_path, project.versions_path} or set(
            project.snapshot.relative_to(project.root).parts
        ) & (IGNORED | {".perseid"}):
            raise ValueError(
                "Spec snapshot must not overwrite project configuration or lock"
            )
        if not data.get("targets"):
            raise ValueError("Configure at least one target")
        destinations = []
        for name, target in project.targets.items():
            identifier(name)
            keys(
                target,
                {
                    "language",
                    "directory",
                    "repository",
                    "branch",
                    "sdk",
                    "task",
                    "runtime_output_dir",
                    "runtime_overrides",
                    "template_overrides",
                    "prepare",
                    "extra_codegen_args",
                    "format_commands",
                    "check_commands",
                    "release",
                },
                f"target {name}",
            )
            language = target.get("language", name)
            if language not in LANGUAGES:
                raise ValueError(f"Unsupported target language: {language}")
            directory = target.get("directory", language)
            relative(Path("/destination"), directory)
            if set(Path(directory).parts) & (IGNORED | {".perseid"}):
                raise ValueError(
                    "Target directory cannot use a reserved metadata/build directory"
                )
            repository = target.get("repository", ".")
            if (
                not isinstance(repository, str)
                or not repository
                or repository.startswith("-")
            ):
                raise ValueError("Invalid destination repository")
            for repo, old_dir, branch in destinations:
                if repository == repo:
                    if branch != target.get("branch"):
                        raise ValueError(
                            "Targets in one repository must use the same base branch"
                        )
                    a, b = Path(old_dir), Path(directory)
                    if a.is_relative_to(b) or b.is_relative_to(a):
                        raise ValueError(
                            "Target directories in one repository must not overlap"
                        )
            destinations.append((repository, directory, target.get("branch")))
            for hook in ("format_commands", "check_commands"):
                commands(target.get(hook, []), hook)
            release = target.get("release", {})
            keys(
                release,
                {"version_commands", "publish", "is_published", "tag"},
                "target release",
            )
            commands(release.get("version_commands", []), "version_commands")
            for hook in ("publish", "is_published", "tag"):
                if hook in release and (
                    not isinstance(release[hook], str) or not release[hook]
                ):
                    raise ValueError(f"release.{hook} must be a nonempty string")
            if bool(release.get("publish")) != bool(release.get("is_published")):
                raise ValueError(
                    "Publishing requires both publish and is_published commands for safe retries"
                )
        keys(data.get("release", {}), {"policy", "initial_version"}, "release")
        if data.get("release", {}).get("policy", "lockstep") not in {
            "lockstep",
            "independent",
        }:
            raise ValueError("release.policy must be lockstep or independent")
        return project

    @property
    def root(self):
        return self.path.parent

    @property
    def targets(self):
        return self.data["targets"]

    @property
    def snapshot(self):
        return relative(
            self.root, self.data["source"].get("snapshot", "spec/openapi.json")
        )

    @property
    def lock_path(self):
        return self.root / "perseid.lock.json"

    @property
    def versions_path(self):
        return self.root / "perseid.versions.json"

    def versions(self):
        initial = self.data.get("release", {}).get("initial_version", "0.1.0")
        versions = {name: initial for name in self.targets}
        if self.versions_path.exists():
            versions.update(json.loads(self.versions_path.read_text())["versions"])
        return versions

    def selected(self, names):
        if unknown := set(names) - self.targets.keys():
            raise ValueError(f"Unknown targets: {sorted(unknown)}")
        return names or list(self.targets)

    def fingerprint(self):
        return digest(json_bytes(self.data))

    def sdk(self, name):
        package = self.data["name"].replace("-", "_")
        client = "".join(word.capitalize() for word in re.split(r"[-_]", package))
        return {
            "client_name": client,
            "package_name": package,
            "rust_crate": package,
            "java_package": f"com.{package}",
            "default_base_url": "http://localhost",
            "user_agent_prefix": self.data["name"],
            "header_prefix": self.data["name"],
            "patch_nullable": False,
            **self.data.get("sdk", {}),
            **self.targets[name].get("sdk", {}),
            "version": self.versions()[name],
        }

    def generator_config(self, name):
        target = self.targets[name]
        language = target.get("language", name)
        sdk = self.sdk(name)
        directory = Path(target.get("directory", language))
        runtime = {
            "rust": directory / "src",
            "typescript": directory / "src",
            "python": directory / sdk["package_name"],
            "java": directory / "src/main/java" / sdk["java_package"].replace(".", "/"),
            "go": directory,
        }[language]
        extension = {
            "rust": "rs",
            "typescript": "ts",
            "python": "py",
            "java": "java",
            "go": "go",
        }[language]
        tasks = ["api_resource", "api_summary", "component_type"]
        if language in {"rust", "typescript", "python"}:
            tasks += ["component_type_summary"]
        if language == "java":
            tasks += ["operation_options"]
        outputs = []
        for task in tasks:
            folder = runtime
            if language != "go":
                if task.startswith("component"):
                    folder /= "models"
                elif task != "api_summary" or language in {"rust", "python"}:
                    folder /= "api"
            outputs.append(
                {
                    "template": f"{language}/{task}.{extension}.jinja",
                    "output_dir": str(folder),
                }
            )
        return {
            "global": {
                "perseid_version": VERSION,
                "input_files": [str(self.snapshot.relative_to(self.root))],
                "sdk": sdk,
                **{
                    k: target[k]
                    for k in ("prepare", "template_overrides")
                    if k in target
                },
            },
            language: {
                "task": outputs,
                "runtime_output_dir": str(runtime),
                **{
                    k: target[k]
                    for k in (
                        "task",
                        "runtime_output_dir",
                        "runtime_overrides",
                        "extra_codegen_args",
                    )
                    if k in target
                },
            },
        }


def engine_fingerprint():
    """Include implementation and assets, so a dirty checkout cannot impersonate a release."""
    paths = [
        PERSEID_ROOT / p
        for p in ("Cargo.toml", "Cargo.lock", "generate.py", "project.py")
    ]
    for folder in ("src", "templates", "runtime", "scripts", "orchestration"):
        paths += [
            p
            for p in (PERSEID_ROOT / folder).rglob("*")
            if p.is_file() and "__pycache__" not in p.parts and p.suffix != ".pyc"
        ]
    return digest(
        json_bytes(
            {
                str(p.relative_to(PERSEID_ROOT)): digest(p.read_bytes())
                for p in sorted(paths)
            }
        )
    )
