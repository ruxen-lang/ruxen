#!/usr/bin/env python3
"""Check the living Markdown docs against the Ruby-only language spec.

Usage:
  python3 scripts/check_docs.py [paths...]   check the docs (paths narrow the report)
  python3 scripts/check_docs.py --self-test  run the embedded cases
  python3 scripts/check_docs.py --list-scope print the in-scope paths
  python3 scripts/check_docs.py --list-forms print the migration table patterns

Scope: every tracked `*.md` file except `docs/audits/` (a dated record),
files named CHANGELOG.md or LICENSE.md, and files whose line 1 is the
history banner. Out-of-scope files still serve as link targets.

Rules (output `path:line: R#: message`; exit 1 on a finding, exit 2 when
the rule source in the spec is broken):
  R0 a code fence is never closed.
  R1 a line of a Ruxen block (info `ruby`, `rx`, `ruxen` or empty) matches
     a `Detect` pattern of the spec's migration table, and the block's
     caption (nearest non-blank line before the fence) has no
     [deprecated] or [retired] tag.
  R2 a `rust` fence is not preceded by `<!-- compiler-rust -->`.
  R3 a [planned] tag is not a link to an open, uniquely anchored
     `#t-...` checkbox in TODO.md or docs/TASKS.md.
  R4 a Ruxen block's caption has no status tag; a body row of a table
     with a `Status...` header has no tag in that column (a `Detect` cell
     of `editorial` exempts the row); or a tag is not one of the four
     spellings.
  R5 a relative link does not resolve to a tracked file, a directory
     holding a tracked file, or an existing heading slug or `id=`.
  R6 (exit 2) the migration table, its `Detect` column, or a cell that is
     neither a backticked regex nor `editorial`.

In-place exemptions: `<!-- tag-vocabulary -->` ... `<!-- /tag-vocabulary -->`
(R3, R4 spelling), and a `<!-- historical -->` line before a heading, which
exempts that section from R1 and R4.
"""
import os
import re
import subprocess
import sys
import urllib.parse

SPEC = "docs/specs/syntax/ruby-naming.spec.md"
TRACKERS = ("TODO.md", "docs/TASKS.md")
BANNER = ("> Historical document. It predates the Ruby-only direction and may "
          "show Rust syntax that Ruxen no longer uses.")
SKIP_DIRS = ("docs/audits/",)
RECORD_NAMES = ("CHANGELOG.md", "LICENSE.md")
RUXEN_INFO = ("", "ruby", "rx", "ruxen")

FENCE_RE = re.compile(r"^(\s*)(`{3,}|~{3,})(.*)$")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
TAG_RE = re.compile(r"\[(implemented|planned|deprecated|retired)\]")
TAG_SPELLING_RE = re.compile(r"\[(implemented|planned|deprecated|retired)([^\]]+)\]")
EXEMPT_R1_RE = re.compile(r"\[(deprecated|retired)\]")
PLANNED_RE = re.compile(r"\[planned[^\]]*\](\(\s*(<[^>]*>|[^)\s]*)[^)]*\))?")
LINK_RE = re.compile(r"!?\[(?:[^\[\]]|\[[^\]]*\])*\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+\"[^\"]*\")?\s*\)")
REFDEF_RE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*(\S+)")
ID_RE = re.compile(r"""\b(?:id|name)\s*=\s*["']([^"']+)["']""")
CODE_SPAN_RE = re.compile(r"(`+)(.+?)\1")
OPEN_BOX_RE = re.compile(r"^\s*- \[ \] ")
SEPARATOR_RE = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


class RuleSourceError(Exception):
    """The spec's migration table cannot give a rule set (R6, exit 2)."""


class Repo:
    """Tracked paths plus a reader; the self-test passes an in-memory one."""

    def __init__(self, tracked, read):
        self.tracked = set(tracked)
        self._read = read
        self._text = {}
        self._anchors = {}

    def text(self, path):
        if path not in self._text:
            self._text[path] = self._read(path) if path in self.tracked else None
        return self._text[path]

    def is_dir(self, path):
        prefix = path.rstrip("/") + "/"
        return any(p.startswith(prefix) for p in self.tracked)

    def in_scope(self, path):
        if not path.endswith(".md") or path.startswith(SKIP_DIRS):
            return False
        if os.path.basename(path) in RECORD_NAMES:
            return False
        text = self.text(path) or ""
        return text.split("\n", 1)[0].rstrip() != BANNER

    def scope(self):
        return sorted(p for p in self.tracked if self.in_scope(p))

    def anchors(self, path):
        if path not in self._anchors:
            self._anchors[path] = anchors_of(Doc(self.text(path) or ""))
        return self._anchors[path]


class Block:
    def __init__(self, start, end, info, caption):
        self.start, self.end, self.info, self.caption = start, end, info, caption

    @property
    def word(self):
        return self.info.split()[0].lower() if self.info.split() else ""


class Doc:
    """One Markdown file split into fenced blocks and prose lines."""

    def __init__(self, text):
        self.lines = text.split("\n")
        n = len(self.lines)
        self.code = [False] * n
        self.blocks = []
        self.unclosed = None
        self._fences()
        self.vocab = self._region("<!-- tag-vocabulary -->", "<!-- /tag-vocabulary -->")
        self.historical = self._historical()

    def _caption(self, i):
        for j in range(i - 1, -1, -1):
            if self.lines[j].strip():
                return self.lines[j]
        return ""

    def _fences(self):
        open_ = None
        for i, line in enumerate(self.lines):
            m = FENCE_RE.match(line)
            if open_ is None:
                if m and not (m.group(2)[0] == "`" and "`" in m.group(3)):
                    open_ = (i, m.group(2), m.group(3).strip())
                    self.code[i] = True
                continue
            self.code[i] = True
            fence = m.group(2) if m else ""
            if (m and fence[0] == open_[1][0] and len(fence) >= len(open_[1])
                    and not m.group(3).strip()):
                self.blocks.append(Block(open_[0], i, open_[2], self._caption(open_[0])))
                open_ = None
        if open_ is not None:
            self.unclosed = open_[0]
            self.blocks.append(Block(open_[0], len(self.lines), open_[2],
                                     self._caption(open_[0])))

    def _region(self, start, end):
        flags, inside = [], False
        for line in self.lines:
            if line.strip() == start:
                inside = True
            flags.append(inside)
            if line.strip() == end:
                inside = False
        return flags

    def _historical(self):
        flags, level, pending = [], None, False
        for i, line in enumerate(self.lines):
            m = None if self.code[i] else HEADING_RE.match(line)
            if m:
                depth = len(m.group(1))
                if level is not None and depth <= level:
                    level = None
                if pending:
                    level = depth
            if line.strip():
                pending = line.strip() == "<!-- historical -->"
            flags.append(level is not None)
        return flags

    def prose(self):
        """(line index, text with code spans removed) for lines outside fences."""
        for i, line in enumerate(self.lines):
            if not self.code[i]:
                yield i, CODE_SPAN_RE.sub("", line)

    def tables(self):
        """(header cells, [(line index, cells)]) for each pipe table."""
        i, n = 0, len(self.lines)
        while i + 1 < n:
            if (not self.code[i] and self.lines[i].lstrip().startswith("|")
                    and SEPARATOR_RE.match(self.lines[i + 1])):
                header, rows, j = split_cells(self.lines[i]), [], i + 2
                while j < n and not self.code[j] and self.lines[j].lstrip().startswith("|"):
                    rows.append((j, split_cells(self.lines[j])))
                    j += 1
                yield header, rows
                i = j
            else:
                i += 1


def split_cells(line):
    s = line.strip()
    s = s[1:] if s.startswith("|") else s
    s = s[:-1] if s.endswith("|") and not s.endswith("\\|") else s
    return [c.strip().replace("\\|", "|") for c in re.split(r"(?<!\\)\|", s)]


def slug(text):
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text).replace("`", "")
    text = re.sub(r"<[^>]+>", "", text).strip().lower()
    return re.sub(r"[^\w\- ]", "", text).replace(" ", "-")


def anchors_of(doc):
    found, seen = set(), {}
    for i, line in enumerate(doc.lines):
        found.update(ID_RE.findall(line))
        m = None if doc.code[i] else HEADING_RE.match(line)
        if m:
            base = slug(m.group(2))
            count = seen.get(base, 0)
            seen[base] = count + 1
            found.add(base if count == 0 else "%s-%d" % (base, count))
    return found


def load_forms(repo):
    """[(row, compiled regex or None for editorial)] from the spec's table (R6)."""
    text = repo.text(SPEC)
    if text is None:
        raise RuleSourceError("%s: R6: spec not found" % SPEC)
    for header, rows in Doc(text).tables():
        if "Rust form" not in header:
            continue
        if "Detect" not in header:
            raise RuleSourceError("%s: R6: migration table has no Detect column" % SPEC)
        col, forms = header.index("Detect"), []
        for line, cells in rows:
            cell = cells[col] if col < len(cells) else ""
            where = "%s:%d: R6:" % (SPEC, line + 1)
            if cell == "editorial":
                forms.append((cells[0], None))
                continue
            m = re.fullmatch(r"`(.+)`", cell)
            if not m:
                raise RuleSourceError("%s row %s: Detect needs a `regex` or editorial" % (where, cells[0]))
            try:
                forms.append((cells[0], re.compile(m.group(1))))
            except re.error as e:
                raise RuleSourceError("%s row %s: invalid regex: %s" % (where, cells[0], e))
        if not forms:
            raise RuleSourceError("%s: R6: migration table has no rows" % SPEC)
        return forms
    raise RuleSourceError("%s: R6: migration table (a `Rust form` column) not found" % SPEC)


def resolve(base, target):
    """Repo-relative path of a link target written in file `base`."""
    target = urllib.parse.unquote(target)
    if target.startswith("/"):
        return os.path.normpath(target.lstrip("/"))
    return os.path.normpath(os.path.join(os.path.dirname(base), target))


def check_file(repo, path, forms):
    doc, out = Doc(repo.text(path) or ""), []

    def add(i, rule, msg):
        out.append((path, i + 1, rule, msg))

    if doc.unclosed is not None:
        add(doc.unclosed, "R0", "code fence is never closed")
    for b in doc.blocks:
        if b.word.startswith("rust") or b.word == "rs":
            if "<!-- compiler-rust -->" not in b.caption:
                add(b.start, "R2", "`%s` fence needs a `<!-- compiler-rust -->` line before it,"
                    " or a `ruby`/`text` label" % b.word)
            continue
        if b.word not in RUXEN_INFO or doc.historical[b.start]:
            continue
        if not TAG_RE.search(b.caption):
            add(b.start, "R4", "Ruxen example needs a status tag on its caption line")
        if EXEMPT_R1_RE.search(b.caption):
            continue
        for i in range(b.start + 1, min(b.end, len(doc.lines))):
            hit = next((row for row, rx in forms if rx and rx.search(doc.lines[i])), None)
            if hit:
                add(i, "R1", "Rust form (migration row %s): %s" % (hit, doc.lines[i].strip()))
    for header, rows in doc.tables():
        status = next((k for k, c in enumerate(header) if c.strip("* ").startswith("Status")), None)
        if status is None:
            continue
        detect = header.index("Detect") if "Detect" in header else None
        for i, cells in rows:
            if doc.historical[i] or (detect is not None and detect < len(cells)
                                     and cells[detect] == "editorial"):
                continue
            if not TAG_RE.search(cells[status] if status < len(cells) else ""):
                add(i, "R4", "status cell has no tag")
    tracker = path in TRACKERS
    for i, line in doc.prose():
        if not doc.vocab[i]:
            if not doc.historical[i]:
                for m in TAG_SPELLING_RE.finditer(line):
                    add(i, "R4", "tag `%s` is not one of [implemented], [planned],"
                        " [deprecated], [retired]" % m.group(0))
            if not tracker:
                for m in PLANNED_RE.finditer(line):
                    problem = planned_problem(repo, path, m.group(2))
                    if problem:
                        add(i, "R3", problem)
        targets = [m.group(1) for m in LINK_RE.finditer(line)]
        targets += [m.group(1) for m in [REFDEF_RE.match(line)] if m]
        for target in targets:
            problem = link_problem(repo, path, target.strip("<>"))
            if problem:
                add(i, "R5", problem)
    return out


def planned_problem(repo, path, target):
    if not target:
        return "[planned] must link an open checkbox in TODO.md or docs/TASKS.md"
    target = target.strip("<>")
    file_part, _, frag = target.partition("#")
    dest = resolve(path, file_part) if file_part else path
    if dest not in TRACKERS:
        return "[planned] links `%s`; it must link TODO.md or docs/TASKS.md" % target
    if not frag.startswith("t-"):
        return "[planned] link `%s` needs a `#t-...` anchor" % target
    text = repo.text(dest) or ""
    hits = [l for l in text.split("\n") if ('id="%s"' % frag) in l]
    if len(hits) != 1:
        return "anchor `%s` is in %s %d times; it must be there once" % (frag, dest, len(hits))
    if not OPEN_BOX_RE.match(hits[0]):
        return "anchor `%s` is not on an open `- [ ]` checkbox in %s" % (frag, dest)
    return None


def link_problem(repo, path, target):
    if not target or SCHEME_RE.match(target):
        return None
    file_part, _, frag = target.partition("#")
    dest = resolve(path, file_part) if file_part else path
    if dest not in repo.tracked:
        if file_part and repo.is_dir(dest):
            return None
        return "link `%s`: `%s` is not a tracked file or directory" % (target, dest)
    if frag and dest.endswith(".md"):
        frag = urllib.parse.unquote(frag)
        anchors = repo.anchors(dest)
        if frag not in anchors and frag.lower() not in anchors:
            return "link `%s`: no heading or id `%s` in %s" % (target, frag, dest)
    return None


def check(repo, select=None):
    """(findings, checked paths); raises RuleSourceError."""
    forms = load_forms(repo)
    paths = [p for p in repo.scope() if not select
             or any(p == s or p.startswith(s.rstrip("/") + "/") for s in select)]
    findings = []
    for p in paths:
        findings.extend(check_file(repo, p, forms))
    return findings, paths


def git_repo(root):
    out = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True,
                         stdout=subprocess.PIPE).stdout.decode()

    def read(path):
        with open(os.path.join(root, path), encoding="utf-8") as f:
            return f.read()
    return Repo([p for p in out.split("\0") if p], read)


# --- self-test ---------------------------------------------------------------
# Each case is a small in-memory repo: {path: text}, plus the expected
# findings as (path, line, rule), or "exit 2" for a broken rule source.

def L(*lines):
    return "\n".join(lines)


SAMPLE_SPEC = L(
    "# Spec",
    "<!-- tag-vocabulary -->",
    "[planned] means decided, not built.",
    "<!-- /tag-vocabulary -->",
    "",
    "| # | Rust form | Ruby form | Status of Ruby form | Detect |",
    "|---|---|---|---|---|",
    r"| 1 | `let x = 1` | `x = 1` | [implemented] | `^\s*(let\|var)\s+\w+\s*=` |",
    r"| 2 | `fn f()` | `def f` | [implemented] | `^\s*(pub\s+)?fn\s+\w+\(` |",
    r"| 3 | `Some(x)` | `x` | [implemented] | `\bSome\(` |",
    "| 4 | Rust prose | Ruby wording | docs rule | editorial |")
TODO_OK = "- [ ] <a id=\"t-ok\"></a> ok"
CASES = [
    ("fences: tilde-nested Markdown, list indent, unclosed, no fences", {
        "docs/a.md": L("# A", "Nested example:", "~~~~markdown", "```rust", "fn main() {}",
                       "```", "~~~~", "", "- item [implemented]:", "    ```ruby", "    x = 1",
                       "    ```"),
        "docs/b.md": L("# B", "```text", "never closed"),
        "docs/c.md": L("# C", "plain prose"),
    }, [("docs/b.md", 2, "R0")]),
    ("scope and links", {
        "docs/prompts/old.md": L(BANNER, "", "[dead](missing.md)", "```", "fn x()", "```"),
        "docs/audits/a.md": "[dead](nope.md)",
        "CHANGELOG.md": "[dead](nope.md)",
        "docs/live.md": L("# Live", "[old](prompts/old.md) and [gone](prompts/gone.md)",
                          "[dir](tutorial/) [nodir](missing/) [ignored](untracked/)",
                          "[ext](https://example.com/x) [mail](mailto:a@b.c) [ctx](CLAUDE.md)",
                          "[self](#live) [bad](#nope) [id](other.md#t-x) [slug](other.md#a-b-c)"),
        "docs/tutorial/01.md": "# One",
        "docs/other.md": L("# A `b` c!", "<a id=\"t-x\"></a>"),
    }, [("docs/live.md", 2, "R5"), ("docs/live.md", 3, "R5"), ("docs/live.md", 3, "R5"),
        ("docs/live.md", 4, "R5"), ("docs/live.md", 5, "R5")]),
    ("R1, R2, R4a: forms, captions, markers", {
        "TODO.md": TODO_OK,
        "docs/r1.md": L(
            "# R1", "Valid forms [implemented]:", "```ruby", "def var push(x: Int)", "f(&var x)",
            "xs.empty?", "def find(id: Int) -> User?", "lib \"c\"",
            "  def open as \"c_open\"(p: &String) -> Int", "end", "xs.each { |fn| puts fn }", "```",
            "", "Ruby spelling [planned](../TODO.md#t-ok):", "```ruby", "let x = 1", "```",
            "Works today [deprecated]:", "```ruby", "let x = 1", "```", "", "```rx", "x = 2", "```",
            "", "Untagged example:", "```", "puts 1", "```", "Compiler output:", "```text",
            "let x = 1", "```", "```rust", "fn a() {}", "```", "<!-- compiler-rust -->", "```rust",
            "fn b() {}", "```", "<!-- historical -->", "## Closed entry", "```", "let y = 2", "```",
            "## Open entry", "```ruby", "Some(3)", "```"),
    }, [("docs/r1.md", 16, "R1"), ("docs/r1.md", 23, "R4"), ("docs/r1.md", 28, "R4"),
        ("docs/r1.md", 35, "R2"), ("docs/r1.md", 48, "R4"), ("docs/r1.md", 49, "R1")]),
    ("R4b status tables and tag spelling", {
        "TODO.md": TODO_OK,
        "docs/t.md": L(
            "| Construct | Ruby form | Status |", "|---|---|---|", "| Loops | `while` | [implemented] |",
            "| Raise | `raise` | not yet |", "",
            "| # | Rust form | Ruby form | Status of Ruby form | Detect |", "|---|---|---|---|---|",
            r"| 9 | `x?.y` | `x&.y` |  | `\?\.` |", "| 10 | prose | wording | docs rule | editorial |",
            "", "Later [planned, later](../TODO.md#t-ok)."),
    }, [("docs/t.md", 4, "R4"), ("docs/t.md", 8, "R4"), ("docs/t.md", 11, "R4")]),
    ("R3 planned links", {
        "ROADMAP.md": TODO_OK,
        "TODO.md": L("# TODO", "- [x] <a id=\"t-done\"></a> done", "- [ ] parent",
                     "  - [ ] <a id=\"t-nested\"></a> nested", "- [ ] <a id=\"t-dup\"></a> one",
                     "- [ ] <a id=\"t-dup\"></a> two", TODO_OK),
        "docs/p.md": L("[planned](../ROADMAP.md#t-ok)", "[planned](../TODO.md#t-done)",
                       "[planned](../TODO.md#t-nested)", "[planned](../TODO.md#t-dup)",
                       "[planned] alone", "<!-- tag-vocabulary -->", "[planned] means decided",
                       "<!-- /tag-vocabulary -->", "`[planned]` in code"),
    }, [("docs/p.md", 1, "R3"), ("docs/p.md", 2, "R3"), ("docs/p.md", 4, "R3"),
        ("docs/p.md", 5, "R3")]),
    ("R6: no migration table", {SPEC: "# Spec"}, "exit 2"),
    ("R6: no Detect column", {SPEC: L("| # | Rust form | Ruby form | Status of Ruby form |",
                                      "|---|---|---|---|", "| 1 | a | b | [implemented] |")}, "exit 2"),
    ("R6: invalid regex", {SPEC: SAMPLE_SPEC.replace(r"`\bSome\(`", "`(`")}, "exit 2"),
    ("R6: cell without a pattern", {SPEC: SAMPLE_SPEC.replace("editorial |", "rule |")}, "exit 2"),
]


# Per migration-table row of the real spec: (a line the Detect pattern must
# flag, a line it must leave alone). Every pattern must also pass PASS_ALL.
ROW_SAMPLES = {
    "1": ("let x = 1", "x = 1"), "2": ("o = Some(3)", "o = 3"),
    "3": ("def f -> Result[Int, String]", "def f -> Int"),
    "4": ("data = read(path)?", "ok = (a > b) ? 1 : 2"),
    "5": ("n = find(1)?.name", "n = find(1)&.name"), "6": ("match v", "case v"),
    "7": ("if let x = opt", "if (x = opt)"), "8": ('panic!("boom")', 'abort "boom"'),
    "9": ("v = o.unwrap_or(0)", "v = o || 0"),
    "10": ("p = Point { x: 1, y: 2 }", "p = Point.new(x: 1, y: 2)"),
    "11": ("enum Color", "module Color"), "12": ("puts pair.0", "puts pair[0] + 3.0"),
    "13": ("def f -> Result[(), String]", "run()"), "14": ("const MAX = 3", "MAX = 3"),
    "15": ("  include PartialEq", "  include Comparable"),
    "16": ("@[repr(C)]", "# a comment [x]"), "17": ("xs.iter.map { |x| x }", "xs.map { |x| x }"),
    "18": ("puts s.len", "puts s.size"), "19": ("args = env.args", "args = ARGV"),
    "20": ("h = JoinHandle.new", "t = Thread.new { 1 }"), "21": ('println!("x")', 'puts "x"'),
    "22": ('puts "#{x:?}"', 'puts "#{x.inspect} #{a ? b : c}"'),
    "23": ("def f(s: &str)", "def f(s: &String)"),
    "24": ("def f(g: Fn(Int) -> Int)", "def f(g: Fn[(Int) -> Int])"),
    "25": ("def m(&self) -> Int", "register(self)"),
    "26": ("use std.io.Stdout", 'require "io"'),
    "27": ("b = n as UInt8", 'def open as "c_open"(p: &String) -> Int'),
    "29": ("def f[T: Comparable](x: T)", "def f[T < Comparable](x: T)"),
    "32": ("loop", "loop do"), "33": ("def f -> Int { 1 }", "def f = xs.each { |x| x }"),
    "34": ("mixin Greet", "module Greet"),
    "35": ("def init(@x: Int)", "def initialize(@x: Int)"),
    "36": ("def main", "def main_menu"), "37": ("m = s ~= /b/", "m = s =~ /b/"),
    "38": ("fn main() {", "xs.each { |fn| puts fn }"),
    "39": ("h: Map[String, Int]", "h: Hash[String, Int]"), "40": ("x = None", "x = nil"),
}
PASS_ALL = ["def var push(x: Int)", "f(&var x)", "xs.empty?", "def find(id: Int) -> User?",
            '  def open as "c_open"(p: &String) -> Int', "xs.each { |fn| puts fn }",
            "x = 1", 'puts "hi #{name}"', "double = ->(x: Int) { x * 2 }", "in nil then 0",
            "def each(&block: Fn[(T) -> nil]) -> nil", "a, b = b, a", "@name = name",
            "handlers: Array[Proc[() -> nil]]"]


def pattern_failures(repo):
    """Check every real Detect pattern against its samples and PASS_ALL."""
    out, forms = [], load_forms(repo)
    for row, rx in forms:
        if rx is None:
            continue
        if row not in ROW_SAMPLES:
            out.append("row %s has a pattern but no sample in ROW_SAMPLES" % row)
            continue
        bad, good = ROW_SAMPLES[row]
        if not rx.search(bad):
            out.append("row %s misses %r" % (row, bad))
        for line in [good] + PASS_ALL:
            if rx.search(line):
                out.append("row %s flags valid Ruby %r" % (row, line))
    stale = set(ROW_SAMPLES) - {row for row, rx in forms if rx}
    out += ["ROW_SAMPLES has row %s, which has no pattern" % row for row in sorted(stale)]
    return out


def mini_repo(files):
    files = dict(files)
    files.setdefault(SPEC, SAMPLE_SPEC)
    return Repo(files, files.get)


def self_test():
    failed = 0
    for name, files, expected in CASES:
        try:
            findings, _ = check(mini_repo(files))
            got = sorted((p, n, r) for p, n, r, _ in findings)
        except RuleSourceError:
            got = "exit 2"
        if got != (sorted(expected) if isinstance(expected, list) else expected):
            failed += 1
            print("FAIL %s\n  expected %s\n  got      %s" % (name, expected, got))
    scope = mini_repo(CASES[1][1]).scope()
    for path, want in (("docs/live.md", True), ("docs/prompts/old.md", False),
                       ("docs/audits/a.md", False), ("CHANGELOG.md", False)):
        if (path in scope) != want:
            failed += 1
            print("FAIL --list-scope: %s %s" % (path, "missing" if want else "listed"))
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    try:
        problems = pattern_failures(Repo([SPEC], lambda p: open(os.path.join(root, p)).read()))
    except RuleSourceError as e:
        problems = [str(e)]
    for problem in problems:
        failed += 1
        print("FAIL migration-table pattern: %s" % problem)
    print("self-test: %d failed" % failed)
    return 1 if failed else 0


def main(argv):
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if argv[:1] == ["--self-test"]:
        return self_test()
    repo = git_repo(root)
    if argv[:1] == ["--list-scope"]:
        print("\n".join(repo.scope()))
        return 0
    try:
        if argv[:1] == ["--list-forms"]:
            for row, rx in load_forms(repo):
                print("%s\t%s" % (row, rx.pattern if rx else "editorial"))
            return 0
        select = [os.path.relpath(os.path.abspath(a), root) for a in argv]
        findings, paths = check(repo, select)
    except RuleSourceError as e:
        print(e, file=sys.stderr)
        return 2
    for f in sorted(findings):
        print("%s:%d: %s: %s" % f)
    print("check_docs: %d files checked, %d findings" % (len(paths), len(findings)),
          file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
