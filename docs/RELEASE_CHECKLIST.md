# Release evidence checklist

| Requirement | Automated evidence | Current state |
| --- | --- | --- |
| Reproducible Rust setup | `scripts/setup_dev.sh`, pinned `rust-toolchain.toml`, tracked `Cargo.lock` | PASS locally and in CI |
| Workspace build and tests | `.github/workflows/ci.yml`, serialized `cargo test --workspace --all-targets --locked` | PASS |
| App package and launch | `scripts/run_release_smoke.sh`, `scripts/run_app_launch_smoke.sh` | PASS on current macOS host |
| Bundle integrity | `codesign`, `scripts/verify_release_metadata.sh` | Ad-hoc signature PASS; notarization pending credentials |
| Dependency security | `cargo audit`, `cargo deny`, `.github/workflows/security.yml` | PASS; unmaintained warnings tracked |
| Audio formats and waveform contracts | workspace audio tests and FFmpeg contract | PASS for covered fixtures |
| Realtime callback budget | `scripts/run_realtime_performance_matrix.sh` | PASS: zero deadline misses in software graph |
| Hardware/CoreAudio | `scripts/run_real_device_matrix.sh` | Device-specific; record PASS/FAIL/SKIPPED report |
| Third-party plugins | `scripts/run_plugin_compatibility_matrix.sh` plus format fixtures | Inventory and isolated fixture evidence; vendor equivalence pending |
| OpenUtau round trip | OpenUtau project/render/re-import checklist | Requires installed OpenUtau and voicebank |
| Display/accessibility/manual transport | `docs/MANUAL_E2E_CHECKLIST.md` | Requires human GUI run |

Never promote a `SKIPPED`, timeout, or missing external fixture to PASS. Attach
the generated logs and reports to the release review.
