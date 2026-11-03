#!/usr/bin/env bash
set -euo pipefail

# Static guard for the native audio callback boundary. This is deliberately
# conservative: a new allocation, lock, filesystem call, or blocking wait in
# processBlock must be reviewed explicitly instead of silently entering the RT
# path through a refactor.
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
chain_host_file="$root_dir/hirari-core-bridge/src/effect_chain_host.rs"

# The unified engine is split across *.inc parts, so search every part rather
# than a single hardcoded file name. A guard that points at a stale path
# silently stops guarding anything.
shopt -s nullglob
engine_parts=("$root_dir"/src/core/hirari_unified_engine_part_*.inc)
shopt -u nullglob

if [ "${#engine_parts[@]}" -eq 0 ]; then
    echo "realtime boundary check error: no unified engine sources found" >&2
    exit 2
fi
if [ ! -f "$chain_host_file" ]; then
    echo "realtime boundary check error: Rust chain host not found" >&2
    exit 2
fi

# ripgrep is not a declared dependency of this repository. Prefer it when it is
# available, and fall back to grep so the guard still runs on a plain toolchain.
if command -v rg >/dev/null 2>&1; then
    search() { rg --no-heading --line-number --color=never "$@"; }
    search_quiet() { rg --quiet "$@"; }
else
    search() { grep -En "$@"; }
    search_quiet() { grep -Eq "$@"; }
fi

# Patterns that must not appear inside a Rust-owned audio-path function.
rust_forbidden='eprintln!|println!|print!|dbg!|format!|vec!|to_string\(\)|String::from|\.lock\(\)|std::fs|std::thread::sleep'

# Extract a function body by counting braces. Matching a closing-brace pattern
# truncates at the first early "}" and silently inspects only a fraction of the
# function, so the depth counter has to drive the exit instead.
extract_function() {
    local signature="$1"
    shift
    awk -v sig="$signature" '
        index($0, sig) && !started { started = 1 }
        started {
            # Blank out string and character literals so a brace inside a
            # string cannot unbalance the depth counter.
            line = $0
            gsub(/"(\\.|[^"\\])*"/, "\"\"", line)
            gsub(/'"'"'(\\.|[^'"'"'\\])*'"'"'/, "'"''"'", line)
            print line
            opens = gsub(/\{/, "{", line)
            closes = gsub(/\}/, "}", line)
            depth += opens - closes
            if (started && depth <= 0 && saw_brace) exit
            if (opens > 0) saw_brace = 1
        }
    ' "$@"
}

assert_clean_body() {
    local body="$1" label="$2" pattern="$3"
    if [ -z "$body" ]; then
        echo "realtime boundary check error: could not locate $label" >&2
        exit 2
    fi
    local hits
    hits="$(printf '%s\n' "$body" | search "$pattern" || true)"
    if [ -n "$hits" ]; then
        echo "realtime boundary violation: forbidden operation found in $label" >&2
        echo "$hits" >&2
        exit 1
    fi
}

forbidden='lock_guard|unique_lock|std::mutex|std::string|std::vector|make_shared|make_unique|new |delete |ifstream|ofstream|filesystem|sleep_for|condition_variable'

callback_body="$(extract_function 'HirariUnifiedEngine::processBlock(' "${engine_parts[@]}")"
assert_clean_body "$callback_body" "HirariUnifiedEngine::processBlock" "$forbidden"

direct_callback_body="$(extract_function 'HirariUnifiedEngine::processBlockDirect(' "${engine_parts[@]}")"
if ! printf '%s\n' "$direct_callback_body" | search_quiet 'validInput' ||
   ! printf '%s\n' "$direct_callback_body" | search_quiet 'kMaxAudioBlockSize'; then
    echo "realtime boundary violation: direct callback input guard is missing" >&2
    exit 1
fi

# The chain's realtime traversal is Rust-owned. The entry point admits the
# callback as a reader against the published snapshot, and every control-thread
# edit goes through a mutation entry that latches readers out first.
if ! search_quiet 'hirari_effect_chain_runtime_enter_audio' "$chain_host_file"; then
    echo "realtime boundary violation: chain realtime entry does not gate readers" >&2
    exit 1
fi

if ! search_quiet 'fn begin_mutation' "$chain_host_file"; then
    echo "realtime boundary violation: control-thread mutation entry is missing" >&2
    exit 1
fi

# The traversal body may use atomics and preallocated scratch planes, but a
# lock, allocation or filesystem call there blocks the audio thread while a
# control thread edits the graph.
chain_process_body="$(extract_function 'fn hirari_effect_chain_process(' "$chain_host_file")"
assert_clean_body "$chain_process_body" "hirari_effect_chain_process (Rust)" "$rust_forbidden"

# The CoreAudio device callback is owned by Rust now, so the C++ guard above
# cannot see it. Printing, formatting, allocating, or locking there blocks the
# audio thread exactly like the C++ violations this script exists to catch.
rust_driver="$root_dir/hirari-core-bridge/src/mac_audio_driver.rs"
rust_callback_body="$(extract_function 'fn audio_unit_render_callback(' "$rust_driver")"
assert_clean_body "$rust_callback_body" "audio_unit_render_callback (Rust)" "$rust_forbidden"

echo "realtime boundary checks passed"
