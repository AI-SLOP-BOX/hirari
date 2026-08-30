#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
SRC="$ROOT/aura-core-bridge/src"

# These modules publish work after an asynchronous boundary.  Keep the list
# explicit: adding a new publisher without choosing a generation policy should
# fail review instead of silently accepting stale completions.
for file in \
  "$SRC/auto_save_manager.rs" \
  "$SRC/parallel_bounce_orchestrator.rs" \
  "$SRC/offline.rs" \
  "$SRC/waveform_cache.rs"; do
  test -f "$file"
  if ! rg -q 'use crate::generation_gate::GenerationGate;|use crate::job_system::PublicationGate;' "$file"; then
    echo "async generation audit failed: missing GenerationGate import in $file" >&2
    exit 1
  fi
done

if ! rg -q 'GenerationGate as PublicationGate' "$SRC/job_system.rs"; then
  echo "async generation audit failed: PublicationGate is not an alias of GenerationGate" >&2
  exit 1
fi

if rg -n 'struct (PublicationGate|AsyncGeneration|RenderGeneration)' "$SRC"; then
  echo "async generation audit failed: duplicate generation gate type detected" >&2
  exit 1
fi

if ! rg -q 'pub struct GenerationGate' "$SRC/generation_gate.rs"; then
  echo "async generation audit failed: canonical GenerationGate is missing" >&2
  exit 1
fi

echo "async generation gate audit passed"
