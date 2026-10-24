#!/usr/bin/env python3
"""Delete C++ function declarations/definitions by name from src/core.

Usage: prune_cpp.py name [name...]
Removes declarations (ending in ';') and definitions (brace matched) whose
signature line looks like `<type tokens> [Class::]name(`; calls are left
alone. Comment lines directly above a removed item go with it.
"""
import os, re, sys

ROOT = "src/core"
EXTS = (".inc", ".hpp", ".cpp", ".h")


def files():
    for d, _, fs in os.walk(ROOT):
        for f in fs:
            if f.endswith(EXTS):
                yield os.path.join(d, f)


def sig_re(name):
    return re.compile(
        r"^\s*(?:\[\[[^\]]*\]\]\s*)?(?:template\s*<[^>]*>\s*)?"
        r"(?:[\w:<>,\*&\[\]]+\s+)+[\*&]*(?:\w+::)?" + re.escape(name) + r"\s*\("
    )


def find_end(lines, i):
    """Return (end_index, kind) for the item starting at line i."""
    depth_paren = 0
    j = i
    seen_open = False
    text = ""
    while j < len(lines):
        line = re.sub(r"//.*", "", lines[j])
        for ch in line:
            if ch == "(":
                depth_paren += 1
            elif ch == ")":
                depth_paren -= 1
            elif depth_paren == 0 and ch == ";":
                return j, "decl"
            elif depth_paren == 0 and ch == "{":
                seen_open = True
                break
        if seen_open:
            break
        j += 1
    if not seen_open:
        return None, None
    # brace match from line j
    depth = 0
    started = False
    k = j
    while k < len(lines):
        line = re.sub(r"//.*", "", lines[k])
        line = re.sub(r'"(\\.|[^"\\])*"', '""', line)
        line = re.sub(r"'(\\.|[^'\\])'", "''", line)
        for ch in line:
            if ch == "{":
                depth += 1
                started = True
            elif ch == "}":
                depth -= 1
        if started and depth == 0:
            return k, "def"
        k += 1
    return None, None


def prune(name):
    rx = sig_re(name)
    removed = 0
    for path in files():
        lines = open(path).read().split("\n")
        out = []
        i = 0
        changed = False
        while i < len(lines):
            line = lines[i]
            first = line.strip().split(" ")[0] if line.strip() else ""
            if first not in ("return", "else", "co_return") and rx.match(line):
                end, kind = find_end(lines, i)
                if end is not None:
                    # drop directly-attached comment lines already emitted
                    while out and out[-1].strip().startswith("//"):
                        out.pop()
                    removed += end - i + 1
                    changed = True
                    i = end + 1
                    # swallow one trailing blank line if previous is blank
                    if i < len(lines) and lines[i].strip() == "" and (not out or out[-1].strip() == ""):
                        i += 1
                    continue
            out.append(line)
            i += 1
        if changed:
            open(path, "w").write("\n".join(out))
    return removed


if __name__ == "__main__":
    total = 0
    for n in sys.argv[1:]:
        r = prune(n)
        print(f"{n}: removed {r} lines")
        total += r
    print("total", total)
