#!/usr/bin/env python3
"""Remove only marker-bearing files tracked by a client's generation manifest."""

import json
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
manifest = json.loads((root / "codegen/generated_files.json").read_text())
groups = manifest.values() if isinstance(manifest, dict) else manifest
for group in groups:
    for relative in group:
        path = (root / relative).resolve()
        if not path.is_relative_to(root) or Path(relative).is_absolute():
            raise SystemExit(f"Invalid generated path: {relative}")
        if path.exists():
            if "@generated" not in "\n".join(path.read_text().splitlines()[:3]):
                raise SystemExit(f"Refusing to remove a handwritten file: {relative}")
            path.unlink()
