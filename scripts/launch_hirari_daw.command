#!/bin/zsh
set -euo pipefail

# Launch the actual desktop DAW application. Do not open target/doc: that is
# Rust API documentation, not the Hirari user interface.
APP_PATH="${HIRARI_APP_PATH:-${HOME}/Applications/Hirari DAW.app}"
if [[ ! -x "$APP_PATH/Contents/MacOS/Hirari DAW" ]]; then
  echo "Hirari DAW app is not built: $APP_PATH" >&2
  exit 1
fi

open "$APP_PATH"
