#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

failures=0
check_file() {
    path="$1"
    if [ ! -e "$path" ]; then
        printf 'MISSING: %s\n' "$path"
        failures=$((failures + 1))
    fi
}

check_source_directory_tracked() {
    path="$1"
    manifest="$path/Cargo.toml"
    mode=$(git ls-files -s -- "$path" | awk 'NR == 1 { print $1 }')
    if [ "$mode" = "160000" ]; then
        printf 'GITLINK_WITHOUT_SOURCE: %s (forks cannot build this checkout)\n' "$path" >&2
        failures=$((failures + 1))
        return
    fi
    if ! git ls-files --error-unmatch -- "$manifest" >/dev/null 2>&1; then
        printf 'UNTRACKED_SOURCE: %s\n' "$manifest" >&2
        failures=$((failures + 1))
    fi
}

printf '%s\n' '== Aura repository health =='
check_file Cargo.toml
check_file Cargo.lock
check_file rust-toolchain.toml
check_file README.md
check_file INSTALL.md
check_file CONTRIBUTING.md
check_file SECURITY.md
check_file CODE_OF_CONDUCT.md
check_file aura-core-bridge/Cargo.toml
check_file aura-core-bridge/src/lib.rs
check_file aura-core-bridge/src/bin/aura.rs
check_file aura-ui/Cargo.toml
check_file aura-ui/src/main.rs
check_file src/core/audio_engine.hpp
check_source_directory_tracked aura-core-bridge
check_source_directory_tracked aura-ui
check_file scripts/build_app.sh
check_file scripts/build_plugin_worker.sh
check_file scripts/verify_release_bundle.sh
check_file scripts/audit_async_generation_gate.sh

if [ -e CMakeLists.txt ]; then
    printf '%s\n' 'CMake entrypoint: present (legacy/secondary build path)'
else
    printf '%s\n' 'CMake entrypoint: absent (Cargo is the active build path)'
fi

if [ -e "Aura DAW.app" ]; then
    printf '%s\n' 'Root app bundle: present'
else
    printf '%s\n' 'Root app bundle: absent (packaging/Aura DAW.app is checked instead)'
fi

if [ -x packaging/"Aura DAW.app"/Contents/MacOS/Aura\ DAW ]; then
    printf '%s\n' 'Release executable: present'
else
    printf '%s\n' 'Release executable: absent (run scripts/build_app.sh to create it)'
    if [ "${AURA_REQUIRE_RELEASE_BUNDLE:-0}" = "1" ]; then
        failures=$((failures + 1))
    fi
fi

if [ "$failures" -ne 0 ]; then
    printf 'Repository health failed: %s required item(s) missing\n' "$failures" >&2
    exit 1
fi

printf '%s\n' 'Repository health: OK'
