from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


# Disposable dependency/build directories must not be copied or treated as SDK
# changes. Everything else, including handwritten sources, participates in checks.
IGNORED = {
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".gradle",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".perseid-work",
    "build",
    "dist",
    "coverage",
    ".coverage",
}


def run(args, *, cwd=None, capture=False, env=None):
    return subprocess.run(
        [str(arg) for arg in args],
        cwd=cwd,
        env={**os.environ, **(env or {})},
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    ).stdout


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def write(path: Path, data: bytes):
    """Replace one file atomically, and avoid changing mtime on no-op runs."""
    if path.exists() and path.read_bytes() == data:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        os.fchmod(fd, path.stat().st_mode & 0o777 if path.exists() else 0o644)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def relative(root: Path, path: str) -> Path:
    candidate = (root / path).resolve()
    if Path(path).is_absolute() or not candidate.is_relative_to(root.resolve()):
        raise ValueError(f"Path must stay inside its repository: {path}")
    return candidate


def hooks(commands, root: Path, *, version: str | None = None, extra_env=None):
    from generate import PERSEID_ROOT

    env = {"PERSEID_DIR": str(PERSEID_ROOT), **(extra_env or {})}
    if version is not None:
        env["PERSEID_VERSION"] = version
    for command in commands:
        run(["bash", "-e", "-o", "pipefail", "-c", command], cwd=root, env=env)
