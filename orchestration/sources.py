from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlsplit
from urllib.request import urlopen

from generate import VERSION
from .common import digest, json_bytes, run, write
from .config import Project, engine_fingerprint

MAX_SPEC_BYTES = 64 * 1024 * 1024


def git_url(repository: str, root: Path) -> str:
    if repository.startswith("-"):
        raise ValueError("Invalid Git repository")
    local = (root / repository).resolve()
    if local.exists():
        return str(local)
    if "://" in repository or repository.startswith("git@"):
        return repository
    if len(repository.split("/")) == 2:
        return f"https://github.com/{repository}.git"
    raise ValueError(
        f"Expected a Git URL, owner/repository, or existing path: {repository}"
    )


def read_spec(data: bytes):
    if len(data) > MAX_SPEC_BYTES:
        raise ValueError("Spec exceeds the 64 MiB limit")
    value = json.loads(data)
    if (
        not isinstance(value, dict)
        or not str(value.get("openapi", "")).startswith("3.")
        or not isinstance(value.get("info"), dict)
        or not isinstance(value.get("paths"), dict)
    ):
        raise ValueError("Expected an OpenAPI 3 JSON document with info and paths")
    return value


def sync(project: Project):
    source = project.data["source"]
    resolved = {}
    if "file" in source:
        path = (project.root / source["file"]).resolve()
        data = path.read_bytes()
        resolved = {"kind": "file", "file": source["file"]}
    elif "git" in source:
        path = source["path"]
        if Path(path).is_absolute() or ".." in Path(path).parts or ":" in path:
            raise ValueError("Git source path must be repository-relative")
        ref = source.get("ref", "HEAD")
        if ref.startswith("-"):
            raise ValueError("Invalid source Git ref")
        with tempfile.TemporaryDirectory(prefix="perseid-source-") as directory:
            run(["git", "init", "--quiet", directory])
            run(
                [
                    "git",
                    "fetch",
                    "--quiet",
                    "--depth=1",
                    git_url(source["git"], project.root),
                    ref,
                ],
                cwd=directory,
            )
            commit = run(
                ["git", "rev-parse", "FETCH_HEAD"], cwd=directory, capture=True
            ).strip()
            data = subprocess.check_output(
                ["git", "show", f"{commit}:{path}"], cwd=directory
            )
        resolved = {
            "kind": "git",
            "repository": source["git"],
            "commit": commit,
            "path": path,
        }
    elif "url" in source:
        parsed = urlsplit(source["url"])
        if parsed.scheme not in {"https", "http"} or parsed.username or parsed.password:
            raise ValueError("Spec URL must use HTTP(S), without embedded credentials")
        with urlopen(source["url"], timeout=60) as response:
            data = response.read(MAX_SPEC_BYTES + 1)
        resolved = {"kind": "url", "url": source["url"]}
    else:
        data = subprocess.check_output(source["command"], cwd=project.root)
        resolved = {"kind": "command", "command": source["command"]}
    read_spec(data)
    lock = {
        "schema": 1,
        "perseid_version": VERSION,
        "engine_sha256": engine_fingerprint(),
        "config_sha256": project.fingerprint(),
        "source": resolved,
        "snapshot": str(project.snapshot.relative_to(project.root)),
        "spec_sha256": digest(data),
    }
    # Validation completes before either file changes. A crash between these
    # replacements is detected by locked(), never accepted as a valid snapshot.
    write(project.snapshot, data)
    write(project.lock_path, json_bytes(lock))
    return lock


def locked(project: Project):
    if not project.lock_path.exists() or not project.snapshot.exists():
        raise ValueError(
            "No spec snapshot; run `perseid project sync` and commit the snapshot and lock"
        )
    lock = json.loads(project.lock_path.read_text())
    expected = {
        "schema": 1,
        "perseid_version": VERSION,
        "config_sha256": project.fingerprint(),
        "engine_sha256": engine_fingerprint(),
        "snapshot": str(project.snapshot.relative_to(project.root)),
        "spec_sha256": digest(project.snapshot.read_bytes()),
    }
    if any(lock.get(key) != value for key, value in expected.items()):
        raise ValueError(
            "Spec, configuration, or generator changed; run `perseid project sync` to refresh the lock"
        )
    return lock
