# Changelog

All notable user-visible changes will be recorded here. Aura currently has no
stable release; entries describe the development preview and do not imply
production support.

## Unreleased

### Added

- Reproducible Rust toolchain selection and one-command developer setup.
- Strict dependency lockfile use in CI.
- Public contribution and security policies.
- Opt-in automatic rustup bootstrap via `AURA_AUTO_INSTALL_RUST=1`.
- AU/VST3/CLAP compatibility inventory reports with architecture and
  FOUND/SKIP status.
- Long-duration realtime callback soak and bounded external-plugin probes.
- Track rename/delete operations in the project model and CLI, with dependent
  regions and routes cleaned up transactionally.
- Waveform cache invalidation API that clears stale peaks across generations.
- Audio compatibility contracts for Unicode paths, malformed/empty files,
  mixed 44.1/48/96 kHz exports, and Missing-asset relinking.
- Explicit `transport_pause` command for clients that need pause semantics
  distinct from stop while retaining the current playhead.
- macOS GUI launch smoke test and a manual device/display E2E acceptance
  checklist.
- App-bundle release metadata packaging with SBOMs, portable ZIP output,
  SHA-256 generation, and tamper-detecting verification.
- macOS CI bundle job covering headless readiness, signature, and release
  metadata verification.
- Optional one-command installation of `cargo-audit` and `cargo-deny` via
  `AURA_AUTO_INSTALL_SECURITY=1`.

### Changed

- Rust formatting is a blocking CI check.
- Native-engine workspace tests run serially to protect the process-wide FFI
  lifecycle.
- Real-device AU/CLAP probes time out after 90 seconds and preserve logs.

### Known limitations

- Hardware, third-party plugin, OpenUtau round-trip, signing, notarization, and
  long-duration evidence remains machine-specific.
- Windows and Linux are contract-check platforms, not production GUI releases.
