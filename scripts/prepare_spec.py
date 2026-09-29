"""Compatibility API for the native optional OpenAPI preparation adapter."""

import subprocess
from pathlib import Path
from scripts._compat import binary


def prepare(
    source: Path,
    destination: Path,
    *,
    report: Path,
    exclude_unsupported: bool = False,
    normalize_tags: bool = False,
) -> None:
    args = [
        binary(),
        "prepare",
        "--input",
        str(source),
        "--output",
        str(destination),
        "--report",
        str(report),
    ]
    if exclude_unsupported:
        args.append("--exclude-unsupported")
    if normalize_tags:
        args.append("--normalize-tags")
    subprocess.run(args, check=True)
