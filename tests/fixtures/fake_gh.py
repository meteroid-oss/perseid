#!/usr/bin/env python3
"""Stateful GitHub API double; Git operations in lifecycle tests remain real."""

import json
import os
from pathlib import Path
import sys

path = Path(os.environ["FAKE_GH_STATE"])
state = (
    json.loads(path.read_text())
    if path.exists()
    else {"prs": {}, "releases": {}, "calls": []}
)
a = sys.argv[1:]
state["calls"].append(a)
repo = a[a.index("--repo") + 1] if "--repo" in a else ""
result = ""
if a[:2] == ["pr", "list"]:
    result = json.dumps(state["prs"].get(repo, []))
elif a[:2] == ["pr", "create"]:
    result = f"https://github.com/{repo}/pull/1"
    state["prs"][repo] = [{"number": 1, "url": result}]
elif a[:2] == ["pr", "close"]:
    state["prs"][repo] = []
elif a[:2] == ["pr", "edit"]:
    pass
elif a[:2] == ["release", "create"]:
    if os.environ.get("FAKE_GH_FAIL_RELEASE"):
        path.write_text(json.dumps(state))
        raise SystemExit(2)
    state["releases"].setdefault(repo, []).append(a[2])
elif a[0] == "api":
    repo = (
        next(p for p in a if p.startswith("repos/"))
        .removeprefix("repos/")
        .removesuffix("/releases")
    )
    result = "\n".join(state["releases"].get(repo, []))
else:
    raise AssertionError(a)
path.write_text(json.dumps(state))
print(result)
