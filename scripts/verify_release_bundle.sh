#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
    : "${AURA_EXPECTED_ARCH:?AURA_EXPECTED_ARCH is required in release mode}"
    AURA_REQUIRE_ARCH=1
    AURA_REQUIRE_APP_SMOKE=1
    AURA_REQUIRE_PLUGIN_SMOKE=1
    AURA_REQUIRE_CODESIGN=1
    AURA_FAIL_ON_RETRY=1
    export AURA_REQUIRE_ARCH AURA_REQUIRE_APP_SMOKE AURA_REQUIRE_PLUGIN_SMOKE AURA_REQUIRE_CODESIGN
    export AURA_FAIL_ON_RETRY
fi
APP_DIR="${1:-$ROOT_DIR/packaging/Aura DAW.app}"
case "$APP_DIR" in
    /*) : ;;
    *) APP_DIR="$ROOT_DIR/$APP_DIR" ;;
esac
CONTENTS="$APP_DIR/Contents"
MAIN="$CONTENTS/MacOS/Aura DAW"
WORKER="$CONTENTS/MacOS/aura-plugin-host-worker"

test -d "$APP_DIR"
test -x "$MAIN"
test -x "$WORKER"
test -f "$CONTENTS/Info.plist"
test -d "$CONTENTS/Resources"
test -s "$CONTENTS/Resources/aura-resources.manifest" || {
    echo "release bundle resource manifest is missing or empty" >&2
    exit 1
}
BUILD_MANIFEST="$CONTENTS/Resources/aura-build.manifest"
test -s "$BUILD_MANIFEST" || {
    echo "release bundle build manifest is missing or empty" >&2
    exit 1
}

# Keep the package contract explicit: an app with a native main executable
# and worker but no resource directory is usually a stale or hand-assembled
# bundle.  The directory is allowed to be empty for the current Rust/Slint
# build, but it must exist so resource lookup has a stable root.
if [ ! -r "$CONTENTS/Info.plist" ]; then
    echo "release bundle Info.plist is not readable" >&2
    exit 1
fi

# The release bundle must contain the actual isolated native worker, not only
# a Rust wrapper.  Keep this check structural and portable across stripped
# binaries: executable size and Mach-O/ELF identification catch placeholder
# text files or an accidentally copied script.
worker_kind=$(file -b "$WORKER")
case "$worker_kind" in
    *Mach-O*|*ELF*) : ;;
    *) echo "release bundle worker is not a native executable: $worker_kind" >&2; exit 1 ;;
esac

main_kind=$(file -b "$MAIN")
case "$main_kind" in
    *Mach-O*|*ELF*) : ;;
    *) echo "release bundle main executable is not native: $main_kind" >&2; exit 1 ;;
esac

test "$MAIN" != "$WORKER"

hash_file() {
    file="$1"
    normalized="$file"
    cleanup_normalized=0
    # build_app.sh records the executable bytes before ad-hoc signing.  On
    # macOS, codesign embeds its signature in the Mach-O, so normalize only a
    # disposable copy before comparing the recorded build identity.
    if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
        normalized=$(mktemp "${TMPDIR:-/tmp}/aura-hash.XXXXXX")
        cp "$file" "$normalized"
        codesign --remove-signature "$normalized" >/dev/null 2>&1 || true
        cleanup_normalized=1
    fi
    if command -v shasum >/dev/null 2>&1; then
        hash=$(shasum -a 256 "$normalized" | awk '{print $1}')
    elif command -v sha256sum >/dev/null 2>&1; then
        hash=$(sha256sum "$normalized" | awk '{print $1}')
    else
        echo "release bundle hash verification requires shasum or sha256sum" >&2
        exit 1
    fi
    [ "$cleanup_normalized" -eq 0 ] || rm -f "$normalized"
    printf '%s\n' "$hash"
}
manifest_hash() {
    key="$1"
    sed -n "s/^${key}=//p" "$BUILD_MANIFEST" | head -n 1
}
manifest_main_hash=$(manifest_hash main_sha256)
manifest_worker_hash=$(manifest_hash worker_sha256)
case "$manifest_main_hash" in
    ''|*[!0-9a-fA-F]*) echo "invalid main_sha256 in release manifest" >&2; exit 1 ;;
esac
case "$manifest_worker_hash" in
    ''|*[!0-9a-fA-F]*) echo "invalid worker_sha256 in release manifest" >&2; exit 1 ;;
esac
[ "${#manifest_main_hash}" -eq 64 ] || { echo "invalid main_sha256 length in release manifest" >&2; exit 1; }
[ "${#manifest_worker_hash}" -eq 64 ] || { echo "invalid worker_sha256 length in release manifest" >&2; exit 1; }
[ "$(hash_file "$MAIN")" = "$manifest_main_hash" ] || {
    echo "release bundle main executable does not match its build manifest" >&2
    exit 1
}
[ "$(hash_file "$WORKER")" = "$manifest_worker_hash" ] || {
    echo "release bundle worker does not match its build manifest" >&2
    exit 1
}

if [ "${AURA_REQUIRE_ARCH:-0}" = "1" ] && [ -z "${AURA_EXPECTED_ARCH:-}" ]; then
    echo "AURA_EXPECTED_ARCH is required when AURA_REQUIRE_ARCH=1" >&2
    exit 1
fi
if [ -n "${AURA_EXPECTED_ARCH:-}" ]; then
    if ! command -v lipo >/dev/null 2>&1; then
        if [ "${AURA_REQUIRE_ARCH:-0}" = "1" ]; then
            echo "architecture verification requires lipo" >&2
            exit 1
        fi
        echo "warning: lipo unavailable; architecture verification skipped" >&2
    else
        verify_arches() {
            binary="$1"
            label="$2"
            actual=$(lipo -archs "$binary")
            if [ "${AURA_EXPECTED_ARCH}" = "universal2" ]; then
                case " $actual " in
                    *" arm64 "*) : ;;
                    *) echo "release bundle $label is not universal2: $actual" >&2; exit 1 ;;
                esac
                case " $actual " in
                    *" x86_64 "*) : ;;
                    *) echo "release bundle $label is not universal2: $actual" >&2; exit 1 ;;
                esac
            else
                case " $actual " in
                    *" ${AURA_EXPECTED_ARCH} "*) : ;;
                    *) echo "release bundle $label lacks ${AURA_EXPECTED_ARCH}: ${actual}" >&2; exit 1 ;;
                esac
            fi
        }
        verify_arches "$MAIN" "main executable"
        verify_arches "$WORKER" "worker"
    fi
fi

if command -v otool >/dev/null 2>&1; then
    otool -L "$MAIN" >/dev/null
    otool -L "$WORKER" >/dev/null
fi

plist_value() {
    key="$1"
    if [ -x /usr/libexec/PlistBuddy ]; then
        /usr/libexec/PlistBuddy -c "Print :$key" "$CONTENTS/Info.plist"
        return
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$CONTENTS/Info.plist" "$key" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as stream:
    value = plistlib.load(stream).get(sys.argv[2])
if value is None:
    raise SystemExit(1)
print(value)
PY
        return
    fi
    echo "cannot inspect Info.plist: PlistBuddy and python3 are unavailable" >&2
    return 1
}

executable=$(plist_value CFBundleExecutable)
test "$executable" = "Aura DAW"
test -x "$CONTENTS/MacOS/$executable"
bundle_identifier=$(plist_value CFBundleIdentifier)
test -n "$bundle_identifier"
bundle_version=$(plist_value CFBundleVersion)
test -n "$bundle_version"
minimum_system=$(plist_value LSMinimumSystemVersion)
test -n "$minimum_system"
bundle_type=$(plist_value CFBundlePackageType 2>/dev/null || true)
if [ -n "$bundle_type" ] && [ "$bundle_type" != "APPL" ]; then
    echo "release bundle has unexpected CFBundlePackageType: $bundle_type" >&2
    exit 1
fi

# Debug-only smoke code must not be present in the production executable.
if strings "$MAIN" | grep -q 'AURA_UI_SMOKE'; then
    echo "release bundle contains debug smoke entry point" >&2
    exit 1
fi
if strings "$WORKER" | grep -q 'AURA_UI_SMOKE'; then
    echo "release bundle worker contains debug smoke entry point" >&2
    exit 1
fi

# If the CLAP fixture is available, exercise the packaged worker itself. This
# catches a bundle with a valid-looking Mach-O that cannot complete the real
# sandbox handshake. Keep the fixture optional for release environments that
# intentionally omit test assets.
FIXTURE="$ROOT_DIR/build-tools/minimal-gain.clap"
if [ "${AURA_REQUIRE_PLUGIN_SMOKE:-0}" = "1" ] && [ ! -f "$FIXTURE" ]; then
    echo "required release plugin smoke fixture is missing: $FIXTURE" >&2
    exit 1
fi
if [ -f "$FIXTURE" ] && command -v cargo >/dev/null 2>&1; then
    cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
    PLUGIN_TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
        -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
    test -x "$PLUGIN_TEST_BIN"
    run_packaged_plugin_test() {
        smoke_test=$1
        shift
        if ! "$PLUGIN_TEST_BIN" --list 2>/dev/null | grep -Eq "^${smoke_test}: test$"; then
            echo "packaged plugin smoke test is missing from selected binary: $smoke_test" >&2
            exit 1
        fi
        env "$@" "$PLUGIN_TEST_BIN" "$smoke_test" --exact --ignored --test-threads=1
    }
    passed=0
    attempt=1
    while [ "$attempt" -le 3 ]; do
        if AURA_PLUGIN_HOST_BIN="$WORKER" \
            AURA_CLAP_FIXTURE="$FIXTURE" \
            AURA_PLUGIN_PATHS="$(dirname "$FIXTURE")" \
            "$PLUGIN_TEST_BIN" minimal_clap_fixture_instantiates_in_the_isolated_worker \
                --exact --ignored --test-threads=1 >/dev/null 2>&1; then
            passed=1
            if [ "$attempt" -gt 1 ]; then
                echo "warning: packaged worker smoke passed on retry $attempt" >&2
                if [ "${AURA_FAIL_ON_RETRY:-0}" = "1" ]; then
                    echo "release smoke is flaky: retry success is not accepted in strict mode" >&2
                    exit 1
                fi
            fi
            break
        fi
        attempt=$((attempt + 1))
    done
    test "$passed" -eq 1

    for smoke_test in \
        crashing_clap_worker_is_restarted_then_quarantined \
        repeated_mailbox_overruns_quarantine_then_recover_to_audio
    do
        run_packaged_plugin_test "$smoke_test" \
            AURA_PLUGIN_HOST_BIN="$WORKER" \
            AURA_CLAP_FIXTURE="$FIXTURE" \
            AURA_PLUGIN_PATHS="$(dirname "$FIXTURE")" >/dev/null 2>&1
    done
fi

if command -v codesign >/dev/null 2>&1; then
    codesign --verify --deep --strict "$APP_DIR"
elif [ "${AURA_REQUIRE_CODESIGN:-0}" = "1" ]; then
    echo "codesign is required but unavailable" >&2
    exit 1
else
    echo "warning: codesign unavailable; signature verification skipped" >&2
fi

if [ "${AURA_REQUIRE_APP_SMOKE:-0}" = "1" ]; then
    before_workers=$(pgrep -f '/aura-plugin-host-worker(-vst3)?([[:space:]]|$)' 2>/dev/null || true)
    if [ -d /dev/shm ]; then
        before_shared_memory=$(find /dev/shm -maxdepth 1 -type f -name 'aura_plugin_*' -print 2>/dev/null | wc -l | tr -d ' ')
    elif command -v lsof >/dev/null 2>&1; then
        before_shared_memory=$(lsof -n -c aura-plugin-host-worker 2>/dev/null | grep -c '/aura_plugin_' || true)
    else
        before_shared_memory=0
    fi
    perl -e '$ENV{AURA_HEADLESS}=1; alarm 30; my $program=shift @ARGV; exec {$program} $program, @ARGV' "$MAIN" >/tmp/aura-release-headless-smoke.$$.log 2>&1 || {
        cat /tmp/aura-release-headless-smoke.$$.log >&2
        rm -f /tmp/aura-release-headless-smoke.$$.log
        exit 1
    }
    grep -Eq '^AURA_HEADLESS_READY .*native_engine=ready .*bridge=ready .*project_layout=valid .*resources=ready .*audio_device_ready=(true|false) .*audio_driver=[^ ]+ ' /tmp/aura-release-headless-smoke.$$.log || {
        cat /tmp/aura-release-headless-smoke.$$.log >&2
        rm -f /tmp/aura-release-headless-smoke.$$.log
        echo "headless app did not report readiness" >&2
        exit 1
    }
    if [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
        grep -Eq '^AURA_HEADLESS_READY .*audio_device_ready=true .*audio_driver=(initialized|running) ' /tmp/aura-release-headless-smoke.$$.log || {
            cat /tmp/aura-release-headless-smoke.$$.log >&2
            rm -f /tmp/aura-release-headless-smoke.$$.log
            echo "release headless app did not prove an active audio device" >&2
            exit 1
        }
    fi
    cleanup_attempt=1
    while [ "$cleanup_attempt" -le 10 ]; do
        leaked_worker=0
        for pid in $(pgrep -f '/aura-plugin-host-worker(-vst3)?([[:space:]]|$)' 2>/dev/null || true); do
            case " $before_workers " in
                *" $pid "*) ;;
                *) leaked_worker=1 ;;
            esac
        done
        if [ -d /dev/shm ]; then
            after_shared_memory=$(find /dev/shm -maxdepth 1 -type f -name 'aura_plugin_*' -print 2>/dev/null | wc -l | tr -d ' ')
        elif command -v lsof >/dev/null 2>&1; then
            after_shared_memory=$(lsof -n -c aura-plugin-host-worker 2>/dev/null | grep -c '/aura_plugin_' || true)
        else
            after_shared_memory=0
        fi
        [ "$leaked_worker" -eq 0 ] && [ "$after_shared_memory" -le "$before_shared_memory" ] && break
        sleep 0.1
        cleanup_attempt=$((cleanup_attempt + 1))
    done
    if [ "$leaked_worker" -ne 0 ]; then
        echo "headless app left worker process" >&2
        exit 1
    fi
    if [ "$after_shared_memory" -gt "$before_shared_memory" ]; then
        echo "headless app left shared-memory resources: before=$before_shared_memory after=$after_shared_memory" >&2
        exit 1
    fi
    rm -f /tmp/aura-release-headless-smoke.$$.log
fi
echo "release bundle verified: $APP_DIR"
