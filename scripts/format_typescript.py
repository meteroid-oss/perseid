#!/usr/bin/env python3
"""Apply the inherited Biome import cleanup to Perseid-owned TypeScript outputs."""

import json
from pathlib import Path
import subprocess
import sys

root = Path.cwd()
package = (root / sys.argv[1]).resolve()
manifest = json.loads((root / "codegen/generated_files.json").read_text())
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
