"""Native CLI lifecycle regressions. Python is only the test harness."""

import json
import os
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
        self.cli("generate")
        self.assertIn(
            'version = "0.2.0" # keep this', (self.root / "rust/Cargo.toml").read_text()
        )
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
        self.assertEqual(state["versions"], {"go": "0.1.1", "python": "0.1.0"})

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

    def prepare_release(self, probe, publish):
        self.configure(
            "[targets.go.release]\npublish = "
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

    def test_failed_registry_probe_prevents_publication(self):
        self.prepare_release("exit 2", "exit 99")
        self.assertIn("probe failed", self.cli("release", success=False).stderr)


if __name__ == "__main__":
    unittest.main()
