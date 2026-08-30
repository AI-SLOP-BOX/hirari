#!/bin/sh
set -u

# Real-device evidence matrix.
# This runner deliberately distinguishes PASS, FAIL, and SKIPPED.  A missing
# device, SDK, or third-party fixture must never become a release pass by
# accident.  Set AURA_REAL_DEVICE_STRICT=1 to require every capability.

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT_DIR=${AURA_EVIDENCE_DIR:-${TMPDIR:-/tmp}/aura-real-device-evidence}
mkdir -p "$OUT_DIR"
REPORT="$OUT_DIR/matrix-$(date +%Y%m%d-%H%M%S).tsv"
STRICT=${AURA_REAL_DEVICE_STRICT:-0}
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

if [ -n "${AURA_VST3_FIXTURE:-}" ] || [ -e "$HOME/Library/Audio/Plug-Ins/VST3/Surge XT.vst3" ]; then
    VST3_SDK_READY=0
    for sdk in "${AURA_VST3_SDK:-}" /tmp/aura-vst3-sdk-new; do
        if [ -n "$sdk" ] && [ -f "$sdk/CMakeLists.txt" ] && [ -d "$sdk/public.sdk" ]; then
            VST3_SDK_READY=1
            break
        fi
    done
    if [ "$VST3_SDK_READY" -eq 1 ]; then
        run_optional vst3 "$ROOT_DIR/scripts/run_vst3_e2e_local.sh"
    else
        record vst3 SKIPPED "VST3 fixture found but AURA_VST3_SDK is not prepared"
    fi
else
    record vst3 SKIPPED "set AURA_VST3_FIXTURE to a real VST3 bundle"
fi

if [ "$(uname -s)" = "Darwin" ]; then
    if command -v auval >/dev/null 2>&1; then
        # auval can hang while probing a misbehaving third-party component;
        # bound the probe so the matrix always emits a deterministic result.
        run_optional au perl -e 'alarm 90; exec @ARGV' \
            "$ROOT_DIR/scripts/run_au_e2e_smoke.sh"
    else
        record au SKIPPED "auval is unavailable"
    fi
else
    record au SKIPPED "requires macOS AudioUnit"
fi

run_required clap perl -e 'alarm 90; exec @ARGV' \
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
