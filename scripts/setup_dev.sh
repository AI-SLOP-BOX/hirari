#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if ! command -v xcrun >/dev/null 2>&1; then
    echo "error: Xcode Command Line Tools are required (run: xcode-select --install)" >&2
    exit 1
fi

if ! xcrun --sdk macosx --find clang++ >/dev/null 2>&1; then
    echo "error: the macOS C++ toolchain is unavailable" >&2
    exit 1
fi

if ! command -v cmake >/dev/null 2>&1; then
    echo "error: CMake 3.20 or newer is required" >&2
    exit 1
fi
cmake_version=$(cmake --version | awk 'NR == 1 {print $3}')
cmake_major=$(printf '%s' "$cmake_version" | cut -d. -f1)
cmake_minor=$(printf '%s' "$cmake_version" | cut -d. -f2)
if [ "${cmake_major:-0}" -lt 3 ] || { [ "${cmake_major:-0}" -eq 3 ] && [ "${cmake_minor:-0}" -lt 20 ]; }; then
    echo "error: CMake 3.20 or newer is required (found $cmake_version)" >&2
    exit 1
fi

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "warning: FFmpeg not found; optional MP3/FLAC checks will be unavailable" >&2
fi

if ! command -v rustup >/dev/null 2>&1; then
    if [ "${AURA_AUTO_INSTALL_RUST:-0}" = "1" ]; then
        if ! command -v curl >/dev/null 2>&1; then
            echo "error: curl is required for automatic Rust installation" >&2
            exit 1
        fi
        echo "Rust toolchain not found; installing rustup from https://rustup.rs" >&2
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal
        # rustup's installer updates the user's shell profile, but this
        # process must also see the toolchain immediately.
        export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    else
        echo "error: rustup is required; install it from https://rustup.rs" >&2
        echo "      or rerun with AURA_AUTO_INSTALL_RUST=1" >&2
        exit 1
    fi
fi

# Non-interactive shells (CI, IDE tasks, and a broken .zshenv) may have
# rustup installed while omitting Cargo's bin directory from PATH. Resolve it
# explicitly so setup remains a genuine one-command bootstrap.
if ! command -v cargo >/dev/null 2>&1; then
    rustup_cargo_home=$(rustup show home 2>/dev/null || true)
    if [ -n "$rustup_cargo_home" ] && [ -x "$rustup_cargo_home/bin/cargo" ]; then
        export PATH="$rustup_cargo_home/bin:$PATH"
    fi
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "error: Cargo is not available on PATH; run 'rustup toolchain install 1.98.0'" >&2
    exit 1
fi

cd "$repository_root"
active_toolchain=$(rustup show active-toolchain)
case "$active_toolchain" in
    1.98.0*) : ;;
    *)
        echo "error: repository requires Rust 1.98.0 (active: $active_toolchain)" >&2
        exit 1
        ;;
esac
cargo fetch --locked
cargo fmt --all -- --check
cargo check --workspace --locked

if ! command -v cargo-audit >/dev/null 2>&1 || ! command -v cargo-deny >/dev/null 2>&1; then
    if [ "${AURA_AUTO_INSTALL_SECURITY:-0}" = "1" ]; then
        cargo install --locked cargo-audit cargo-deny
    else
        echo "warning: security tools are not installed; run 'AURA_AUTO_INSTALL_SECURITY=1 scripts/setup_dev.sh' or install 'cargo-audit cargo-deny' for local dependency gates" >&2
    fi
fi

echo "Aura development environment is ready."
