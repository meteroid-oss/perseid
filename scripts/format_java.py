#!/usr/bin/env python3
"""Format only Perseid-owned Java outputs with pinned google-java-format."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import urllib.request

VERSION = "1.25.2"
SHA256 = "25157797a0a972c2290b5bc71530c4f7ad646458025e3484412a6e5a9b8c9aa6"
root = Path.cwd()
jar = Path(
    os.environ.get(
        "PERSEID_JAVA_FORMAT_JAR",
        root / ".codegen-tmp" / f"google-java-format-{VERSION}.jar",
    )
)
if not jar.exists():
    jar.parent.mkdir(parents=True, exist_ok=True)
    urllib.request.urlretrieve(
        f"https://github.com/google/google-java-format/releases/download/v{VERSION}/google-java-format-{VERSION}-all-deps.jar",
        jar,
    )
if hashlib.sha256(jar.read_bytes()).hexdigest() != SHA256:
    raise SystemExit(f"Unexpected formatter checksum: {jar}")
manifest = json.loads((root / "codegen/generated_files.json").read_text())
files = [str(root / path) for path in manifest["java"] if path.endswith(".java")]
if files:
    subprocess.run(["java", "-jar", str(jar), "-i", "-a", *files], check=True)
