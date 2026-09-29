"""Regression coverage for configuration, overrides, and all preserved templates."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LANGUAGES = ("rust", "java", "typescript", "python", "go")


class GenerationTests(unittest.TestCase):
    def test_all_languages_and_runtime_overrides(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "codegen").mkdir()
            (root / "spec.json").write_text(
                json.dumps(
                    {
                        "openapi": "3.1.0",
                        "info": {"title": "Example", "version": "1"},
                        "paths": {
                            "/widgets": {
                                "get": {
                                    "tags": ["Widgets"],
                                    "operationId": "list_widgets",
                                    "parameters": [
                                        {
                                            "name": "get_if_exists",
                                            "in": "query",
                                            "schema": {"type": "boolean"},
                                        }
                                    ],
                                    "responses": {
                                        "200": {
                                            "description": "ok",
                                            "content": {
                                                "application/json": {
                                                    "schema": {
                                                        "$ref": "#/components/schemas/Widget"
                                                    }
                                                }
                                            },
                                        }
                                    },
                                }
                            }
                        },
                        "components": {
                            "schemas": {
                                "Widget": {
                                    "type": "object",
                                    "required": ["name"],
                                    "properties": {"name": {"type": "string"}},
                                }
                            }
                        },
                    }
                )
            )
            (root / "overrides/rust").mkdir(parents=True)
            (root / "overrides/rust/api_summary.rs.jinja").write_text(
                "// this file is @generated\n// override {{ sdk.client_name }}\n"
            )
            (root / "overrides/java/extensions").mkdir(parents=True)
            (root / "overrides/java/extensions/widgets.java").write_text(
                'public String extensionName() { return "{{ sdk.client_name }}"; }\n'
            )
            (root / "request.rs").write_text(
                "// this file is @generated\n// runtime override @@CLIENT_NAME@@\n"
            )
            config = """[global]
perseid_version = "0.1.0"
input_files = ["spec.json"]
template_overrides = "overrides"
[global.sdk]
client_name = "Example"
package_name = "example"
rust_crate = "example_rs"
java_package = "org.example"
default_base_url = "https://example.test"
user_agent_prefix = "example"
header_prefix = "example"
version = "1.0.0"
patch_nullable = false
"""
            ext = {
                "rust": "rs",
                "java": "java",
                "typescript": "ts",
                "python": "py",
                "go": "go",
            }
            for language in LANGUAGES:
                config += f'\n[{language}]\nruntime_output_dir = "{language}/runtime"\n'
                if language == "rust":
                    config += '[rust.runtime_overrides]\n"request.rs" = "request.rs"\n'
                tasks = ["api_resource", "api_summary", "component_type"]
                if language in ["rust", "python", "typescript"]:
                    tasks += ["component_type_summary"]
                if language == "java":
                    tasks += ["operation_options"]
                for task in tasks:
                    folder = "models" if task.startswith("component") else "api"
                    config += f'[[{language}.task]]\ntemplate = "{language}/{task}.{ext[language]}.jinja"\noutput_dir = "{language}/{folder}"\n'
            (root / "codegen/codegen.toml").write_text(config)
            binary = Path(os.environ.get("PERSEID_BIN", ROOT / "target/debug/perseid"))
            if not binary.exists():
                self.fail(
                    "Build Perseid with cargo build before running integration tests"
                )
            env = {**os.environ, "PERSEID_BIN": str(binary)}
            command = [
                "python3",
                str(ROOT / "generate.py"),
                "--config",
                str(root / "codegen/codegen.toml"),
                "--no-format",
            ]
            subprocess.run(command, env=env, check=True, capture_output=True)
            manifest = json.loads((root / "codegen/generated_files.json").read_text())
            self.assertEqual(set(manifest), set(LANGUAGES))
            self.assertIn("override Example", (root / "rust/api/mod.rs").read_text())
            self.assertIn(
                "runtime override Example",
                (root / "rust/runtime/request.rs").read_text(),
            )
            self.assertIn(
                'public String extensionName() { return "Example"; }',
                (root / "java/api/Widgets.java").read_text(),
            )
            for language, paths in manifest.items():
                self.assertTrue(paths)
                for path in paths:
                    source = (root / path).read_text()
                    self.assertNotIn("Meteroid", source)
                    self.assertNotIn("Hyperfluid", source)
                    self.assertNotIn("@@", source)
            before = {
                p: (root / p).read_bytes() for group in manifest.values() for p in group
            }
            subprocess.run(
                command + ["--language", "rust"],
                env=env,
                check=True,
                capture_output=True,
            )
            after = json.loads((root / "codegen/generated_files.json").read_text())
            self.assertEqual(manifest, after)
            self.assertTrue(
                all((root / p).read_bytes() == content for p, content in before.items())
            )
            # Legacy hooks must retain access to installed dependencies and .git.
            biome = root / "typescript/node_modules/.bin/biome"
            biome.parent.mkdir(parents=True)
            biome.write_text("#!/bin/sh\necho called >> biome-calls\n")
            biome.chmod(0o755)
            (root / ".git").mkdir()
            config = config.replace(
                "[typescript]\n",
                "[typescript]\nformat_commands = ['python3 \"$PERSEID_DIR/scripts/format_typescript.py\" typescript']\ncheck_commands = ['test -d .git && test -f typescript/node_modules/.bin/biome']\n",
            )
            (root / "codegen/codegen.toml").write_text(config)
            subprocess.run(
                [arg for arg in command if arg != "--no-format"]
                + ["--language", "typescript", "--check"],
                env=env,
                check=True,
                capture_output=True,
            )
            self.assertEqual(
                (root / "typescript/biome-calls").read_text(), "called\ncalled\n"
            )

            # Exercise a supported RON fixture; the inherited query-schema IR
            # serializer has a separate pre-existing roundtrip limitation.
            ron_spec = json.loads((root / "spec.json").read_text())
            ron_spec["paths"]["/widgets"]["get"].pop("parameters")
            (root / "ron-source.json").write_text(json.dumps(ron_spec))
            subprocess.run(
                [str(binary), "debug", "--input-file", str(root / "ron-source.json")],
                cwd=root,
                env=env,
                check=True,
                capture_output=True,
            )
            ron_config = config.replace(
                'input_files = ["spec.json"]', 'input_files = ["debug.ron"]'
            )
            (root / "codegen/codegen.toml").write_text(ron_config)
            completed = subprocess.run(command, env=env, capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr.decode())
            (root / "codegen/codegen.toml").write_text(config)
            subprocess.run(command, env=env, check=True, capture_output=True)

            # A new transport must fail clearly for targets without its runtime.
            source = json.loads((root / "spec.json").read_text())
            source["paths"]["/events"] = {
                "get": {
                    "tags": ["Widgets"],
                    "operationId": "events",
                    "responses": {
                        "200": {
                            "description": "events",
                            "content": {"text/event-stream": {}},
                        }
                    },
                }
            }
            (root / "spec.json").write_text(json.dumps(source))
            for language in ("java", "typescript", "python", "go"):
                failed = subprocess.run(
                    command + ["--language", language], env=env, capture_output=True
                )
                self.assertNotEqual(failed.returncode, 0)
                self.assertIn(b"currently require the Rust target", failed.stderr)
            self.assertTrue(
                all((root / p).read_bytes() == content for p, content in before.items())
            )

            # Failed rendering must not replace any already-generated SDK files.
            (root / "request.rs").write_text("// @generated\n@@MISSING@@\n")
            failed = subprocess.run(
                command + ["--language", "rust"], env=env, capture_output=True
            )
            self.assertNotEqual(failed.returncode, 0)
            self.assertTrue(
                all((root / p).read_bytes() == content for p, content in before.items())
            )
            self.assertEqual(
                after, json.loads((root / "codegen/generated_files.json").read_text())
            )


if __name__ == "__main__":
    unittest.main()
