#!/usr/bin/env python3
"""Apply the inherited Biome import cleanup to Perseid-owned TypeScript outputs."""

import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(os.environ.get("PERSEID_REPOSITORY_ROOT", Path.cwd()))
package = (root / sys.argv[1]).resolve() if len(sys.argv) > 1 else Path.cwd()
manifest_path = Path(
    os.environ.get("PERSEID_GENERATED_MANIFEST", root / "codegen/generated_files.json")
)
manifest = json.loads(manifest_path.read_text())
files = [str(root / path) for path in manifest["typescript"] if path.endswith(".ts")]
if files:
    biome = package / "node_modules/.bin/biome"
    subprocess.run(
        [
            str(biome),
            "lint",
            "--only=organizeImports",
            "--only=noUnusedImports",
            "--only=useImportType",
            "--unsafe",
            "--write",
            *files,
        ],
        cwd=package,
        check=True,
    )
    subprocess.run([str(biome), "format", "--write", *files], cwd=package, check=True)
