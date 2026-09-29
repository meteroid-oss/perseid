from __future__ import annotations

from contextlib import ExitStack
import json
import re
import subprocess
import tomllib

from .common import hooks, json_bytes, relative, run, write
from .sources import locked

SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def version_tuple(value):
    if not isinstance(value, str) or not SEMVER.fullmatch(value):
        raise ValueError(f"Expected a stable major.minor.patch version: {value!r}")
    return tuple(int(part) for part in value.split("."))


def next_version(current, requested):
    parts = list(version_tuple(current))
    if requested in {"major", "minor", "patch"}:
        index = {"major": 0, "minor": 1, "patch": 2}[requested]
        parts[index] += 1
        parts[index + 1 :] = [0] * (2 - index)
        return ".".join(map(str, parts))
    if version_tuple(requested) <= tuple(parts):
        raise ValueError(f"New version {requested} must be greater than {current}")
    return requested


def prepare_version(project, names, requested, notes):
    locked(project)
    selected = project.selected(names)
    versions = project.versions()
    if project.data.get("release", {}).get("policy", "lockstep") == "lockstep":
        if set(selected) != set(project.targets):
            raise ValueError("Lockstep releases must include every target")
        current = max((versions[n] for n in selected), key=version_tuple)
        value = next_version(current, requested)
        versions.update({n: value for n in selected})
    else:
        versions.update({n: next_version(versions[n], requested) for n in selected})
    previous = (
        json.loads(project.versions_path.read_text())
        if project.versions_path.exists()
        else {}
    )
    release_notes = previous.get("notes", {})
    if not notes.strip():
        raise ValueError("Provide release notes for review")
    for name in selected:
        release_notes[name] = notes.strip()
    state = {"schema": 1, "versions": versions, "notes": release_notes}
    write(project.versions_path, json_bytes(state))
    return state


def replace_toml_version(path, section, version):
    source = path.read_text()
    parsed = tomllib.loads(source)
    table = parsed
    for part in section.split("."):
        table = table.get(part, {})
    if not isinstance(table.get("version"), str):
        raise ValueError(
            f"{path} needs a literal [{section}].version or release.version_commands"
        )
    in_section = False
    result = []
    updated = False
    for line in source.splitlines(keepends=True):
        if match := re.match(r"^\s*\[([^\[\]]+)\]\s*(?:#.*)?$", line.rstrip()):
            in_section = match[1].strip() == section
        elif line.lstrip().startswith("["):
            in_section = False
        if in_section and re.match(r"^\s*version\s*=", line):
            line = re.sub(
                r"""(\bversion\s*=\s*)("[^"]*"|'[^']*')""",
                lambda m: m[1] + json.dumps(version),
                line,
                count=1,
            )
            updated = True
        result.append(line)
    if not updated:
        raise ValueError(
            f"Cannot update version in {path}; configure release.version_commands"
        )
    rendered = "".join(result)
    table = tomllib.loads(rendered)
    for part in section.split("."):
        table = table[part]
    if table["version"] != version:
        raise ValueError(f"Could not update {path}")
    write(path, rendered.encode())


def stamp_version(project, name, directory):
    target = project.targets[name]
    version = project.versions()[name]
    version_tuple(version)
    if commands := target.get("release", {}).get("version_commands"):
        hooks(commands, directory, version=version)
        return
    language = target.get("language", name)
    if language == "rust" and (directory / "Cargo.toml").exists():
        replace_toml_version(directory / "Cargo.toml", "package", version)
    elif language == "python" and (directory / "pyproject.toml").exists():
        replace_toml_version(directory / "pyproject.toml", "project", version)
    elif language == "typescript" and (directory / "package.json").exists():
        path = directory / "package.json"
        package = json.loads(path.read_text())
        package["version"] = version
        write(path, json_bytes(package))
    elif language == "java":
        raise ValueError(
            f"Target {name} needs release.version_commands for its Maven/Gradle layout"
        )
    # Go versions are tags. Lockfile updates and custom metadata belong in the
    # version_commands hook when the default manifest update is insufficient.


def release(project, names, *, dry_run=False):
    from .github import (
        delivery_checkout,
        git_output,
        github_repo,
        repository_url,
        tracked_changes,
    )
    from .workspace import groups, render

    lock = locked(project)
    if not project.versions_path.exists():
        raise ValueError(
            "Prepare and commit release metadata with `perseid project version` first"
        )
    state = json.loads(project.versions_path.read_text())
    # Require reviewed control inputs. Registry side effects may only use
    # committed configuration, snapshot, and release metadata.
    for path in (
        project.path,
        project.lock_path,
        project.snapshot,
        project.versions_path,
    ):
        relative_path = str(path.relative_to(project.root))
        committed = subprocess.check_output(
            ["git", "show", f"HEAD:{relative_path}"], cwd=project.root
        )
        if committed != path.read_bytes():
            raise ValueError(f"Commit release input before publishing: {relative_path}")
    results = []
    with ExitStack() as stack:
        pending = []
        for repository, selected in groups(project, names).items():
            repo = github_repo(repository_url(project, repository))
            for name in selected:
                target = project.targets[name]
                config = target.get("release", {})
                if not config.get("publish"):
                    raise ValueError(
                        f"Target {name} requires release.publish and release.is_published hooks"
                    )
                version = state["versions"][name]
                version_tuple(version)
                tag = config.get("tag", "{target}/v{version}").format(
                    target=name, version=version
                )
                run(["git", "check-ref-format", f"refs/tags/{tag}"], capture=True)
                root = stack.enter_context(
                    delivery_checkout(project, repository, [name])
                )
                existing = git_output(
                    root,
                    "ls-remote",
                    "--tags",
                    "origin",
                    f"refs/tags/{tag}",
                    f"refs/tags/{tag}^{{}}",
                )
                if existing:
                    # Resume at the immutable tagged commit even if the default
                    # branch has advanced since the interrupted publication.
                    run(
                        ["git", "fetch", "--quiet", "origin", f"refs/tags/{tag}"],
                        cwd=root,
                    )
                    run(
                        ["git", "checkout", "--quiet", "--detach", "FETCH_HEAD"],
                        cwd=root,
                    )
                commit = git_output(root, "rev-parse", "HEAD")
                render(project, root, [name], lock, checks=True)
                if tracked_changes(root):
                    raise ValueError(
                        f"{repo}/{name} does not match the reviewed release inputs; merge generated changes first"
                    )
                notes = state.get("notes", {}).get(name)
                if not notes:
                    raise ValueError(f"No reviewed release notes for {name}")
                directory = relative(
                    root, target.get("directory", target.get("language", name))
                )
                pending.append(
                    (
                        root,
                        directory,
                        repo,
                        name,
                        config,
                        version,
                        tag,
                        commit,
                        bool(existing),
                        notes,
                    )
                )
        tag_keys = [(p[2], p[6]) for p in pending]
        if len(tag_keys) != len(set(tag_keys)):
            raise ValueError(
                "Targets in the same repository need distinct release tags"
            )
        for (
            root,
            directory,
            repo,
            name,
            config,
            version,
            tag,
            commit,
            exists,
            notes,
        ) in pending:
            if dry_run:
                results.append(
                    {
                        "target": name,
                        "repository": repo,
                        "version": version,
                        "tag": tag,
                        "commit": commit,
                    }
                )
                continue
            if not exists:
                # Create the immutable tag before registry publication. Registry
                # probes make the remaining steps retryable if the process dies.
                run(["git", "push", "origin", f"{commit}:refs/tags/{tag}"], cwd=root)
            from generate import PERSEID_ROOT
            import os

            probe = subprocess.run(
                ["bash", "-e", "-o", "pipefail", "-c", config["is_published"]],
                cwd=directory,
                env={
                    **os.environ,
                    "PERSEID_VERSION": version,
                    "PERSEID_DIR": str(PERSEID_ROOT),
                },
            )
            if probe.returncode == 1:
                hooks([config["publish"]], directory, version=version)
                # A successful shell command alone is not evidence that the
                # package was published. The probe must confirm registry state.
                hooks([config["is_published"]], directory, version=version)
            elif probe.returncode != 0:
                raise ValueError(
                    f"Publication probe failed for {name} (exit {probe.returncode}); no publish attempted"
                )
            # Query via the list API so authentication/network failures cannot be
            # mistaken for a missing release. Tags identify per-target releases.
            existing = run(
                [
                    "gh",
                    "api",
                    "--paginate",
                    f"repos/{repo}/releases",
                    "--jq",
                    ".[].tag_name",
                ],
                capture=True,
            ).splitlines()
            if tag not in existing:
                notes_file = root.parent / f"{name}-notes.md"
                notes_file.write_text(notes)
                run(
                    [
                        "gh",
                        "release",
                        "create",
                        tag,
                        "--repo",
                        repo,
                        "--verify-tag",
                        "--title",
                        f"{name} {version}",
                        "--notes-file",
                        notes_file,
                    ]
                )
            results.append(
                {
                    "target": name,
                    "repository": repo,
                    "version": version,
                    "tag": tag,
                    "published": True,
                }
            )
    return results
