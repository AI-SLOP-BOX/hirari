#!/bin/sh
# Build the Rust staticlib that every native C++ contract links against, and
# prove the resulting archive is not stale.
#
# The crate declares no crate-type, so `cargo build --lib` emits only an rlib.
# A `target/debug/libhirari_core_bridge.a` left over from an older
# configuration is therefore never refreshed by a normal build, and a native
# contract that links it can silently validate months-old Rust code. That is
# not hypothetical: it hid a missing FFI symbol once already.
#
# `cargo rustc --crate-type staticlib` is the only thing that regenerates the
# archive, so every native link must go through here. The freshness check below
# is a backstop for the case where a caller links the archive directly.
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

BRIDGE_LIB=target/debug/libhirari_core_bridge.a

# Rebuild unconditionally. Cargo decides whether recompilation is needed, and
# forcing the staticlib codegen is what actually refreshes the archive.
if ! cargo rustc -p hirari-core-bridge --lib --crate-type staticlib; then
    echo "failed to build the hirari-core-bridge staticlib" >&2
    exit 1
fi

if [ ! -f "$BRIDGE_LIB" ]; then
    echo "hirari-core-bridge staticlib not found at $BRIDGE_LIB" >&2
    exit 1
fi

# Backstop: refuse to hand back an archive older than any Rust source it was
# built from. Comparing against mtime is only a guard here, never the mechanism
# that refreshes the build; the rebuild above is what guarantees correctness.
newest_source=$(find hirari-core-bridge/src -name '*.rs' -newer "$BRIDGE_LIB" -print -quit 2>/dev/null || true)
if [ -n "$newest_source" ]; then
    echo "stale $BRIDGE_LIB: $newest_source is newer; refusing to hand back an" >&2
    echo "archive that cannot contain the current Rust sources" >&2
    exit 1
fi

echo "$ROOT_DIR/$BRIDGE_LIB"
