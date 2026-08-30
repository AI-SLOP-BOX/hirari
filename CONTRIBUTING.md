# Contributing to Aura DAW

Aura welcomes focused fixes with reproducible evidence. Before editing, read
`docs/ARCHITECTURE_STATUS.md` and the nearest `SKILL.md` for the subsystem.

## Development workflow

1. Run `scripts/setup_dev.sh` from a clean checkout.
2. Keep realtime callbacks free of locks, allocation, file access, logging,
   exceptions, and unbounded work.
3. Add a regression test for behavior changes. Tests must exercise the real
   production path and must not turn missing capabilities into passes.
4. Run `cargo fmt --all -- --check`, `cargo check --workspace --locked`, and
   `AURA_NATIVE_TEST_ISOLATION=1 RUST_TEST_THREADS=1 cargo test --workspace --locked`.
5. For native or release changes, run the relevant script under `scripts/` and
   include its PASS/FAIL/SKIPPED evidence in the change description.

Do not commit app bundles, plugin binaries, voicebanks, SDKs, rendered audio,
build directories, local journals, logs, or credentials. Third-party material
must retain its original license and must be documented in
`THIRD_PARTY_NOTICES.md`.

Bug reports should include the exact commit, macOS and CPU architecture, audio
device, sample rate, buffer size, reproduction steps, and the smallest safe log
that demonstrates the failure. Never attach project audio or plugin licenses
without permission.
