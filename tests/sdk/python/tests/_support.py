"""Generates the SDKs of the fixtures that the tests of this directory import."""

import atexit
import calendar
import importlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType

WORK = Path(tempfile.mkdtemp(prefix="perseid-python-tests-"))
atexit.register(shutil.rmtree, WORK, True)


def fixtures() -> Path:
    """The tests/fixtures directory of perseid."""
    path = os.environ.get("FIXTURES")
    assert path, "set FIXTURES to the perseid tests/fixtures directory"
    return Path(path)


def perseid(project: Path, *args: str) -> str:
    """Runs the `perseid` binary of the PATH in `project`, failing with its output."""
    done = subprocess.run(["perseid", *args], cwd=project, capture_output=True, text=True)
    assert done.returncode == 0, f"perseid {' '.join(args)} failed:\n{done.stdout}\n{done.stderr}"
    return done.stdout


def generate_fixture(fixture: str, name: str, base_url: str | None = "https://api.test") -> Path:
    """Generates the Python SDK of `tests/fixtures/<fixture>` as the client `name`.

    Returns the project directory, whose `python` directory holds the package.
    """
    project = WORK / name.lower()
    project.mkdir()
    shutil.copy(fixtures() / fixture, project / "openapi.yaml")
    url = f'base_url = "{base_url}"\n' if base_url else ""
    (project / "perseid.toml").write_text(
        f'spec = "openapi.yaml"\nname = "{name}"\nsdks = ["python"]\n{url}'
    )
    perseid(project, "generate")
    return project


def package_name(project: Path) -> str:
    """The name of the only package that `perseid generate` wrote to `project/python`."""
    packages = [p.name for p in (project / "python").iterdir() if (p / "models").is_dir()]
    assert len(packages) == 1, packages
    return packages[0]


def import_package(project: Path) -> ModuleType:
    """Imports the generated package of `project`, and its `models`."""
    name = package_name(project)
    sys.path.insert(0, str(project / "python"))
    try:
        importlib.import_module(f"{name}.models")
        return importlib.import_module(name)
    finally:
        sys.path.remove(str(project / "python"))


_INSTANT = re.compile(
    r"(\d{4})-(\d\d)-(\d\d)[Tt](\d\d):(\d\d):(\d\d)(?:\.(\d+))?([Zz]|[+-]\d\d:\d\d)"
)


def instant(text: str) -> tuple[int, int] | None:
    """An RFC 3339 date-time as UTC seconds and microseconds, `None` for any other text."""
    match = _INSTANT.fullmatch(text)
    if match is None:
        return None
    year, month, day, hour, minute, second = (int(group) for group in match.groups()[:6])
    micros = int((match.group(7) or "").ljust(6, "0")[:6])
    zone = match.group(8)
    offset = 0
    if zone not in ("Z", "z"):
        offset = (1 if zone[0] == "+" else -1) * (int(zone[1:3]) * 60 + int(zone[4:6]))
    return calendar.timegm((year, month, day, hour, minute, second)) - offset * 60, micros
