#!/bin/sh
set -u

# Real-device evidence matrix.
# This runner deliberately distinguishes PASS, FAIL, and SKIPPED.  A missing
# device, SDK, or third-party fixture must never become a release pass by
# accident.  Set HIRARI_REAL_DEVICE_STRICT=1 to require every capability.

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${HIRARI_EVIDENCE_DIR:-${TMPDIR:-/tmp}/hirari-real-device-evidence}
mkdir -p "$OUT_DIR"
REPORT="$OUT_DIR/matrix-$(date +%Y%m%d-%H%M%S).tsv"
STRICT=${HIRARI_REAL_DEVICE_STRICT:-0}
FAILURES=0
SKIPS=0

printf 'capability\tstatus\tdetail\n' > "$REPORT"

record() {
    capability=$1
    status=$2
    detail=$3
    printf '%s\t%s\t%s\n' "$capability" "$status" "$detail" | tee -a "$REPORT"
    case "$status" in
        FAIL) FAILURES=$((FAILURES + 1)) ;;
        SKIPPED) SKIPS=$((SKIPS + 1)) ;;
    esac
}

run_required() {
    capability=$1
    shift
    log="$OUT_DIR/$capability.log"
    if "$@" >"$log" 2>&1; then
        record "$capability" PASS "log=$log"
    else
        record "$capability" FAIL "log=$log"
    fi
}

run_optional() {
    capability=$1
    shift
    log="$OUT_DIR/$capability.log"
    if "$@" >"$log" 2>&1; then
        record "$capability" PASS "log=$log"
    else
        record "$capability" FAIL "log=$log"
    fi
}

run_required software_device_transition "$ROOT_DIR/scripts/run_device_transition_contract.sh"

if [ "$(uname -s)" = "Darwin" ]; then
    run_required coreaudio_device "$ROOT_DIR/scripts/run_macos_audio_device_contract.sh"
else
    record coreaudio_device SKIPPED "requires macOS CoreAudio"
fi

if [ "$(uname -s)" = "Darwin" ]; then
    if command -v auval >/dev/null 2>&1; then
        # auval can hang while probing a misbehaving third-party component;
        # bound the probe so the matrix always emits a deterministic result.
        # AU validation may include four isolated-worker recovery passes when
        # a real component is installed. Bound it, but leave enough room for
        # cold-start and quarantine/restart cycles on a developer machine.
        run_optional au perl -e 'alarm 240; exec @ARGV' \
            "$ROOT_DIR/scripts/run_au_e2e_smoke.sh"
    else
        record au SKIPPED "auval is unavailable"
    fi
else
    record au SKIPPED "requires macOS AudioUnit"
fi

run_required clap perl -e 'alarm 240; exec @ARGV' \
    "$ROOT_DIR/scripts/run_clap_fixture_smoke.sh"

if [ "$FAILURES" -ne 0 ]; then
    printf 'real-device matrix failed: failures=%s skips=%s report=%s\n' \
        "$FAILURES" "$SKIPS" "$REPORT" >&2
    exit 1
fi
if [ "$STRICT" = "1" ] && [ "$SKIPS" -ne 0 ]; then
    printf 'real-device matrix incomplete in strict mode: skips=%s report=%s\n' \
        "$SKIPS" "$REPORT" >&2
    exit 2
fi
printf 'real-device matrix passed with skips=%s report=%s\n' "$SKIPS" "$REPORT"
