from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys

from .config import Project
from .sources import sync
from .workspace import generate, plan


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Sync, generate, review, and release SDK projects"
    )
    sub = parser.add_subparsers(dest="command", required=True)
    for name, description in {
        "sync": "Resolve the spec source and record an immutable snapshot",
        "plan": "Show the locked inputs, versions, and destinations (no network)",
        "generate": "Generate SDKs from the locked snapshot",
        "check": "Regenerate in isolation, run checks, and fail on drift",
        "propose": "Generate, check, and create/update one GitHub PR per repository",
        "version": "Prepare reviewed SDK version and release-note metadata",
        "release": "Publish merged SDK versions and create tagged GitHub releases",
    }.items():
        command = sub.add_parser(name, help=description, description=description)
        command.add_argument("--config", type=Path, default=Path("perseid.toml"))
        if name != "sync":
            command.add_argument(
                "--target",
                action="append",
                default=[],
                help="Target name; repeat to select several (default: all)",
            )
        if name == "generate":
            command.add_argument("--no-format", action="store_true")
            command.add_argument(
                "--check",
                action="store_true",
                help="Run configured SDK checks before applying files",
            )
        if name in {"propose", "release"}:
            command.add_argument(
                "--dry-run",
                action="store_true",
                help="Validate and show changes without pushing, publishing, or creating PRs/releases",
            )
        if name == "version":
            command.add_argument(
                "bump", help="major, minor, patch, or an explicit stable version"
            )
            command.add_argument(
                "--notes", type=Path, required=True, help="Release notes Markdown file"
            )
    args = parser.parse_args(argv)
    try:
        project = Project.load(args.config)
        if args.command == "sync":
            result = sync(project)
        elif args.command == "plan":
            result = plan(project, args.target)
        elif args.command in {"generate", "check"}:
            result = generate(
                project,
                args.target,
                check=args.command == "check",
                no_format=getattr(args, "no_format", False),
                checks=getattr(args, "check", False),
            )
            if args.command == "check" and any(item["changed"] for item in result):
                print(json.dumps(result, indent=2))
                print("Generated SDKs are out of date", file=sys.stderr)
                return 1
        elif args.command == "propose":
            from .github import propose

            result = propose(project, args.target, dry_run=args.dry_run)
        elif args.command == "version":
            from .releases import prepare_version

            result = prepare_version(
                project, args.target, args.bump, args.notes.read_text()
            )
        else:
            from .releases import release

            result = release(project, args.target, dry_run=args.dry_run)
        print(json.dumps(result, indent=2))
        return 0
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"perseid: {error}", file=sys.stderr)
        return 1
