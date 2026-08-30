#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ROUNDS=${AURA_STRESS_ROUNDS:-100}
DURATION_SECONDS=${AURA_STRESS_DURATION_SECONDS:-0}
MAX_RSS_KB=${AURA_STRESS_MAX_RSS_KB:-65536}
MAX_FDS=${AURA_STRESS_MAX_FDS:-128}
MAX_RSS_GROWTH_KB=${AURA_STRESS_MAX_RSS_GROWTH_KB:-8192}
MAX_FD_GROWTH=${AURA_STRESS_MAX_FD_GROWTH:-16}
WARMUP_ROUNDS=${AURA_STRESS_WARMUP_ROUNDS:-2}
MAX_RETRY_SUCCESSES=${AURA_STRESS_MAX_RETRY_SUCCESSES:-0}
BASELINE_SAMPLES=${AURA_STRESS_BASELINE_SAMPLES:-3}
REQUIRE_RESOURCE_METRICS=${AURA_REQUIRE_RESOURCE_METRICS:-0}
retry_successes=0
peak_rss_seen=0
peak_fds_seen=0
ARTIFACT_DIR=${AURA_STRESS_ARTIFACT_DIR:-}
BASELINE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aura-stress-baseline.XXXXXX")
OBSERVED_DIR="$BASELINE_DIR/observed"
mkdir -p "$OBSERVED_DIR"
cleanup_baseline() {
    if [ -n "${CURRENT_TEST_PID:-}" ] && kill -0 "$CURRENT_TEST_PID" 2>/dev/null; then
        kill "$CURRENT_TEST_PID" 2>/dev/null || true
    fi
    for pid in ${CURRENT_TEST_WORKER_PIDS:-}; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    done
    # Give children a short grace period, then reap only the processes this
    # coordinator launched. Never use a name-based kill here: other Aura
    # sessions may be running on the same machine.
    for pid in ${CURRENT_TEST_PID:-} ${CURRENT_TEST_WORKER_PIDS:-}; do
        [ -n "$pid" ] || continue
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
        fi
    done
    rm -rf "$BASELINE_DIR"
}
trap cleanup_baseline EXIT
trap 'cleanup_baseline; exit 130' INT TERM
if [ -n "$ARTIFACT_DIR" ]; then mkdir -p "$ARTIFACT_DIR"; fi

if command -v pgrep >/dev/null 2>&1 && command -v ps >/dev/null 2>&1; then RSS_METRICS_AVAILABLE=1; else RSS_METRICS_AVAILABLE=0; fi
if command -v lsof >/dev/null 2>&1; then FD_METRICS_AVAILABLE=1; else FD_METRICS_AVAILABLE=0; fi
if [ "$RSS_METRICS_AVAILABLE" -eq 0 ] || [ "$FD_METRICS_AVAILABLE" -eq 0 ]; then
    if [ "$REQUIRE_RESOURCE_METRICS" = "1" ]; then
        echo "resource metrics unavailable: rss=$RSS_METRICS_AVAILABLE fd=$FD_METRICS_AVAILABLE" >&2
        exit 2
    fi
    echo "SKIPPED: rss_metrics=$RSS_METRICS_AVAILABLE fd_metrics=$FD_METRICS_AVAILABLE" >&2
fi

case "$ROUNDS" in
    ''|*[!0-9]*) echo "AURA_STRESS_ROUNDS must be numeric" >&2; exit 2 ;;
esac
[ "$ROUNDS" -gt 0 ] || { echo "AURA_STRESS_ROUNDS must be positive" >&2; exit 2; }
case "$DURATION_SECONDS" in
    ''|*[!0-9]*) echo "AURA_STRESS_DURATION_SECONDS must be numeric" >&2; exit 2 ;;
esac
case "$BASELINE_SAMPLES" in
    ''|*[!0-9]*) echo "AURA_STRESS_BASELINE_SAMPLES must be numeric" >&2; exit 2 ;;
esac
[ "$BASELINE_SAMPLES" -gt 0 ] || { echo "AURA_STRESS_BASELINE_SAMPLES must be positive" >&2; exit 2; }

median_file() {
    file=$1
    count=$(wc -l <"$file" | tr -d ' ')
    if [ "$count" -eq 0 ]; then echo 0; return; fi
    sort -n "$file" | awk -v n="$count" 'NR == int((n + 1) / 2) { low = $1 } NR == int((n + 2) / 2) { high = $1 } END { if (n % 2) print low; else print int((low + high) / 2) }'
}

shared_memory_count() {
    if [ -d /dev/shm ]; then
        find /dev/shm -maxdepth 1 -type f -name 'aura_plugin_*' -print 2>/dev/null | wc -l | tr -d ' '
    elif command -v lsof >/dev/null 2>&1; then
        lsof -n -c aura-plugin-host-worker 2>/dev/null | grep -c '/aura_plugin_' || true
    else
        echo 0
    fi
}

# Some lifecycle contracts intentionally create and tear down a worker before
# the first resource sample can be observed.  Those tests still verify their
# exit/quarantine semantics; they are explicitly marked as non-sampleable
# rather than turning a valid lifecycle result into a false resource failure.
resource_sample_optional() {
    case "$1" in
        minimal_clap_fixture_reconfigures_worker_without_stale_audio_format|minimal_clap_fixture_keeps_multiple_instances_isolated|two_independent_sandbox_hosts_interleave_without_cross_talk|minimal_clap_fixture_state_survives_project_v2_reload|repeated_mailbox_overruns_quarantine_then_recover_to_audio|crashing_clap_worker_is_restarted_then_quarantined|generic_worker_fault_injection_quarantines_clap|isolated_plugin_worker_survives_audio_reconfiguration|real_plugin_worker_fault_is_quarantined_without_nonfinite_audio|real_plugin_overruns_quarantine_then_recover_audio_and_midi)
            return 0 ;;
        *) return 1 ;;
    esac
}

BASELINE_SHARED_MEMORY=$(shared_memory_count)
new_worker_pids() {
    root_pid=$1
    descendant_pids() {
        parent=$1
        for child in $(pgrep -P "$parent" 2>/dev/null || true); do
            printf '%s\n' "$child"
            descendant_pids "$child"
        done
    }
    for pid in $(descendant_pids "$root_pid"); do
        command_line=$(ps -o command= -p "$pid" 2>/dev/null || true)
        case "$command_line" in
            */aura-plugin-host-worker|*/aura-plugin-host-worker-vst3|*/aura-plugin-host-worker\ *|*/aura-plugin-host-worker-vst3\ *)
                printf '%s\n' "$pid" ;;
        esac
    done
}
wait_for_worker_cleanup() {
    attempt=1
    while [ "$attempt" -le 10 ]; do
        leaked_pid=""
        for pid in ${CURRENT_TEST_WORKER_PIDS:-}; do
            if kill -0 "$pid" 2>/dev/null; then leaked_pid="$pid"; break; fi
        done
        [ -z "$leaked_pid" ] && return 0
        sleep 0.05
        attempt=$((attempt + 1))
    done
    echo "worker process leak detected: tracked_pids=${CURRENT_TEST_WORKER_PIDS:-none}" >&2
    return 1
}

run_worker_test() {
    test_name=$1
    test_key=$(printf '%s' "$test_name" | tr -cd 'A-Za-z0-9_')
    max_rss=0
    max_fds=0
    rss_observed=0
    fds_observed=0
    attempt=1
    passed=0
    retry_used=0
    CURRENT_TEST_WORKER_PIDS=""
    if ! "$SANDBOX_TEST_BIN" --list 2>/dev/null | grep -Eq "^${test_name}: test$"; then
        echo "stress test is missing from selected binary: $test_name" >&2
        return 2
    fi
    while [ "$attempt" -le 3 ]; do
        log_file=$(mktemp "${TMPDIR:-/tmp}/aura-worker-test.XXXXXX")
        run_test_command "$test_name" >"$log_file" 2>&1 &
        test_pid=$!
        CURRENT_TEST_PID=$test_pid
        while kill -0 "$test_pid" 2>/dev/null; do
            if command -v pgrep >/dev/null 2>&1; then
                for worker_pid in $(new_worker_pids "$test_pid"); do
                    case " $CURRENT_TEST_WORKER_PIDS " in
                        *" $worker_pid "*) ;;
                        *) CURRENT_TEST_WORKER_PIDS="$CURRENT_TEST_WORKER_PIDS $worker_pid" ;;
                    esac
                    rss=$(ps -o rss= -p "$worker_pid" 2>/dev/null | tr -d ' ' || true)
                    case "$rss" in
                        ''|0|*[!0-9]*) ;;
                        *) rss_observed=1; [ "$rss" -gt "$max_rss" ] && max_rss="$rss" ;;
                    esac
                    if [ "$FD_METRICS_AVAILABLE" -eq 1 ]; then
                        fds=$(lsof -p "$worker_pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')
                        case "$fds" in
                            ''|0|*[!0-9]*) ;;
                            *) fds_observed=1; [ "$fds" -gt "$max_fds" ] && max_fds="$fds" ;;
                        esac
                    fi
                done
            fi
            # Short-lived fixture workers can complete in less than one
            # scheduler tick.  Sample frequently enough to observe their
            # actual PID without changing the lifecycle under test.
            sleep 0.005
        done
        if wait "$test_pid"; then
            CURRENT_TEST_PID=""
            passed=1
            if [ "$attempt" -gt 1 ]; then
                retry_used=1
                retry_successes=$((retry_successes + 1))
                echo "WARNING: stress test passed only after retry: test=$test_name attempt=$attempt" >&2
            fi
            rm -f "$log_file"
            [ -z "${last_log:-}" ] || rm -f "$last_log"
            break
        fi
        if [ -n "$ARTIFACT_DIR" ]; then
            cp "$log_file" "$ARTIFACT_DIR/${CURRENT_ROUND:-0}-${test_key}-attempt-${attempt}.log"
        fi
        last_log="$log_file"
        CURRENT_TEST_PID=""
        attempt=$((attempt + 1))
    done
    if [ "$passed" -ne 1 ]; then
        cat "$last_log" >&2
        rm -f "$last_log"
        rm -f "$log_file"
        return 1
    fi
    if [ "$RSS_METRICS_AVAILABLE" -eq 1 ] && [ "$rss_observed" -eq 1 ]; then
        : >"$OBSERVED_DIR/${test_key}.rss"
    fi
    if [ "$FD_METRICS_AVAILABLE" -eq 1 ] && [ "$fds_observed" -eq 1 ]; then
        : >"$OBSERVED_DIR/${test_key}.fd"
    fi
    if [ "$RSS_METRICS_AVAILABLE" -eq 1 ] && [ "$rss_observed" -eq 0 ]; then
        if [ "$REQUIRE_RESOURCE_METRICS" = "1" ] &&
           [ ! -e "$OBSERVED_DIR/${test_key}.rss" ] &&
           ! resource_sample_optional "$test_name"; then
            echo "RSS metrics unavailable for test: test=$test_name" >&2
            return 1
        fi
        echo "SKIPPED_RESOURCE_METRIC: rss test=$test_name worker exited before sampling" >&2
    fi
    if [ "$FD_METRICS_AVAILABLE" -eq 1 ] && [ "$fds_observed" -eq 0 ]; then
        if [ "$REQUIRE_RESOURCE_METRICS" = "1" ] &&
           [ ! -e "$OBSERVED_DIR/${test_key}.fd" ] &&
           ! resource_sample_optional "$test_name"; then
            echo "FD metrics unavailable for test: test=$test_name" >&2
            return 1
        fi
        echo "SKIPPED_RESOURCE_METRIC: fd test=$test_name worker exited before sampling" >&2
    fi
    if [ "$RSS_METRICS_AVAILABLE" -eq 1 ] && [ "$max_rss" -gt "$MAX_RSS_KB" ]; then
        echo "RSS limit exceeded: test=$test_name max_rss_kb=$max_rss limit=$MAX_RSS_KB" >&2
        return 1
    fi
    if [ "$FD_METRICS_AVAILABLE" -eq 1 ] && [ "$max_fds" -gt "$MAX_FDS" ]; then
        echo "FD limit exceeded: test=$test_name max_fds=$max_fds limit=$MAX_FDS" >&2
        return 1
    fi
    if [ "$RSS_METRICS_AVAILABLE" -eq 1 ] || [ "$FD_METRICS_AVAILABLE" -eq 1 ]; then
        if [ "$max_rss" -gt "$peak_rss_seen" ]; then peak_rss_seen="$max_rss"; fi
        if [ "$max_fds" -gt "$peak_fds_seen" ]; then peak_fds_seen="$max_fds"; fi
        if [ "${CURRENT_ROUND:-0}" -gt "$WARMUP_ROUNDS" ]; then
            baseline_rss_file="$BASELINE_DIR/${test_key}.rss"
            baseline_fd_file="$BASELINE_DIR/${test_key}.fd"
            if [ -f "$baseline_rss_file" ]; then
                rss_sample_count=$(wc -l <"$baseline_rss_file" | tr -d ' ')
            else
                rss_sample_count=0
            fi
            if [ -f "$baseline_fd_file" ]; then
                fd_sample_count=$(wc -l <"$baseline_fd_file" | tr -d ' ')
            else
                fd_sample_count=0
            fi
            if [ "$rss_observed" -eq 1 ] && [ "$rss_sample_count" -lt "$BASELINE_SAMPLES" ]; then
                printf '%s\n' "$max_rss" >>"$baseline_rss_file"
                rss_sample_count=$((rss_sample_count + 1))
            fi
            if [ "$fds_observed" -eq 1 ] && [ "$fd_sample_count" -lt "$BASELINE_SAMPLES" ]; then
                printf '%s\n' "$max_fds" >>"$baseline_fd_file"
                fd_sample_count=$((fd_sample_count + 1))
            fi
            if [ "$rss_observed" -eq 1 ] || [ "$fds_observed" -eq 1 ]; then
                echo "stress baseline sample: test=$test_name rss=$rss_sample_count/$BASELINE_SAMPLES fd=$fd_sample_count/$BASELINE_SAMPLES rss_kb=$max_rss fds=$max_fds"
            fi
            if [ "$rss_sample_count" -ge "$BASELINE_SAMPLES" ]; then
                baseline_test_rss=$(median_file "$baseline_rss_file")
            fi
            if [ "$fd_sample_count" -ge "$BASELINE_SAMPLES" ]; then
                baseline_test_fds=$(median_file "$baseline_fd_file")
            fi
            if [ "$rss_observed" -eq 1 ] && [ "$rss_sample_count" -ge "$BASELINE_SAMPLES" ] && [ "$max_rss" -gt $((baseline_test_rss + MAX_RSS_GROWTH_KB)) ]; then
                echo "RSS growth limit exceeded: test=$test_name baseline=$baseline_test_rss current=$max_rss limit=$MAX_RSS_GROWTH_KB" >&2
                return 1
            fi
            if [ "$fds_observed" -eq 1 ] && [ "$fd_sample_count" -ge "$BASELINE_SAMPLES" ] && [ "$max_fds" -gt $((baseline_test_fds + MAX_FD_GROWTH)) ]; then
                echo "FD growth limit exceeded: test=$test_name baseline=$baseline_test_fds current=$max_fds limit=$MAX_FD_GROWTH" >&2
                return 1
            fi
        fi
        if [ "$peak_rss_seen" -gt "$MAX_RSS_KB" ] || [ "$peak_fds_seen" -gt "$MAX_FDS" ]; then
            echo "resource peak limit exceeded: peak_rss_kb=$peak_rss_seen peak_fds=$peak_fds_seen" >&2
            return 1
        fi
        echo "stress metrics: test=$test_name max_rss_kb=$max_rss max_fds=$max_fds"
    else
        echo "stress metrics: test=$test_name unavailable=1"
    fi
}

run_test_command() {
    # This function is always launched asynchronously by run_worker_test.  Do
    # not let the child execute the coordinator's cleanup trap.
    trap - EXIT INT TERM
    test_name=$1
    if [ "$test_name" = "official_vst3_fixture_instantiates_and_processes_in_the_isolated_worker" ] ||
       [ "$test_name" = "isolated_plugin_worker_survives_audio_reconfiguration" ] ||
       [ "$test_name" = "real_plugin_worker_fault_is_quarantined_without_nonfinite_audio" ] ||
       [ "$test_name" = "real_plugin_overruns_quarantine_then_recover_audio_and_midi" ]; then
        AURA_PLUGIN_HOST_BIN="$WORKER_BIN" \
        AURA_VST3_FIXTURE="$AURA_VST3_FIXTURE" \
        "$SANDBOX_TEST_BIN" "$test_name" --exact --ignored --test-threads=1
    else
        AURA_PLUGIN_HOST_BIN="$WORKER_BIN" \
        AURA_CLAP_FIXTURE="$ROOT_DIR/build-tools/minimal-gain.clap" \
        AURA_PLUGIN_PATHS="$ROOT_DIR/build-tools" \
        "$SANDBOX_TEST_BIN" "$test_name" --exact --ignored --test-threads=1
    fi
}

# Build the Rust integration-test harness once.  Starting Cargo for every
# worker case and every round measures dependency resolution and test-harness
# startup more than worker lifecycle.  Callers may provide a prebuilt binary
# when running from a staged/release tree.
SANDBOX_TEST_BIN=${AURA_STRESS_TEST_BIN:-}
if [ -z "$SANDBOX_TEST_BIN" ]; then
    cargo test -p aura-core-bridge --test plugin_sandbox_workflow --no-run --quiet
    SANDBOX_TEST_BIN=$(find "$ROOT_DIR/target/debug/deps" -type f -perm -111 \
        -name 'plugin_sandbox_workflow-*' -print0 | xargs -0 ls -t 2>/dev/null | head -n 1)
fi
if [ -z "$SANDBOX_TEST_BIN" ] || [ ! -x "$SANDBOX_TEST_BIN" ]; then
    echo "unable to locate plugin sandbox stress test binary" >&2
    exit 2
fi

WORKER_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker"
if [ -n "${AURA_VST3_FIXTURE:-}" ]; then
    AURA_PLUGIN_WORKER_OUTPUT="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3" \
        "$ROOT_DIR/scripts/build_plugin_worker_with_vst3.sh" >/dev/null
    WORKER_BIN="$ROOT_DIR/build-tools/aura-plugin-host-worker-vst3"
else
    "$ROOT_DIR/scripts/build_plugin_worker.sh" >/dev/null
fi
"$ROOT_DIR/scripts/build_clap_fixture.sh" >/dev/null

i=1
start_epoch=$(date +%s)
while [ "$i" -le "$ROUNDS" ]; do
    CURRENT_ROUND=$i
    run_worker_test minimal_clap_fixture_instantiates_in_the_isolated_worker
    run_worker_test minimal_clap_fixture_processes_continuous_audio_blocks_without_stale_output
    run_worker_test minimal_clap_fixture_reconfigures_worker_without_stale_audio_format
    run_worker_test minimal_clap_fixture_keeps_multiple_instances_isolated
    run_worker_test two_independent_sandbox_hosts_interleave_without_cross_talk
    run_worker_test minimal_clap_fixture_state_survives_project_v2_reload
    run_worker_test repeated_mailbox_overruns_quarantine_then_recover_to_audio
    run_worker_test crashing_clap_worker_is_restarted_then_quarantined
    run_worker_test generic_worker_fault_injection_quarantines_clap
    if [ -n "${AURA_VST3_FIXTURE:-}" ]; then
        run_worker_test official_vst3_fixture_instantiates_and_processes_in_the_isolated_worker
        run_worker_test isolated_plugin_worker_survives_audio_reconfiguration
        run_worker_test real_plugin_worker_fault_is_quarantined_without_nonfinite_audio
        run_worker_test real_plugin_overruns_quarantine_then_recover_audio_and_midi
    fi
    wait_for_worker_cleanup
    current_shared_memory=$(shared_memory_count)
    if [ "$current_shared_memory" -gt "$BASELINE_SHARED_MEMORY" ]; then
        echo "shared memory leak detected: baseline=$BASELINE_SHARED_MEMORY current=$current_shared_memory" >&2
        exit 1
    fi
    if [ "$DURATION_SECONDS" -gt 0 ]; then
        now_epoch=$(date +%s)
        elapsed=$((now_epoch - start_epoch))
        if [ "$elapsed" -ge "$DURATION_SECONDS" ]; then
            break
        fi
    fi
    i=$((i + 1))
done
if [ "$MAX_RETRY_SUCCESSES" -ge 0 ] && [ "$retry_successes" -gt "$MAX_RETRY_SUCCESSES" ]; then
    echo "stress retry budget exceeded: retry_successes=$retry_successes limit=$MAX_RETRY_SUCCESSES" >&2
    exit 1
fi
completed_rounds=$((i - 1))
if [ "$DURATION_SECONDS" -gt 0 ]; then
    echo "native worker lifecycle stress passed: $completed_rounds rounds, duration budget=${DURATION_SECONDS}s"
else
    echo "native worker lifecycle stress passed: $completed_rounds rounds"
fi
