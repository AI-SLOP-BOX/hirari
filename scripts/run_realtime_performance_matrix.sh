#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ARTIFACT_DIR=${AURA_EVIDENCE_DIR:-${TMPDIR:-/tmp}/aura-realtime-evidence}
mkdir -p "$ARTIFACT_DIR"
REPORT="$ARTIFACT_DIR/realtime-$(date +%Y%m%d-%H%M%S).log"

cd "$ROOT_DIR"
if ! AURA_NATIVE_TEST_ISOLATION=1 RUST_TEST_THREADS=1 \
    cargo test --release -p aura-core-bridge --test realtime_performance_workflow \
    hundred_track_callback_matrix_meets_device_deadlines -- \
    --ignored --exact --test-threads=1 --nocapture >"$REPORT" 2>&1; then
    cat "$REPORT"
    exit 1
fi
cat "$REPORT"

grep -q 'AURA_REALTIME_EVIDENCE=' "$REPORT" || {
    echo "realtime performance evidence was not emitted" >&2
    exit 1
}
echo "Realtime performance matrix passed: $REPORT"
