#!/usr/bin/env python3
"""Translate a ruxenc fixture (.rx) into equivalent REPL input.

The AOT harness runs whole programs that each define a `def main
... end` entry point. We feed the SAME program to the REPL verbatim
and append a single `main()` call so the body executes as one
compilation unit — exactly how a user pastes a program into the REPL.

Why not hoist the body to bare top-level statements (the old
approach): the REPL keeps cross-input variable state by *replaying*
every prior side-effecting statement on each new input. Hoisting a
multi-statement `def main` body to N separate top-level inputs then
makes statement K re-run statements 1..K-1 — earlier `puts` reprint,
and one-shot side effects (creating a symlink, opening a file) re-run
and fail the second time (`symlink_fail`). Running the body inside one
`main()` call sidesteps replay entirely and matches the AOT semantics
the `.out` fixtures encode.

Fixtures with no top-level `def main` (bare top-level code) are passed
through unchanged.

Reads from stdin, writes to stdout.
"""
from __future__ import annotations

import re
import sys


def has_top_level_main(src: str) -> bool:
    """True if the source declares a column-0 `def main` / `def main()`."""
    for line in src.split("\n"):
        if line.startswith(" ") or line.startswith("\t"):
            continue
        if re.match(r"^def\s+main\s*(\(\s*\))?\s*(->|$)", line.strip()):
            return True
    return False


def translate(src: str) -> str:
    # Pass the program through verbatim; if it defines `main`, append a
    # call so the REPL actually runs it (defining a fn doesn't execute
    # it). Otherwise the fixture is bare top-level code that runs as-is.
    out = src.rstrip("\n")
    if has_top_level_main(src):
        out += "\nmain()"
    return out + "\n"


if __name__ == "__main__":
    sys.stdout.write(translate(sys.stdin.read()))
