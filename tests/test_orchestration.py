"""Exercise lifecycle behavior with real generators and local Git repositories."""

from contextlib import ExitStack
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from orchestration.common import json_bytes, run
from orchestration.config import Project
from orchestration.github import propose
from orchestration.releases import prepare_version, release, replace_toml_version
from orchestration.sources import locked, sync
from orchestration.workspace import files, generate, plan

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


def git(root, *args):
    return run(["git", *args], cwd=root, capture=True).strip()


def init_git(root):
    git(root, "init", "--quiet", "--initial-branch=main")
    git(root, "config", "user.name", "Test")
    git(root, "config", "user.email", "test@example.test")


def commit(root, message="fixture"):
    git(root, "add", "--all")
    git(root, "commit", "--quiet", "-m", message)


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="perseid-test-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.root = self.home / "controller"
        self.root.mkdir()
        (self.root / "source.json").write_bytes(json_bytes(SPEC))
        self.config = self.root / "perseid.toml"
        self.configure("[targets.go]\n")

    def configure(self, targets, source='file = "source.json"', extra=""):
        self.config.write_text(
            'name = "example"\nperseid_version = "0.1.0"\n'
            + extra
            + "\n[source]\n"
            + source
            + '\n[sdk]\ndefault_base_url = "https://example.test"\n'
            + targets
        )
        self.project = Project.load(self.config)
        return self.project

    def setup_remote(self):
        init_git(self.root)
        (self.root / ".gitignore").write_text(".perseid-work/\n")
        commit(self.root)
        remote = self.home / "remote.git"
        run(["git", "clone", "--quiet", "--bare", self.root, remote])
        git(self.root, "remote", "add", "origin", str(remote))
        return remote

    def test_file_snapshot_is_locked_and_sync_is_idempotent(self):
        result = sync(self.project)
        self.assertEqual(result, locked(self.project))
        mtime = self.project.lock_path.stat().st_mtime_ns
        self.assertEqual(result, sync(self.project))
        self.assertEqual(mtime, self.project.lock_path.stat().st_mtime_ns)
        self.assertEqual(plan(self.project, [])["targets"][0]["version"], "0.1.0")
        (self.root / "source.json").write_text("invalid")
        with self.assertRaises(ValueError):
            sync(self.project)
        self.assertEqual(result, locked(self.project))
        self.project.snapshot.write_text("{}")
        with self.assertRaisesRegex(ValueError, "changed"):
            locked(self.project)

    def test_git_source_pins_commit_without_changing_exported_bytes(self):
        source = self.home / "backend"
        source.mkdir()
        init_git(source)
        (source / "openapi.json").write_bytes(json_bytes(SPEC))
        commit(source)
        self.configure(
            "[targets.go]\n",
            source=f'git = "{source}"\npath = "openapi.json"\nref = "main"',
        )
        result = sync(self.project)
        self.assertEqual(result["source"]["commit"], git(source, "rev-parse", "HEAD"))
        self.assertEqual(
            self.project.snapshot.read_bytes(), (source / "openapi.json").read_bytes()
        )

    def test_url_and_command_sources(self):
        import io

        self.configure(
            "[targets.go]\n", source='url = "https://example.test/openapi.json"'
        )
        with patch(
            "orchestration.sources.urlopen", return_value=io.BytesIO(json_bytes(SPEC))
        ):
            sync(self.project)
        self.configure(
            "[targets.go]\n",
            source='command = ["python3", "-c", "from pathlib import Path; print(Path(\'source.json\').read_text())"]',
        )
        sync(self.project)
        self.assertEqual(json.loads(self.project.snapshot.read_text()), SPEC)

    def test_configuration_validation(self):
        for invalid in (
            '[targets.go]\ndirectory = "../outside"',
            "[targets.go]\nformat_command = []",
            '[targets.go]\n[targets.other]\nlanguage = "go"\ndirectory = "go/nested"',
            '[targets.go.release]\npublish = "echo published"',
        ):
            with self.subTest(config=invalid), self.assertRaises(ValueError):
                self.configure(invalid)

    def test_all_language_presets_generate_and_check_detects_drift(self):
        self.configure(
            "\n".join(
                f"[targets.{lang}]"
                for lang in ("rust", "java", "typescript", "python", "go")
            )
        )
        sync(self.project)
        result = generate(self.project, [])
        self.assertTrue(result[0]["changed"])
        self.assertTrue((self.root / "rust/src/api/mod.rs").exists())
        self.assertTrue(
            (self.root / "java/src/main/java/com/example/Example.java").exists()
        )
        self.assertTrue((self.root / "typescript/src/index.ts").exists())
        self.assertTrue((self.root / "python/example/api/__init__.py").exists())
        self.assertTrue((self.root / "go/request.go").exists())
        self.assertFalse(generate(self.project, [], check=True)[0]["changed"])
        path = self.root / "go/request.go"
        path.write_text(path.read_text() + "\n// drift\n")
        before = files(self.root)
        result = generate(self.project, ["go"], check=True)
        self.assertIn("go/request.go", result[0]["changed"])
        self.assertEqual(before, files(self.root))

    def test_failed_check_does_not_apply_any_target(self):
        self.configure('[targets.go]\n[targets.python]\ncheck_commands = ["exit 42"]')
        sync(self.project)
        before = files(self.root)
        with self.assertRaises(subprocess.CalledProcessError):
            generate(self.project, [], checks=True)
        self.assertEqual(before, files(self.root))

    def test_external_repository_staging_and_target_ownership(self):
        sdk = self.home / "sdk"
        sdk.mkdir()
        init_git(sdk)
        (sdk / "README.md").write_text("Handwritten\n")
        commit(sdk)
        self.configure(f'[targets.go]\nrepository = "{sdk}"\ndirectory = "."')
        sync(self.project)
        results = generate(self.project, [])
        destination = Path(results[0]["directory"])
        self.assertTrue((destination / "request.go").exists())
        self.assertEqual((destination / "README.md").read_text(), "Handwritten\n")
        self.assertFalse((sdk / "request.go").exists())
        self.assertFalse(generate(self.project, [], check=True)[0]["changed"])
        # Stale manifests cannot delete a neighboring target's file.
        self.configure("[targets.go]\n")
        sync(self.project)
        (self.root / ".perseid/manifests").mkdir(parents=True)
        (self.root / ".perseid/manifests/go.json").write_text(
            '{"go": ["neighbor/file.go"]}'
        )
        with self.assertRaisesRegex(ValueError, "within"):
            generate(self.project, [])

    def test_version_policy_and_manifest_stamping(self):
        self.configure(
            "[targets.typescript]\n[targets.rust]",
            extra='[release]\npolicy = "lockstep"',
        )
        sync(self.project)
        with self.assertRaisesRegex(ValueError, "every target"):
            prepare_version(self.project, ["rust"], "minor", "Changes")
        state = prepare_version(self.project, [], "minor", "New API")
        self.assertEqual(state["versions"], {"rust": "0.2.0", "typescript": "0.2.0"})
        (self.root / "typescript").mkdir()
        (self.root / "typescript/package.json").write_text(
            '{"name":"example", "version":"0.1.0"}'
        )
        (self.root / "rust").mkdir()
        (self.root / "rust/Cargo.toml").write_text(
            '[package]\nname = "example"\nversion = "0.1.0" # keep this\n'
        )
        generate(self.project, [])
        self.assertEqual(
            json.loads((self.root / "typescript/package.json").read_text())["version"],
            "0.2.0",
        )
        self.assertIn(
            'version = "0.2.0" # keep this', (self.root / "rust/Cargo.toml").read_text()
        )
        self.assertFalse(generate(self.project, [], check=True)[0]["changed"])

    def test_independent_versions_and_toml_workspace_rejection(self):
        self.configure(
            "[targets.go]\n[targets.python]", extra='[release]\npolicy = "independent"'
        )
        sync(self.project)
        self.assertEqual(
            prepare_version(self.project, ["go"], "patch", "Fix")["versions"],
            {"go": "0.1.1", "python": "0.1.0"},
        )
        path = self.root / "Cargo.toml"
        path.write_text("[package]\nversion.workspace = true\n")
        with self.assertRaisesRegex(ValueError, "version_commands"):
            replace_toml_version(path, "package", "1.0.0")

    def fake_gh(self):
        prs, releases = [], []
        calls = []

        def execute(args, **kwargs):
            if args[0] != "gh":
                return run(args, **kwargs)
            calls.append(list(args))
            if args[1:3] == ["pr", "list"]:
                return json.dumps(prs)
            if args[1:3] == ["pr", "create"]:
                prs.append({"number": 1, "url": "https://github.com/test/sdk/pull/1"})
                return prs[0]["url"]
            if args[1:3] == ["pr", "edit"]:
                return ""
            if args[1:3] == ["pr", "close"]:
                prs.clear()
                return ""
            if args[1] == "api":
                return "\n".join(item["tag_name"] for item in releases)
            if args[1:3] == ["release", "create"]:
                releases.append({"tag_name": args[3]})
                return ""
            raise AssertionError(args)

        stack = ExitStack()
        stack.enter_context(
            patch("orchestration.github.github_repo", return_value="test/sdk")
        )
        stack.enter_context(patch("orchestration.github.run", side_effect=execute))
        stack.enter_context(patch("orchestration.releases.run", side_effect=execute))
        self.addCleanup(stack.close)
        return calls

    def test_propose_dry_run_and_repeat_uses_one_pr(self):
        sync(self.project)
        remote = self.setup_remote()
        calls = self.fake_gh()
        before = files(self.root)
        result = propose(self.project, [], dry_run=True)
        self.assertTrue(result[0]["changed"])
        self.assertFalse(calls)
        self.assertEqual(
            git(remote, "for-each-ref", "--format=%(refname)", "refs/heads"),
            "refs/heads/main",
        )
        first = propose(self.project, [])
        second = propose(self.project, [])
        self.assertEqual(first[0]["pr"], second[0]["pr"])
        self.assertEqual(sum(c[1:3] == ["pr", "create"] for c in calls), 1)
        self.assertFalse(second[0]["changed"])
        self.assertEqual(before, files(self.root))

    def test_propose_target_filter_includes_siblings_and_preserves_manual_commits(self):
        self.configure("[targets.go]\n[targets.python]")
        sync(self.project)
        remote = self.setup_remote()
        self.fake_gh()
        result = propose(self.project, ["go"], dry_run=True)
        self.assertTrue(any(p.startswith("python/") for p in result[0]["changed"]))
        propose(self.project, ["go"])
        branch = "perseid/example/update"
        git(self.root, "fetch", "--quiet", "origin", branch)
        git(self.root, "checkout", "--quiet", "-b", branch, "FETCH_HEAD")
        (self.root / "manual.txt").write_text("human change")
        commit(self.root, "Manual update")
        git(self.root, "push", "--quiet", "origin", branch)
        expected = git(remote, "rev-parse", branch)
        with self.assertRaisesRegex(ValueError, "non-Perseid"):
            propose(self.project, [])
        self.assertEqual(expected, git(remote, "rev-parse", branch))

    def test_external_targets_also_propose_controller_metadata(self):
        sdk = self.home / "sdk"
        sdk.mkdir()
        init_git(sdk)
        (sdk / "README.md").write_text("SDK")
        commit(sdk)
        self.configure(f'[targets.go]\nrepository = "{sdk}"\ndirectory = "."')
        self.setup_remote()  # snapshot/lock are not yet on the controller base
        sync(self.project)
        self.fake_gh()
        result = propose(self.project, [], dry_run=True)
        self.assertEqual(len(result), 2)
        self.assertIn("request.go", result[0]["changed"])
        self.assertIn("perseid.lock.json", result[1]["changed"])
        self.assertIn("spec/openapi.json", result[1]["changed"])

    def test_obsolete_pr_is_closed_after_base_catches_up(self):
        sync(self.project)
        self.setup_remote()
        calls = self.fake_gh()
        propose(self.project, [])
        git(self.root, "fetch", "--quiet", "origin", "perseid/example/update")
        git(self.root, "merge", "--quiet", "--ff-only", "FETCH_HEAD")
        git(self.root, "push", "--quiet", "origin", "main")
        result = propose(self.project, [])
        self.assertIsNone(result[0]["pr"])
        self.assertTrue(any(c[1:3] == ["pr", "close"] for c in calls))

    def test_stale_generated_files_are_removed_but_handwritten_files_survive(self):
        sync(self.project)
        generate(self.project, [])
        manifest = json.loads((self.root / ".perseid/manifests/go.json").read_text())
        owned = self.root / "go/obsolete.go"
        owned.write_text("// @generated\n")
        handwritten = self.root / "go/extension.go"
        handwritten.write_text("// handwritten\n")
        manifest["go"] += ["go/obsolete.go", "go/extension.go"]
        (self.root / ".perseid/manifests/go.json").write_bytes(json_bytes(manifest))
        generate(self.project, [])
        self.assertFalse(owned.exists())
        self.assertTrue(handwritten.exists())

    def test_release_recovers_after_publication_and_base_branch_advances(self):
        registry = self.home / "registry"
        count = self.home / "publish-count"
        self.configure(f"""[targets.go.release]
publish = "touch '{registry}'; echo published >> '{count}'"
is_published = "test -f '{registry}'"
""")
        sync(self.project)
        prepare_version(self.project, [], "patch", "Fix widget handling")
        generate(self.project, [])
        remote = self.setup_remote()
        calls = self.fake_gh()
        dry = release(self.project, [], dry_run=True)
        self.assertEqual(dry[0]["version"], "0.1.1")
        self.assertFalse(registry.exists())
        self.assertFalse(git(remote, "tag"))
        # Fail after publishing, before the GitHub release. Retry must use the
        # immutable tag and registry probe, even when main has advanced.
        original_run = __import__("orchestration.releases", fromlist=["run"]).run

        def fail_release(args, **kwargs):
            if args[:3] == ["gh", "release", "create"]:
                raise subprocess.CalledProcessError(1, args)
            return original_run(args, **kwargs)

        with patch("orchestration.releases.run", side_effect=fail_release):
            with self.assertRaises(subprocess.CalledProcessError):
                release(self.project, [])
        self.assertTrue(registry.exists())
        (self.root / "README.md").write_text("Later commit")
        commit(self.root, "advance main")
        git(self.root, "push", "--quiet", "origin", "main")
        release(self.project, [])
        release(self.project, [])
        self.assertEqual(count.read_text(), "published\n")
        self.assertEqual(sum(c[:3] == ["gh", "release", "create"] for c in calls), 1)
        self.assertNotEqual(
            git(remote, "rev-parse", "main"), git(remote, "rev-parse", "go/v0.1.1")
        )

    def test_failed_publication_probe_cannot_publish(self):
        self.configure(
            '[targets.go.release]\npublish = "exit 99"\nis_published = "exit 2"'
        )
        sync(self.project)
        prepare_version(self.project, [], "patch", "Fix")
        generate(self.project, [])
        self.setup_remote()
        self.fake_gh()
        with self.assertRaisesRegex(ValueError, "probe failed"):
            release(self.project, [])


if __name__ == "__main__":
    unittest.main()
