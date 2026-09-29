"""Optional Python entry-point compatibility; all generation runs in Rust."""

import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def binary():
    if path := os.environ.get("PERSEID_BIN"):
        return str(Path(path).resolve())
    manifest = str(ROOT / "Cargo.toml")
    subprocess.run(
        ["cargo", "build", "--locked", "--manifest-path", manifest], check=True
    )
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
    return str(
        Path(metadata["target_directory"])
        / "debug"
        / ("perseid.exe" if os.name == "nt" else "perseid")
    )


def run(arguments):
    return subprocess.run([binary(), *arguments]).returncode
