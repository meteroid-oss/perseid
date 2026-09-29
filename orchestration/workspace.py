from __future__ import annotations

from contextlib import contextmanager
import json
from pathlib import Path
import shutil
import tempfile

from generate import generate_config
from .common import IGNORED, digest, hooks, json_bytes, relative, run, write
from .config import Project
from .sources import git_url, locked


def files(root: Path):
    result = {}
    for entry in sorted(root.iterdir()):
        if entry.name in IGNORED:
            continue
        if entry.is_symlink():
            raise ValueError(f"Symlinks in SDK workspaces are unsupported: {entry}")
        if entry.is_dir():
            result.update(
                {f"{entry.name}/{name}": value for name, value in files(entry).items()}
            )
        elif entry.is_file():
            result[entry.name] = (
                digest(entry.read_bytes()),
                entry.stat().st_mode & 0o777,
            )
    return result


def changed(before, after):
    return sorted(
        p for p in before.keys() | after.keys() if before.get(p) != after.get(p)
    )


def groups(project: Project, names):
    result = {}
    for name in project.selected(names):
        repository = project.targets[name].get("repository", ".")
        result.setdefault(repository, []).append(name)
    return result


def checkout(project, repository, names, destination):
    target = project.targets[names[0]] if names else {}
    url = git_url(repository, project.root)
    command = ["git", "clone", "--quiet", "--single-branch"]
    if branch := target.get("branch"):
        if branch.startswith("-"):
            raise ValueError("Invalid destination branch")
        command += ["--branch", branch]
    command += ["--", url, destination]
    run(command)


def working_root(project, repository, names):
    if repository == ".":
        return project.root
    destination = project.root / ".perseid-work" / digest(repository.encode())[:16]
    if not destination.exists():
        destination.parent.mkdir(parents=True, exist_ok=True)
        checkout(project, repository, names, destination)
    # Reuse local work without resetting or discarding edits. PR/release operations
    # use fresh clones of the base branch instead of this convenience workspace.
    return destination


@contextmanager
def stage(root: Path):
    with tempfile.TemporaryDirectory(prefix="perseid-target-") as directory:
        destination = Path(directory) / "repo"
        files(root)  # Reject links before copying; never follow links outside root.
        shutil.copytree(root, destination, ignore=lambda _, names: set(names) & IGNORED)
        yield destination


def render(project, root, names, lock, no_format=False, checks=False):
    data = project.snapshot.read_bytes()
    if digest(data) != lock["spec_sha256"]:
        raise ValueError("Spec snapshot changed during generation")
    with tempfile.TemporaryDirectory(prefix="perseid-input-") as directory:
        source = Path(directory) / "openapi.json"
        source.write_bytes(data)
        _render(project, root, names, lock, source, no_format, checks)


def _render(project, root, names, lock, source, no_format, checks):
    from .releases import stamp_version

    for name in names:
        target = project.targets[name]
        directory = relative(
            root, target.get("directory", target.get("language", name))
        )
        directory.mkdir(parents=True, exist_ok=True)
        configuration = project.generator_config(name)
        language = target.get("language", name)
        manifest = f".perseid/manifests/{name}.json"
        previous = relative(root, manifest)
        previous_paths = []
        if previous.exists():
            previous_paths = json.loads(previous.read_text()).get(language, [])
        # Each target owns only its directory, including explicit task/runtime
        # overrides and stale paths loaded from an earlier manifest.
        output_paths = [task["output_dir"] for task in configuration[language]["task"]]
        output_paths += [configuration[language]["runtime_output_dir"], *previous_paths]
        if any(
            not relative(root, path).is_relative_to(directory) for path in output_paths
        ):
            raise ValueError(
                f"Target {name} output must stay within {directory.relative_to(root)}"
            )
        generate_config(
            configuration,
            root,
            [language],
            no_format=True,
            inputs=[source],
            manifest=manifest,
            coverage_file=f".perseid/manifests/{name}.coverage.json",
        )
        if (
            "release" in project.data
            or "release" in target
            or project.versions_path.exists()
        ):
            stamp_version(project, name, directory)
        hook_env = {
            "PERSEID_REPOSITORY_ROOT": str(root),
            "PERSEID_GENERATED_MANIFEST": str(root / manifest),
        }
        if not no_format:
            hooks(target.get("format_commands", []), directory, extra_env=hook_env)
        if checks:
            hooks(target.get("check_commands", []), directory, extra_env=hook_env)
        receipt = {
            "lock": lock,
            "target": name,
            "version": project.versions()[name],
        }
        write(root / f".perseid/receipts/{name}.json", json_bytes(receipt))


def apply(root, staged, before, changes):
    current = files(root)
    if any(current.get(path) != before.get(path) for path in changes):
        raise ValueError(
            "SDK files changed during generation; refusing to overwrite concurrent edits"
        )
    for path in changes:
        destination = relative(root, path)
        source = relative(staged, path)
        if source.exists():
            write(destination, source.read_bytes())
            destination.chmod(source.stat().st_mode & 0o777)
        else:
            destination.unlink(missing_ok=True)


def generate(project: Project, names, *, check=False, no_format=False, checks=False):
    lock = locked(project)
    from contextlib import ExitStack

    results = []
    # Stage every repository before applying any SDK changes. Rendering/formatting
    # or checks failing in a later target leave all user workspaces untouched.
    with ExitStack() as stack:
        pending = []
        for repository, selected in groups(project, names).items():
            root = working_root(project, repository, selected)
            before = files(root)
            staged = stack.enter_context(stage(root))
            render(project, staged, selected, lock, no_format, checks or check)
            changes = changed(before, files(staged))
            results.append(
                {
                    "repository": repository,
                    "directory": str(root),
                    "targets": selected,
                    "changed": changes,
                }
            )
            pending.append((root, staged, before, changes))
        if not check:
            # Check every destination for concurrent edits before the first write.
            for root, _, before, changes in pending:
                current = files(root)
                if any(current.get(path) != before.get(path) for path in changes):
                    raise ValueError("SDK files changed during generation")
            for args in pending:
                apply(*args)
    return results


def plan(project, names):
    lock = locked(project)
    return {
        "source": lock["source"],
        "spec_sha256": lock["spec_sha256"],
        "targets": [
            {
                "name": name,
                "language": target.get("language", name),
                "repository": target.get("repository", "."),
                "directory": target.get("directory", target.get("language", name)),
                "version": project.versions()[name],
            }
            for name in project.selected(names)
            for target in [project.targets[name]]
        ],
    }
