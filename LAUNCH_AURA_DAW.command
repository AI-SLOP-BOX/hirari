#!/bin/zsh
set -euo pipefail

# Launch the actual desktop DAW application. Do not open target/doc: that is
# Rust API documentation, not the Aura user interface.
APP_PATH="${AURA_APP_PATH:-${HOME}/Applications/Aura DAW.app}"
if [[ ! -x "$APP_PATH/Contents/MacOS/Aura DAW" ]]; then
  echo "Aura DAW app is not built: $APP_PATH" >&2
  exit 1
fi

open "$APP_PATH"
