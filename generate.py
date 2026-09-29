#!/usr/bin/env python3
"""Compatibility wrapper for `perseid sdk`; Python is not needed by the CLI."""

import sys
from scripts._compat import run

if __name__ == "__main__":
    arguments = [arg for arg in sys.argv[1:] if arg != "--local"]
    raise SystemExit(
        run(arguments if arguments == ["--version"] else ["sdk", *arguments])
    )
