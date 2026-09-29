#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository_root"

./scripts/build_app.sh
./scripts/verify_release_bundle.sh "packaging/Hirari DAW.app"
HIRARI_HEADLESS=1 "packaging/Hirari DAW.app/Contents/MacOS/Hirari DAW"
HIRARI_APP_PATH="packaging/Hirari DAW.app" ./scripts/run_app_launch_smoke.sh
codesign --verify --deep --strict "packaging/Hirari DAW.app"
./scripts/generate_release_metadata.sh release-metadata "packaging/Hirari DAW.app"
./scripts/verify_release_metadata.sh release-metadata

echo "Hirari release smoke passed"
