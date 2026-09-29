"""Compile a generated synthetic SDK and exercise its real HTTP runtime."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RustTransportTests(unittest.TestCase):
    def test_generated_rust_sdk(self):
        binary = Path(os.environ.get("PERSEID_BIN", ROOT / "target/debug/perseid"))
        with tempfile.TemporaryDirectory(prefix="perseid-rust-") as directory:
            sdk = Path(directory) / "sdk"
            shutil.copytree(ROOT / "tests/fixtures/rust-sdk", sdk)
            env = {
                **os.environ,
                "PERSEID_BIN": str(binary),
                "CARGO_TARGET_DIR": os.environ.get(
                    "PERSEID_TEST_TARGET", str(ROOT / "target/rust-sdk")
                ),
            }
            subprocess.run(
                [
                    "python3",
                    str(ROOT / "generate.py"),
                    "--config",
                    str(sdk / "codegen/codegen.toml"),
                ],
                env=env,
                check=True,
            )
            report = json.loads((sdk / "codegen/coverage.json").read_text())
            self.assertEqual(report["included_operations"], 6)
            self.assertEqual(report["excluded_operations"], [])
            subprocess.run(
                [
                    "cargo",
                    "test",
                    "--locked",
                    "--manifest-path",
                    str(sdk / "Cargo.toml"),
                ],
                env=env,
                check=True,
            )
