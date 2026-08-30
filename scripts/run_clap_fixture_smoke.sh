#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
"$ROOT_DIR/scripts/build_plugin_worker.sh"
"$ROOT_DIR/scripts/build_clap_fixture.sh"

cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
    -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
test -x "$TEST_BIN"

for TEST_NAME in \
    minimal_clap_fixture_instantiates_in_the_isolated_worker \
    minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output \
    crashing_clap_worker_is_restarted_then_quarantined \
    repeated_mailbox_overruns_quarantine_then_recover_to_audio
do
    if ! "$TEST_BIN" --list 2>/dev/null | grep -Eq "^${TEST_NAME}: test$"; then
        echo "CLAP smoke test is missing from selected binary: $TEST_NAME" >&2
        exit 2
    fi
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    AURA_CLAP_FIXTURE="$ROOT_DIR/build-tools/minimal-gain.clap" \
    AURA_PLUGIN_PATHS="$ROOT_DIR/build-tools" \
    "$TEST_BIN" "$TEST_NAME" --exact --ignored --test-threads=1
done
