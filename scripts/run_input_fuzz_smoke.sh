#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# Deterministic mutation corpora are a cheap, repeatable gate on every
# platform. They complement, but do not replace, coverage-guided fuzzing.
for test_name in \
    project::tests::truncated_project_layout_prefixes_never_hydrate_successfully \
    project::tests::mutated_project_layout_corpus_never_panics_or_accepts_invalid_state \
    recording_stream::tests::metadata_parser_fuzz_corpus_never_panics \
    tests::tests::plugin_state_mutation_corpus_never_panics_or_reports_success_without_storage
do
    cargo test -p aura-core-bridge --manifest-path "$ROOT_DIR/Cargo.toml" \
        "$test_name" -- --test-threads=1
done

echo "input fuzz smoke passed"
