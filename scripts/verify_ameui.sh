#!/bin/sh
# Verifies that hirari-ui builds against the sibling AmeUI checkout.
# AmeUI's source lives on the Linux server at ~/work/ameui
# (remote: aislopbox@192.168.0.21:~/repos/ameui.git). On a developer Mac the
# sibling ../AmeUI is used instead when present.
set -eu
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
AMEUI_DIR="${AMEUI_DIR:-$ROOT_DIR/../AmeUI}"

if [ ! -d "$AMEUI_DIR/.git" ]; then
    echo "verify_ameui: no AmeUI checkout at $AMEUI_DIR - skipping"
    exit 0
fi

AMEUI_DIR_REAL=$(CDPATH= cd -- "$AMEUI_DIR" && pwd)
echo "verify_ameui: using $AMEUI_DIR_REAL ($(git -C "$AMEUI_DIR_REAL" log --oneline -1 2>&1 || echo '?'))"

cd "$ROOT_DIR"
# The ame-ui crate is a path dependency on $AMEUI_DIR; the checked-out sibling
# is the point of the test. This is just the default workspace build now.
cargo check -p hirari-ui --locked
