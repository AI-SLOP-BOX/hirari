#!/usr/bin/env python3
"""List C++ functions in src/core that nothing else references.

A name is reported when, across the whole repo (minus target/archive/.git),
every textual occurrence sits on a line that declares or defines it. Calls,
cxx-bridge declarations and Rust-side uses all count as references.
Usage: dead_cpp_functions.py
"""
import os, re, subprocess

ROOT = "src/core"
EXTS = (".inc", ".hpp", ".cpp", ".h")
SKIP_DIRS = {"target", "archive", ".git", "node_modules"}

# name( at a declaration position: type tokens, optional Class::, name, '('.
DECL = re.compile(
    r"^\s*(?:\[\[[^\]]*\]\]\s*)?(?:template\s*<[^>]*>\s*)?"
    r"(?:(?:static|virtual|inline|constexpr|explicit)\s+)*"
    r"(?:[\w:<>,\*&\[\]]+\s+)+[\*&]*(?:\w+::)?(\w+)\s*\("
)
KEYWORDS = {"return", "else", "co_return", "if", "for", "while", "switch", "case",
            "delete", "new", "throw", "sizeof", "static_assert", "using", "typedef"}


def all_files():
    for d, dirs, fs in os.walk("."):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        for f in fs:
            if f.endswith((".inc", ".hpp", ".cpp", ".h", ".mm", ".rs", ".slint", ".sh", ".toml")):
                yield os.path.join(d, f)


def main():
    defs = {}  # name -> set of (path, lineno)
    for d, _, fs in os.walk(ROOT):
        for f in fs:
            if not f.endswith(EXTS):
                continue
            p = os.path.join(d, f)
            for n, line in enumerate(open(p, errors="ignore"), 1):
                if line.rstrip().endswith(";") and "(" in line and not line.lstrip().startswith("//"):
                    pass
                first = line.strip().split(" ")[0]
                if first in KEYWORDS:
                    continue
                m = DECL.match(line)
                if m and m.group(1) not in KEYWORDS:
                    defs.setdefault(m.group(1), set()).add((p, n))
    texts = {p: open(p, errors="ignore").read().split("\n") for p in all_files()}
    # A cxx-bridge declaration only says "callable from Rust"; it is a site,
    # not a use. A Rust call such as `engine.name(` still counts.
    bridge = "./hirari-core-bridge/src/lib.rs"
    if bridge in texts:
        for n, line in enumerate(texts[bridge], 1):
            m = re.match(r"\s*fn (\w+)\(", line)
            if m:
                defs.setdefault(m.group(1), set()).add((bridge, n))
    word = {}
    dead = []
    for name, sites in sorted(defs.items()):
        if len(name) < 4 or name.startswith("~") or name.startswith("operator"):
            continue
        rx = re.compile(r"\b" + re.escape(name) + r"\b")
        refs = 0
        for p, lines in texts.items():
            for n, line in enumerate(lines, 1):
                if rx.search(line) and (p, n) not in sites:
                    # comment-only mentions do not count
                    code = line.split("//")[0]
                    if rx.search(code):
                        refs += 1
                        if refs:
                            break
            if refs:
                break
        if refs == 0:
            dead.append(name)
    print(" ".join(dead))


if __name__ == "__main__":
    main()
