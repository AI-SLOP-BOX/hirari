#!/bin/sh
# Creates a placeholder sibling for the private, unpublished `ame-ui` path
# dependency so `cargo` can load and resolve the workspace (Cargo.lock is
# committed; the stub declares the exact same dependency closure so
# `--locked` keeps working).
#
# The stub is only a manifest: it is never built. Building Hirari against
# the placeholder is unsupported; without the real AmeUI tree there is no
# way to build the desktop UI. Hirari's engine-only checks and the
# internal Linux CI run against the real AmeUI tree and are not available
# to forks.
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
AMEUI_DIR="${AMEUI_DIR:-$ROOT_DIR/../AmeUI}"

if [ -f "$AMEUI_DIR/Cargo.toml" ]; then
    echo "AmeUI sibling already present at $AMEUI_DIR - leaving it alone"
    exit 0
fi

mkdir -p "$AMEUI_DIR/src"
AMEUI_DIR="$AMEUI_DIR" python3 - "$ROOT_DIR/Cargo.lock" <<'PY'
import re
import sys

import os

lock_path, out_dir = sys.argv[1], os.environ["AMEUI_DIR"]
lock = open(lock_path, encoding="utf-8").read()

manifest = re.search(
    r'\[\[package\]\]\nname = "ame-ui"\nversion = "([^"]+)"\ndependencies = \[\n(.*?)\]\n',
    lock,
    re.S,
)
if not manifest:
    sys.exit("ensure_ameui_stub: Cargo.lock has no ame-ui entry; refusing to guess")

uuidless = [
    (re.match(r"(\S+)(?: (\S+))?$", entry).group(1),
     re.match(r"(\S+)(?: (\S+))?$", entry).group(2))
    for entry in re.findall(r'"([^"]+)"', manifest.group(2))
]

pkg_versions = {}
for name, version in re.findall(
    r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', lock
):
    pkg_versions.setdefault(name, set()).add(version)

deps = []
for name, pinned in uuidless:
    if pinned is None:
        versions = pkg_versions.get(name, set())
        if len(versions) != 1:
            sys.exit(
                f"ensure_ameui_stub: ambiguous lock versions for {name}: {sorted(versions)}"
            )
        pinned = versions.pop()
    deps.append((name, pinned.split('+', 1)[0]))

print(f"ensure_ameui_stub: generating stub at {out_dir} with {len(deps)} pinned deps")
lines = [
    "[package]",
    'name = "ame-ui"',
    'version = "0.1.0"',
    'edition = "2021"',
    "",
    "# Placeholder for the private AmeUI crate. Manifest only; never built.",
    '[lib]\npath = "src/lib.rs"',
    "",
    "[dependencies]",
]
for name, pinned in deps:
    lines.append(f'{name} = "={pinned}"')
with open(out_dir + "/Cargo.toml", "w", encoding="utf-8") as fh:
    fh.write("\n".join(lines) + "\n")
PY
printf '//! Placeholder for the private ame-ui crate. Manifest only; never built.\n' > "$AMEUI_DIR/src/lib.rs"
echo "Created a placeholder AmeUI sibling at $AMEUI_DIR (manifest only; not buildable)"
