#!/usr/bin/env python3
"""Compatibility wrapper for native `perseid project` commands."""

import sys
from scripts._compat import run

if __name__ == "__main__":
    raise SystemExit(run(["project", *sys.argv[1:]]))
