#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

echo "[mvp] build"
cmake --build build --parallel 4

echo "[mvp] workspace tests"
AURA_NATIVE_TEST_ISOLATION=1 RUST_TEST_THREADS=1 cargo test --workspace --quiet

echo "[mvp] cli capabilities"
capabilities="$(cargo run -q -p aura-core-bridge --bin aura -- capabilities)"
printf '%s\n' "$capabilities" | rg -q '"audio_|"midi_|"project\.'

echo "[mvp] source hygiene"
git diff --check

echo "MVP_GATE_PASS"
