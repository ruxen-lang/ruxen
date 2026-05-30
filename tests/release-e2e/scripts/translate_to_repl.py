#!/usr/bin/env python3
"""Translate a ruxenc fixture (.rx) into equivalent REPL input.

The ruxenc harness runs full programs that each define a `def main
... end` entry point. The REPL evaluates top-level statements one at
a time. To share the same `.out` expectations with the REPL harness,
we strip the `def main` wrapper and hoist its body to the top level,
leaving every other top-level item (class / struct / enum / trait /
impl / const / type / fn) verbatim.

Reads from stdin, writes to stdout.
"""
from __future__ import annotations

import re
import sys


# Tokens that open a new multi-line scope (closed by `end`).
OPENERS = {
    "def", "while", "if", "for", "loop", "match", "do",
    "unsafe", "class", "struct", "enum", "trait", "impl", "module",
}


def first_keyword(line: str) -> str | None:
    m = re.match(r"\s*([a-zA-Z_][a-zA-Z0-9_]*)\b", line)
    return m.group(1) if m else None


# Match keywords anywhere on a line, outside of strings and comments.
KEYWORD_SCAN = re.compile(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\b")


def count_openers(line: str) -> int:
    """Count opener keywords on a line that aren't cancelled out by `end`.

    Ignores content inside string literals and line comments, so `# def`
    and `"end"` don't disturb the balance.
    """
    stripped = line
    # Drop line comments.
    if "#" in stripped and not stripped.lstrip().startswith("##"):
        for m in re.finditer(r"#", stripped):
            i = m.start()
            # Not a block comment marker (#=) and not a doc comment (##)
            if (i + 1 < len(stripped) and stripped[i + 1] in ("=",)):
                continue
            # Strip from here to end of line.
            stripped = stripped[:i]
            break
    # Remove double-quoted string bodies so keywords inside strings don't count.
    stripped = re.sub(r'"(?:\\.|[^"\\])*"', '""', stripped)
    opens = 0
    for m in KEYWORD_SCAN.finditer(stripped):
        word = m.group(1)
        if word in OPENERS:
            opens += 1
        elif word == "end":
            opens -= 1
    return opens


def translate(src: str) -> str:
    lines = src.split("\n")
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        # Spot `def main` at column 0 with no args or with `def main()`.
        if re.match(r"^def\s+main\s*(\(\s*\))?\s*$", stripped) and not line.startswith(" "):
            # Consume the body, dedent by the first-line indent,
            # and skip the matching `end`.
            depth = 1
            body: list[str] = []
            i += 1
            while i < len(lines):
                b = lines[i]
                bs = b.strip()
                if bs == "end":
                    depth -= 1
                    if depth == 0:
                        i += 1
                        break
                    body.append(b)
                else:
                    # Count openers and end-closers on the line. A `let v = loop`
                    # line has `loop` as an opener (but first_keyword would
                    # return `let`). Scanning the whole line catches these.
                    delta = count_openers(b)
                    if delta != 0:
                        # Inline `{ ... }` one-liners suppress only when the
                        # line has no opener keyword. `match v.iter.find { |n|
                        # n > 3 }` has a real `match` opener and a closure on
                        # the same line — the closure shouldn't cancel the
                        # `match`'s +1.
                        line_no_strings = re.sub(r'"(?:\\.|[^"\\])*"', '""', bs)
                        has_opener_kw = any(
                            w in OPENERS for w in KEYWORD_SCAN.findall(line_no_strings)
                        )
                        if "{" in bs and bs.endswith("}") and not has_opener_kw:
                            pass
                        else:
                            depth += delta
                    body.append(b)
                i += 1
            # Dedent by the smallest nonzero leading-space count.
            nonblank = [ln for ln in body if ln.strip()]
            if nonblank:
                min_indent = min(len(ln) - len(ln.lstrip(" ")) for ln in nonblank)
                body = [ln[min_indent:] if len(ln) >= min_indent else ln for ln in body]
            out.extend(body)
        else:
            out.append(line)
            i += 1
    return "\n".join(out)


if __name__ == "__main__":
    sys.stdout.write(translate(sys.stdin.read()))
