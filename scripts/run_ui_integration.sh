#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

run_nonempty_test() {
    log_file=$(mktemp "${TMPDIR:-/tmp}/aura-ui-test.XXXXXX")
    if ! cargo test "$@" >"$log_file" 2>&1; then
        cat "$log_file"
        rm -f "$log_file"
        return 1
    fi
    cat "$log_file"
    # A successful Cargo invocation is not enough for an integration gate:
    # an empty/placeholder test target can otherwise report success with zero
    # tests.  Require at least one executed test in the captured harness.
    if ! grep -Eq 'running [1-9][0-9]* tests?' "$log_file"; then
        echo "[aura] verification failed: no tests executed for cargo test $*" >&2
        rm -f "$log_file"
        return 1
    fi
    rm -f "$log_file"
}

echo "[aura] core workflow integration tests"
# recording_render_workflow.rs is intentionally only a historical placeholder
# and contains no test functions.  Running it made Cargo print "0 tests" while
# the integration script still reported success.  Invoke the real targets so
# this gate exercises recording and rendering instead of a vacuous target.
run_nonempty_test -p aura-core-bridge --test recording_workflow -j 1
run_nonempty_test -p aura-core-bridge --test render_workflow -j 1
run_nonempty_test -p aura-core-bridge --test project_roundtrip -j 1

echo "[aura] isolated plugin workflow integration tests"
"$ROOT_DIR/scripts/build_plugin_worker.sh"
"$ROOT_DIR/scripts/build_clap_fixture.sh"
for sandbox_test in \
    minimal_clap_fixture_instantiates_in_the_isolated_worker \
    minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output \
    crashing_clap_worker_is_restarted_then_quarantined \
    repeated_mailbox_overruns_quarantine_then_recover_to_audio
do
    AURA_PLUGIN_HOST_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker" \
    AURA_CLAP_FIXTURE="$ROOT_DIR/build-tools/minimal-gain.clap" \
    AURA_PLUGIN_PATHS="$ROOT_DIR/build-tools" \
    run_nonempty_test -p aura-core-bridge --test plugin_sandbox_workflow -j 1 \
        "$sandbox_test" -- --ignored --exact --test-threads=1
done

echo "[aura] UI contract tests"
# Keep this gate scoped to the library contract.  `cargo test -p aura-ui`
# also builds every UI integration target and can consume several gigabytes
# during a clean checkout; the main-thread integration target is invoked
# explicitly below so the coverage remains visible and bounded.
run_nonempty_test -p aura-ui --bin aura-ui -j 1

echo "[aura] main-thread Slint integration test"
run_nonempty_test -p aura-ui --test ui_core_integration -j 1

echo "[aura] UI/Core integration smoke passed"
