"""Native CLI lifecycle regressions. Python is only the test harness."""

import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("PERSEID_BIN", ROOT / "target/debug/perseid")).resolve()
SPEC = {
    "openapi": "3.1.0",
    "info": {"title": "Example", "version": "99"},
    "paths": {
        "/widgets": {
            "get": {
                "tags": ["Widgets"],
                "operationId": "list_widgets",
                "responses": {
                    "200": {
                        "description": "ok",
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/Widget"}
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


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="perseid-native-test-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.root = self.home / "controller"
        self.root.mkdir()
        (self.root / "source.json").write_text(json.dumps(SPEC))
        self.config = self.root / "perseid.toml"
        self.remotes = self.home / "remotes"
        self.remotes.mkdir()
        self.env = {
            **os.environ,
            "PERSEID_BIN": str(BINARY),
            "GIT_CONFIG_GLOBAL": str(self.home / "gitconfig"),
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        self.env.pop("PERSEID_DIR", None)
        self.git(
            self.root,
            "config",
            "--global",
            f"url.file://{self.remotes}/.insteadOf",
            "https://github.com/testing/",
        )
        self.configure("[targets.go]")

    def run_command(self, args, cwd=None, env=None, success=True):
        result = subprocess.run(
            list(map(str, args)),
            cwd=cwd or self.root,
            env=env or self.env,
            text=True,
            capture_output=True,
        )
        if success and result.returncode:
            self.fail(f"{args}\n{result.stdout}\n{result.stderr}")
        if not success:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def cli(self, *args, success=True, env=None):
        return self.run_command([BINARY, *args], env=env, success=success)

    def result(self, *args, **kwargs):
        return json.loads(self.cli(*args, **kwargs).stdout)

    def git(self, root, *args):
        return self.run_command(["git", *args], cwd=root).stdout.strip()

    def init_git(self, root):
        self.git(root, "init", "--quiet", "--initial-branch=main")
        self.git(root, "config", "user.name", "Test")
        self.git(root, "config", "user.email", "test@example.test")

    def commit(self, root, message="fixture"):
        self.git(root, "add", "--all")
        self.git(root, "commit", "--quiet", "-m", message)

    def configure(self, targets, source='file = "source.json"', extra=""):
        names = set(re.findall(r"\[targets\.([a-z_]+)", targets))
        for name in names:
            header = f"[targets.{name}]"
            if header not in targets:
                targets = header + "\nformat_commands = []\n" + targets
            else:
                start = targets.index(header) + len(header)
                end = targets.find("[", start)
                section = targets[start : end if end >= 0 else len(targets)]
                if "format_commands" not in section:
                    targets = (
                        targets[:start] + "\nformat_commands = []" + targets[start:]
                    )
        self.config.write_text(
            'name = "example"\nperseid_version = "0.1.0"\n'
            + extra
            + "\n[source]\n"
            + source
            + '\n[sdk]\ndefault_base_url = "https://example.test"\n'
            + targets
        )

    def remote(self, root=None, name="controller"):
        root = root or self.root
        self.init_git(root)
        (root / ".gitignore").write_text(".perseid-work/\n")
        self.commit(root)
        remote = self.remotes / f"{name}.git"
        self.run_command(["git", "clone", "--quiet", "--bare", root, remote])
        self.git(
            root, "remote", "add", "origin", f"https://github.com/testing/{name}.git"
        )
        return remote

    def fake_gh(self):
        folder = self.home / "bin"
        folder.mkdir(exist_ok=True)
        shutil.copy(ROOT / "tests/fixtures/fake_gh.py", folder / "gh")
        (folder / "gh").chmod(0o755)
        self.env["PATH"] = str(folder) + os.pathsep + self.env.get("PATH", "")
        self.env["FAKE_GH_STATE"] = str(self.home / "gh-state.json")

    def gh_state(self):
        return json.loads(Path(self.env["FAKE_GH_STATE"]).read_text())

    def snapshot(self):
        return {
            str(p.relative_to(self.root)): p.read_bytes()
            for p in self.root.rglob("*")
            if p.is_file() and ".git" not in p.parts and ".perseid-work" not in p.parts
        }

    def test_standalone_generation_has_no_interpreter_or_checkout_dependency(self):
        env = {**self.env, "PATH": str(self.home / "empty-bin")}
        binary = self.home / "perseid"
        shutil.copy(BINARY, binary)
        for command in ("sync", "generate", "check", "plan"):
            result = self.run_command([binary, command], env=env)
            self.assertEqual(result.returncode, 0)
        self.assertTrue((self.root / "go/request.go").exists())

    def test_file_snapshot_lock_idempotence_and_invalid_sync(self):
        lock = self.result("sync")
        mtime = (self.root / "perseid.lock.json").stat().st_mtime_ns
        self.assertEqual(lock, self.result("sync"))
        self.assertEqual(mtime, (self.root / "perseid.lock.json").stat().st_mtime_ns)
        self.assertEqual(self.result("plan")["targets"][0]["version"], "0.1.0")
        (self.root / "source.json").write_text("invalid")
        self.cli("sync", success=False)
        self.cli("plan")
        (self.root / "spec/openapi.json").write_text("{}")
        self.assertIn("changed", self.cli("generate", success=False).stderr)

    def test_git_source_records_resolved_commit(self):
        source = self.home / "backend"
        source.mkdir()
        self.init_git(source)
        (source / "openapi.json").write_text(json.dumps(SPEC))
        self.commit(source)
        self.configure(
            "[targets.go]",
            source=f'git = "{source}"\npath = "openapi.json"\nref = "main"',
        )
        lock = self.result("sync")
        self.assertEqual(
            lock["source"]["commit"], self.git(source, "rev-parse", "HEAD")
        )
        self.assertEqual(
            (self.root / "spec/openapi.json").read_bytes(),
            (source / "openapi.json").read_bytes(),
        )

    def test_http_and_exporter_sources(self):
        import http.server
        import threading

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(json.dumps(SPEC).encode())

            def log_message(self, *_):
                pass

        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                self.configure(
                    "[targets.go]",
                    source=f'url = "http://127.0.0.1:{server.server_port}/openapi.json"',
                )
                self.cli("sync")
            finally:
                server.shutdown()
                thread.join()
        self.configure("[targets.go]", source='command = ["cat", "source.json"]')
        self.cli("sync")
        self.assertEqual(
            json.loads((self.root / "spec/openapi.json").read_text()), SPEC
        )

    def test_target_path_and_config_validation(self):
        for invalid in (
            '[targets.go]\ndirectory = "../outside"',
            "[targets.go]\nformat_command = []",
            '[targets.go]\ndirectory = "sdk"\n[targets.other]\nlanguage = "go"\ndirectory = "temp/../sdk"',
            '[targets.go]\ndirectory = "sdk"\n[targets.other]\nlanguage = "go"\ndirectory = "./sdk"',
            '[targets.go]\ndirectory = "target"',
            '[targets.go.release]\npublish = "echo published"',
        ):
            with self.subTest(config=invalid):
                self.configure(invalid)
                self.cli("sync", success=False)

    def test_all_language_presets_and_readonly_drift_check(self):
        self.configure(
            "\n".join(
                f"[targets.{lang}]"
                for lang in ("rust", "java", "typescript", "python", "go")
            )
        )
        self.cli("sync")
        self.cli("generate")
        for path in (
            "rust/src/api/mod.rs",
            "java/src/main/java/com/example/Example.java",
            "typescript/src/index.ts",
            "python/example/api/__init__.py",
            "go/request.go",
        ):
            self.assertTrue((self.root / path).exists(), path)
        self.cli("check")
        path = self.root / "go/request.go"
        path.write_text(path.read_text() + "\n// drift\n")
        before = self.snapshot()
        result = self.result("check", "--target", "go", success=False)
        self.assertIn("go/request.go", result[0]["changed"])
        self.assertEqual(before, self.snapshot())

    def test_failed_check_does_not_apply_any_target(self):
        self.configure('[targets.go]\n[targets.python]\ncheck_commands = ["exit 42"]')
        self.cli("sync")
        before = self.snapshot()
        self.cli("generate", "--check", success=False)
        self.assertEqual(before, self.snapshot())

    def test_external_generation_and_manifest_ownership(self):
        sdk = self.home / "sdk"
        sdk.mkdir()
        (sdk / "README.md").write_text("Handwritten")
        self.remote(sdk, "sdk")
        self.configure('[targets.go]\nrepository = "testing/sdk"\ndirectory = "."')
        self.cli("sync")
        result = self.result("generate")
        destination = Path(result[0]["directory"])
        self.assertTrue((destination / "request.go").exists())
        self.assertFalse((sdk / "request.go").exists())
        self.cli("check")
        self.configure("[targets.go]")
        self.cli("sync")
        manifest = self.root / ".perseid/manifests/go.json"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('{"go":["neighbor/file.go"]}')
        self.assertIn("within", self.cli("generate", success=False).stderr)

    def test_override_hash_changes_target_receipt(self):
        self.configure(
            '[targets.go.runtime_overrides]\n"request.go" = "overrides/request.go"'
        )
        override = self.root / "overrides/request.go"
        override.parent.mkdir()
        override.write_text("// @generated\n// A\n")
        self.cli("sync")
        self.cli("generate")
        receipt = self.root / ".perseid/receipts/go.json"
        before = json.loads(receipt.read_text())["extensions_sha256"]
        override.write_text("// @generated\n// B\n")
        self.cli("generate")
        self.assertNotEqual(
            before, json.loads(receipt.read_text())["extensions_sha256"]
        )
        self.assertIn("// B", (self.root / "go/request.go").read_text())

    def test_relative_template_context_and_extra_file_confinement(self):
        self.configure('[targets.rust]\ntemplate_overrides = "overrides"')
        template = self.root / "overrides/rust/api_summary.rs.jinja"
        template.parent.mkdir(parents=True)
        template.write_text(
            '// @generated\n// {{ output_dir }}\n{% set _ = generate_extra_file(output_dir ~ "/custom.rs", "// @generated\\n") %}'
        )
        self.cli("sync")
        self.cli("generate")
        self.assertIn(
            "// rust/src/api", (self.root / "rust/src/api/mod.rs").read_text()
        )
        self.assertTrue((self.root / "rust/src/api/custom.rs").exists())
        self.cli("check")
        template.write_text(
            '// @generated\n{% set _ = generate_extra_file(output_dir ~ "/../outside.rs", "// @generated\\n") %}'
        )
        before = self.snapshot()
        self.cli("generate", success=False)
        self.assertEqual(before, self.snapshot())

    def test_versions_and_package_metadata(self):
        self.configure(
            "[targets.typescript]\n[targets.rust]",
            extra='[release]\npolicy = "lockstep"',
        )
        self.cli("sync")
        (self.root / "notes.md").write_text("New API")
        self.cli(
            "version", "minor", "--target", "rust", "--notes", "notes.md", success=False
        )
        state = self.result("version", "minor", "--notes", "notes.md")
        self.assertEqual(state["versions"], {"rust": "0.2.0", "typescript": "0.2.0"})
        (self.root / "typescript").mkdir()
        (self.root / "typescript/package.json").write_text(
            '{"name":"example","version":"0.1.0"}'
        )
        (self.root / "rust").mkdir()
        (self.root / "rust/Cargo.toml").write_text(
            '[package]\nname="example"\nversion = "0.1.0" # keep this\n'
        )
        (self.root / "rust/Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "example"\nversion = "0.1.0"\n[[package]]\nname = "dep"\nversion = "1.0.0"\nsource = "registry+https://example.test"\n'
        )
        (self.root / "typescript/package-lock.json").write_text(
            '{"version":"0.1.0","lockfileVersion":3,"packages":{"":{"name":"example","version":"0.1.0"},"node_modules/dep":{"version":"1.0.0"}}}'
        )
        self.cli("generate")
        self.assertIn(
            'version = "0.2.0" # keep this', (self.root / "rust/Cargo.toml").read_text()
        )
        self.assertIn('version = "0.2.0"', (self.root / "rust/Cargo.lock").read_text())
        lock = json.loads((self.root / "typescript/package-lock.json").read_text())
        self.assertEqual(lock["version"], "0.2.0")
        self.assertEqual(lock["packages"][""]["version"], "0.2.0")
        self.assertEqual(lock["packages"]["node_modules/dep"]["version"], "1.0.0")
        self.assertEqual(
            json.loads((self.root / "typescript/package.json").read_text())["version"],
            "0.2.0",
        )
        self.cli("check")

    def test_independent_versions(self):
        self.configure(
            "[targets.go]\n[targets.python]", extra='[release]\npolicy = "independent"'
        )
        self.cli("sync")
        (self.root / "notes.md").write_text("Fix")
        state = self.result("version", "patch", "--target", "go", "--notes", "notes.md")
        self.assertEqual(list(state["requests"]), ["go"])
        self.assertEqual(state["requests"]["go"]["bump"], "patch")
        self.cli("generate")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "0.1.1")

    def test_pr_dry_run_idempotence_and_grouping(self):
        self.configure("[targets.go]\n[targets.python]")
        self.cli("sync")
        remote = self.remote()
        self.fake_gh()
        before = self.snapshot()
        result = self.result("generate", "--pr", "--target", "go", "--dry-run")
        self.assertTrue(any(p.startswith("python/") for p in result[0]["changed"]))
        self.assertFalse(Path(self.env["FAKE_GH_STATE"]).exists())
        self.assertEqual(
            self.git(remote, "for-each-ref", "--format=%(refname)", "refs/heads"),
            "refs/heads/main",
        )
        first = self.result("generate", "--pr")
        second = self.result("generate", "--pr")
        self.assertEqual(first[0]["pr"], second[0]["pr"])
        self.assertFalse(second[0]["changed"])
        self.assertEqual(
            sum(c[:2] == ["pr", "create"] for c in self.gh_state()["calls"]), 1
        )
        self.assertEqual(before, self.snapshot())

    def test_delivery_rejects_symlinks_before_dry_run_writes(self):
        victim = self.home / "victim"
        victim.mkdir()
        (victim / "openapi.json").write_text("must survive")
        (self.root / "spec").symlink_to(victim, target_is_directory=True)
        self.remote()
        (self.root / "spec").unlink()
        self.cli("sync")
        self.fake_gh()
        result = self.cli("generate", "--pr", "--dry-run", success=False)
        self.assertIn("Symlink", result.stderr)
        self.assertEqual((victim / "openapi.json").read_text(), "must survive")

    def test_external_targets_also_propose_controller_changes(self):
        sdk = self.home / "sdk"
        sdk.mkdir()
        (sdk / "README.md").write_text("SDK")
        self.remote(sdk, "sdk")
        self.configure('[targets.go]\nrepository = "testing/sdk"\ndirectory = "."')
        self.remote()
        self.cli("sync")
        self.fake_gh()
        result = self.result("generate", "--pr", "--dry-run")
        self.assertEqual(len(result), 2)
        changes = [p for r in result for p in r["changed"]]
        self.assertIn("request.go", changes)
        self.assertIn("perseid.lock.json", changes)

    def test_manual_branch_commit_is_not_replaced(self):
        self.cli("sync")
        remote = self.remote()
        self.fake_gh()
        self.cli("generate", "--pr")
        branch = "perseid/example/update"
        self.git(self.root, "fetch", "--quiet", "origin", branch)
        self.git(self.root, "checkout", "--quiet", "-b", branch, "FETCH_HEAD")
        (self.root / "manual.txt").write_text("human change")
        self.commit(self.root, "Manual update")
        self.git(self.root, "push", "--quiet", "origin", branch)
        before = self.git(remote, "rev-parse", branch)
        self.assertIn("non-Perseid", self.cli("generate", "--pr", success=False).stderr)
        self.assertEqual(before, self.git(remote, "rev-parse", branch))

    def test_obsolete_pr_is_closed(self):
        self.cli("sync")
        self.remote()
        self.fake_gh()
        self.cli("generate", "--pr")
        self.git(self.root, "fetch", "--quiet", "origin", "perseid/example/update")
        self.git(self.root, "merge", "--quiet", "--ff-only", "FETCH_HEAD")
        self.git(self.root, "push", "--quiet", "origin", "main")
        self.assertIsNone(self.result("generate", "--pr")[0]["pr"])
        self.assertTrue(any(c[:2] == ["pr", "close"] for c in self.gh_state()["calls"]))

    def test_stale_owned_files_removed_handwritten_files_retained(self):
        self.cli("sync")
        self.cli("generate")
        path = self.root / ".perseid/manifests/go.json"
        manifest = json.loads(path.read_text())
        for name, content in [
            ("obsolete.go", "// @generated\n"),
            ("extension.go", "// handwritten\n"),
        ]:
            (self.root / "go" / name).write_text(content)
            manifest["go"].append("go/" + name)
        path.write_text(json.dumps(manifest))
        self.cli("generate")
        self.assertFalse((self.root / "go/obsolete.go").exists())
        self.assertTrue((self.root / "go/extension.go").exists())

    def test_builtin_formatters_use_only_owned_files_and_selected_languages(self):
        self.configure(
            "[targets.go]\n[targets.rust]\n[targets.java]\n[targets.python]\n[targets.typescript]"
        )
        self.config.write_text(
            self.config.read_text().replace("format_commands = []", "")
        )
        folder = self.home / "formatters"
        folder.mkdir()
        log = self.home / "formatters.jsonl"
        import sys

        for tool in ("gofmt", "rustfmt", "google-java-format", "ruff", "biome"):
            path = folder / tool
            path.write_text(
                f"#!{sys.executable}\nimport json,sys\nwith open({str(log)!r}, 'a') as f: f.write(json.dumps(sys.argv)+'\\n')\n"
            )
            path.chmod(0o755)
        self.env["PATH"] = str(folder)
        self.cli("sync")
        self.cli("generate")
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual(
            {Path(c[0]).name for c in calls},
            {"gofmt", "rustfmt", "google-java-format", "ruff", "biome"},
        )
        handwritten = self.root / "go/custom.go"
        handwritten.write_text("package example\n")
        log.write_text("")
        self.cli("generate", "--target", "go")
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual({Path(c[0]).name for c in calls}, {"gofmt"})
        self.assertFalse(any("custom.go" in arg for call in calls for arg in call))
        self.env["PATH"] = str(self.home / "empty")
        error = self.cli("generate", "--target", "go", success=False).stderr
        self.assertIn("Install gofmt on PATH", error)
        self.cli("generate", "--target", "go", "--no-format")

    def test_destination_manifest_is_authoritative_and_bumps_do_not_repeat(self):
        self.configure("[targets.typescript]")
        directory = self.root / "typescript"
        directory.mkdir()
        manifest = directory / "package.json"
        manifest.write_text('{"name":"example","version":"1.4.0"}')
        self.cli("sync")
        self.cli("generate")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "1.4.0")
        (self.root / "notes.md").write_text("Fix")
        self.cli("version", "patch", "--target", "typescript", "--notes", "notes.md")
        intent = (self.root / "perseid.releases.json").read_bytes()
        self.assertNotIn("versions", json.loads(intent))
        self.assertFalse((self.root / "perseid.versions.json").exists())
        self.cli("generate")
        self.cli("generate")
        self.assertEqual(json.loads(manifest.read_text())["version"], "1.4.1")
        # A manual release is adopted without editing controller metadata.
        manifest.write_text('{"name":"example","version":"1.5.0"}')
        self.cli("generate")
        self.cli("check")
        self.assertEqual(json.loads(manifest.read_text())["version"], "1.5.0")
        self.assertEqual(intent, (self.root / "perseid.releases.json").read_bytes())
        self.cli("version", "minor", "--target", "typescript", "--notes", "notes.md")
        self.cli("generate")
        self.assertEqual(json.loads(manifest.read_text())["version"], "1.6.0")
        latest = (self.root / "perseid.releases.json").read_bytes()
        (self.root / "perseid.releases.json").write_bytes(intent)
        self.assertIn("stale", self.cli("generate", success=False).stderr)
        self.assertEqual(json.loads(manifest.read_text())["version"], "1.6.0")
        (self.root / "perseid.releases.json").write_bytes(latest)
        self.cli("version", "1.2.0", "--target", "typescript", "--notes", "notes.md")
        self.assertIn("greater than", self.cli("generate", success=False).stderr)

    def test_go_versions_follow_manual_release_tags(self):
        self.cli("sync")
        self.init_git(self.root)
        self.commit(self.root)
        self.git(self.root, "tag", "go/v1.2.0")
        self.git(self.root, "tag", "unrelated/v9.0.0")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "1.2.0")
        (self.root / "notes.md").write_text("Fix")
        self.cli("version", "patch", "--notes", "notes.md")
        self.cli("generate")
        self.cli("check")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "1.2.1")
        self.commit(self.root)
        self.git(self.root, "tag", "go/v1.3.0")
        self.cli("generate")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "1.3.0")
        self.cli("version", "minor", "--notes", "notes.md")
        self.cli("generate")
        self.assertEqual(self.result("plan")["targets"][0]["version"], "1.4.0")

    def test_external_manual_release_is_used_by_fresh_pr_generation(self):
        sdk = self.home / "sdk"
        sdk.mkdir()
        (sdk / "package.json").write_text('{"name":"example","version":"2.1.0"}')
        remote = self.remote(sdk, "sdk")
        self.configure(
            '[targets.typescript]\nrepository = "testing/sdk"\ndirectory = "."'
        )
        self.cli("sync")
        self.remote()
        self.fake_gh()
        (self.root / "notes.md").write_text("API addition")
        self.cli("version", "minor", "--target", "typescript", "--notes", "notes.md")
        self.cli("generate", "--pr")
        branch = "perseid/example/update"
        manifest = json.loads(self.git(remote, "show", f"{branch}:package.json"))
        self.assertEqual(manifest["version"], "2.2.0")
        self.git(sdk, "fetch", "--quiet", "origin", branch)
        self.git(sdk, "merge", "--quiet", "--ff-only", "FETCH_HEAD")
        (sdk / "package.json").write_text('{"name":"example","version":"2.3.0"}')
        self.commit(sdk, "manual SDK release")
        self.git(sdk, "tag", "typescript/v2.3.0")
        self.git(sdk, "push", "--quiet", "origin", "main", "--tags")
        self.cli("generate", "--pr")
        self.assertEqual(
            json.loads(self.git(remote, "show", f"{branch}:package.json"))["version"],
            "2.3.0",
        )
        self.cli("version", "patch", "--target", "typescript", "--notes", "notes.md")
        self.cli("generate", "--pr")
        self.assertEqual(
            json.loads(self.git(remote, "show", f"{branch}:package.json"))["version"],
            "2.3.1",
        )

    def test_destination_overrides_are_discovered_and_hashed(self):
        self.configure("[targets.go]")
        folder = self.root / ".perseid"
        folder.mkdir()
        overrides = folder / "overrides.toml"
        overrides.write_text(
            '[targets.go]\nformat_commands = ["printf formatted > format-marker"]\n[targets.go.sdk]\nclient_name = "Custom"\n[targets.go.runtime_overrides]\n"request.go" = ".perseid/custom.go"\n'
        )
        (folder / "custom.go").write_text(
            "// @generated\npackage example\n// custom runtime\n"
        )
        self.cli("sync")
        self.cli("generate")
        self.assertEqual((self.root / "go/format-marker").read_text(), "formatted")
        self.assertIn("custom runtime", (self.root / "go/request.go").read_text())
        receipt = self.root / ".perseid/receipts/go.json"
        before = receipt.read_bytes()
        overrides.write_text(
            overrides.read_text().replace(
                'client_name = "Custom"', 'client_name = "Another"'
            )
        )
        self.cli("generate")
        self.assertNotEqual(before, receipt.read_bytes())
        overrides.write_text('[targets.go]\ndirectory = "outside"\n')
        self.cli("generate", success=False)

    def test_init_all_languages_and_conflicts(self):
        destination = self.home / "new-sdk"
        args = [
            "init",
            "--name",
            "example",
            "--output",
            str(destination),
            "--go-module",
            "github.com/testing/example/go",
        ]
        for language in ("rust", "java", "typescript", "python", "go"):
            args += ["--language", language]
        self.cli(*args)
        for path in (
            "rust/Cargo.toml",
            "rust/src/error.rs",
            "typescript/src/util.ts",
            "python/example/errors.py",
            "java/build.gradle",
            "go/errors.go",
            ".perseid/overrides.toml",
            "perseid.toml",
        ):
            self.assertTrue((destination / path).exists(), path)
        (destination / "openapi.json").write_text(json.dumps(SPEC))
        self.cli("sync", "--config", str(destination / "perseid.toml"))
        self.cli(
            "generate", "--no-format", "--config", str(destination / "perseid.toml")
        )
        self.cli(
            "generate", "--no-format", "--config", str(destination / "perseid.toml")
        )
        self.assertEqual((destination / "java/version.txt").read_bytes(), b"0.1.0\n")
        before = {str(p): p.read_bytes() for p in destination.rglob("*") if p.is_file()}
        self.assertIn("overwrite", self.cli(*args, success=False).stderr)
        self.assertEqual(
            before,
            {str(p): p.read_bytes() for p in destination.rglob("*") if p.is_file()},
        )
        self.cli(
            "init",
            "--name",
            "other",
            "--language",
            "go",
            "--output",
            str(self.home / "missing-module"),
            success=False,
        )
        self.assertFalse((self.home / "missing-module").exists())
        dedicated = self.home / "go-sdk"
        self.cli(
            "init",
            "--name",
            "example",
            "--language",
            "go",
            "--directory",
            ".",
            "--sdk-only",
            "--go-module",
            "github.com/testing/go-sdk",
            "--output",
            str(dedicated),
        )
        self.assertTrue((dedicated / "go.mod").exists())
        self.assertFalse((dedicated / "perseid.toml").exists())

    def prepare_release(self, probe, publish):
        self.configure(
            "[targets.go]\nformat_commands = []\n[targets.go.release]\npublish = "
            + json.dumps(publish)
            + "\nis_published = "
            + json.dumps(probe)
        )
        self.cli("sync")
        (self.root / "notes.md").write_text("Fix widget handling")
        self.cli("version", "patch", "--notes", "notes.md")
        self.cli("generate")
        remote = self.remote()
        self.fake_gh()
        return remote

    def test_release_retry_uses_tagged_commit_and_registry_probe(self):
        registry = self.home / "registry"
        count = self.home / "publish-count"
        remote = self.prepare_release(
            f'test -f "{registry}"', f'touch "{registry}"; echo published >> "{count}"'
        )
        self.assertEqual(self.result("release", "--dry-run")[0]["version"], "0.1.1")
        self.assertFalse(registry.exists())
        self.assertFalse(self.git(remote, "tag"))
        self.cli(
            "release", env={**self.env, "FAKE_GH_FAIL_RELEASE": "1"}, success=False
        )
        self.assertTrue(registry.exists())
        (self.root / "README.md").write_text("Later commit")
        self.commit(self.root, "advance main")
        self.git(self.root, "push", "--quiet", "origin", "main")
        self.cli("release")
        self.cli("release")
        self.assertEqual(count.read_text(), "published\n")
        self.assertEqual(
            self.gh_state()["releases"]["testing/controller"], ["go/v0.1.1"]
        )
        self.assertNotEqual(
            self.git(remote, "rev-parse", "main"),
            self.git(remote, "rev-parse", "go/v0.1.1"),
        )

    def test_old_release_recovers_after_new_request_and_manual_release(self):
        registry = self.home / "registry"
        count = self.home / "publish-count"
        self.prepare_release(
            f'test -f "{registry}"', f'touch "{registry}"; echo published >> "{count}"'
        )
        controller_a = self.git(self.root, "rev-parse", "HEAD")
        self.cli(
            "release", env={**self.env, "FAKE_GH_FAIL_RELEASE": "1"}, success=False
        )
        self.git(self.root, "fetch", "--quiet", "origin", "--tags")
        # A manual SDK release must not overwrite A's immutable release identity.
        (self.root / "README.md").write_text("manual release")
        self.commit(self.root)
        self.git(self.root, "tag", "go/v0.2.0")
        self.cli("generate")
        record = json.loads((self.root / ".perseid/releases/go.json").read_text())
        self.assertEqual(record["version"], "0.1.1")
        self.cli("version", "patch", "--notes", "notes.md")
        self.cli("generate")
        self.commit(self.root, "new release intent B")
        self.git(self.root, "push", "--quiet", "origin", "main", "--tags")
        # Rerunning workflow A still recovers its original tag, never bumps/publishes B.
        self.git(self.root, "checkout", "--quiet", "--detach", controller_a)
        result = self.result("release")
        self.assertEqual(result[0]["version"], "0.1.1")
        self.assertEqual(count.read_text(), "published\n")
        self.assertEqual(
            self.gh_state()["releases"]["testing/controller"], ["go/v0.1.1"]
        )

    def test_failed_registry_probe_prevents_publication(self):
        self.prepare_release("exit 2", "exit 99")
        self.assertIn("probe failed", self.cli("release", success=False).stderr)


if __name__ == "__main__":
    unittest.main()
