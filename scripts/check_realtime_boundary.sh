#!/usr/bin/env bash
set -euo pipefail

# Static guard for the native audio callback boundary. This is deliberately
# conservative: a new allocation, lock, filesystem call, or blocking wait in
# processBlock must be reviewed explicitly instead of silently entering the RT
# path through a refactor.
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
callback_file="$root_dir/src/core/aura_unified_engine_part_1.inc"
chain_file="$root_dir/src/core/effect_chain.hpp"
sandbox_file="$root_dir/src/core/plugins/plugin_sandbox_host.hpp"

callback_body="$(awk '
  /void AuraUnifiedEngine::processBlock\(/ {inside=1}
  inside {print}
  inside && /^    }$/ {exit}
' "$callback_file")"

forbidden='lock_guard|unique_lock|std::mutex|std::string|std::vector|make_shared|make_unique|new |delete |ifstream|ofstream|filesystem|sleep_for|condition_variable'
if printf '%s\n' "$callback_body" | rg -n "$forbidden"; then
    echo "realtime boundary violation: forbidden operation found in processBlock" >&2
    exit 1
fi

direct_callback_body="$(awk '
  /void AuraUnifiedEngine::processBlockDirect\(/ {inside=1}
  inside {print}
  inside && /^}$/ {exit}
' "$callback_file")"
if ! printf '%s\n' "$direct_callback_body" | rg -q "validInput" ||
   ! printf '%s\n' "$direct_callback_body" | rg -q "kMaxAudioBlockSize"; then
    echo "realtime boundary violation: direct callback input guard is missing" >&2
    exit 1
fi

if ! rg -q "ReaderGuard reader\(m_audioReaders, m_audioMutation\)" "$chain_file"; then
    echo "realtime boundary violation: EffectChain reader/mutation guard is missing" >&2
    exit 1
fi

if ! rg -q "beginAudioMutation\(\)" "$chain_file"; then
    echo "realtime boundary violation: control-thread mutation barrier is missing" >&2
    exit 1
fi

# The sandbox process() path is also an audio-thread boundary. It may use
# atomics and preallocated shared memory, but must not acquire locks, perform
# filesystem/process control, or allocate while exchanging a block.
sandbox_body="$(awk '
  /bool process\(AudioBuffer& audio, MidiBuffer& midi\)/ {inside=1}
  inside {print}
  inside && /^    }$/ {exit}
' "$sandbox_file")"
if printf '%s\n' "$sandbox_body" | rg -n 'lock_guard|unique_lock|std::mutex|std::string|std::vector|make_shared|make_unique|new |delete |ifstream|ofstream|filesystem|sleep_for|waitpid|kill\('; then
    echo "realtime boundary violation: forbidden operation found in PluginSandboxHost::process" >&2
    exit 1
fi

echo "realtime boundary checks passed"
