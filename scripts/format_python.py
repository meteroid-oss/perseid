#!/usr/bin/env python3
"""Run the pinned Python SDK formatter on a supplied package directory."""

import shutil
import subprocess
import sys

VERSION = "0.14.10"
if shutil.which("uvx"):
    command = ["uvx", f"ruff@{VERSION}"]
elif shutil.which("ruff"):
    command = ["ruff"]
    actual = subprocess.check_output(["ruff", "--version"], text=True).strip()
    if actual != f"ruff {VERSION}":
        sys.exit(
            f"Expected ruff {VERSION}; found {actual}. Install uvx or the pinned ruff."
        )
else:
    sys.exit(f"Install uvx or ruff=={VERSION} to format generated Python code.")
target = sys.argv[1]
for args in [
    ["check", "--no-respect-gitignore", "--fix", "--quiet"],
    ["check", "--no-respect-gitignore", "--select", "I", "--fix", "--quiet"],
    ["format", "--no-respect-gitignore", "--quiet"],
]:
    subprocess.run([*command, *args, target], check=True)
