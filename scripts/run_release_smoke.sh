#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository_root"

./scripts/build_app.sh
./scripts/verify_release_bundle.sh "packaging/Aura DAW.app"
AURA_HEADLESS=1 "packaging/Aura DAW.app/Contents/MacOS/Aura DAW"
AURA_APP_PATH="packaging/Aura DAW.app" ./scripts/run_app_launch_smoke.sh
codesign --verify --deep --strict "packaging/Aura DAW.app"
./scripts/generate_release_metadata.sh release-metadata "packaging/Aura DAW.app"
./scripts/verify_release_metadata.sh release-metadata

echo "Aura release smoke passed"
