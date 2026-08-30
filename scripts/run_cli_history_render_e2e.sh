#!/bin/sh
set -eu
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CLI_BIN=${AURA_CLI_BIN:-$ROOT_DIR/target/debug/aura}
cargo build -q -p aura-core-bridge --bin aura
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aura-cli-e2e.XXXXXX")
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM
PROJECT="$TMP_DIR/Song.aura"
"$CLI_BIN" project init "$PROJECT" "CLI E2E" 48000 >"$TMP_DIR/init.json"
"$CLI_BIN" track add "$PROJECT" "E2E Vocal" Audio >"$TMP_DIR/track.json"
TRACK_ID=$(python3 - "$TMP_DIR/track.json" <<'PY'
import json, sys
v=json.load(open(sys.argv[1])); assert v["ok"] is True; print(v["track_id"])
PY
)
"$CLI_BIN" plugin insert "$PROJECT" "$TRACK_ID" 1 >"$TMP_DIR/plugin.json"
python3 - "$TMP_DIR/plugin.json" <<'PY'
import json, sys
v=json.load(open(sys.argv[1])); assert v["ok"] is True
PY
# Read-only inspection and reproducibility manifest must work in a fresh
# process after all mutations have been persisted.
"$CLI_BIN" project inspect "$PROJECT" >"$TMP_DIR/inspect.json"
"$CLI_BIN" project manifest "$PROJECT" >"$TMP_DIR/manifest.json"
python3 - "$TMP_DIR/inspect.json" "$TMP_DIR/manifest.json" <<'PY'
import json, sys
inspect=json.load(open(sys.argv[1])); manifest=json.load(open(sys.argv[2]))
assert inspect["ok"] and inspect["track_count"] == 1
assert manifest["manifest_version"] == 1 and len(manifest["snapshot_sha256"]) == 64
PY
# Re-open through a second process and mutate the persisted document again.
"$CLI_BIN" track add "$PROJECT" "E2E Audio" Audio >"$TMP_DIR/track2.json"
python3 - "$TMP_DIR/track2.json" <<'PY'
import json, sys
v=json.load(open(sys.argv[1])); assert v["ok"] is True
PY
"$CLI_BIN" track rename "$PROJECT" "$TRACK_ID" "Renamed Vocal" >"$TMP_DIR/rename.json"
"$CLI_BIN" track delete "$PROJECT" "$TRACK_ID" >"$TMP_DIR/delete.json"
"$CLI_BIN" project inspect "$PROJECT" >"$TMP_DIR/inspect-after-delete.json"
python3 - "$TMP_DIR/rename.json" "$TMP_DIR/delete.json" "$TMP_DIR/inspect-after-delete.json" <<'PY'
import json, sys
rename, delete, inspect = (json.load(open(p)) for p in sys.argv[1:])
assert rename["ok"] and delete["ok"]
assert inspect["ok"] and inspect["track_count"] == 1
PY
echo "CLI project E2E passed: track_id=$TRACK_ID project=$PROJECT"
