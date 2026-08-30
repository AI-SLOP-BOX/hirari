# Building Aura DAW

Aura is currently developed and released on macOS. Linux is used for portable
Rust and native contract checks, but the production GUI, CoreAudio device path,
Audio Units, signing, and app bundle require macOS.

## Requirements

- macOS 14 or newer
- Apple Silicon or Intel 64-bit Mac
- Xcode Command Line Tools
- Rustup; the repository selects the exact toolchain in `rust-toolchain.toml`
- CMake 3.20 or newer for the aggregate verification targets
- FFmpeg for optional MP3 and FLAC import/export checks

Install the Xcode tools once:

```sh
xcode-select --install
```

Install Rust from <https://rustup.rs>, then prepare a checkout:

```sh
scripts/setup_dev.sh
```

On a disposable or freshly provisioned machine, the same command can install
the minimal Rust toolchain automatically:

```sh
AURA_AUTO_INSTALL_RUST=1 scripts/setup_dev.sh
```

The installer is opt-in and uses the official rustup endpoint over TLS; without
the variable, the script never changes the host toolchain.

## Build and test

```sh
cargo build --workspace --locked
AURA_NATIVE_TEST_ISOLATION=1 RUST_TEST_THREADS=1 cargo test --workspace --locked
```

The workspace suite is serialized because several integration contracts own
process-global macOS audio and plugin-host resources. Parallel lifecycle stress
is exercised separately by `scripts/run_parallel_worker_smoke.sh`.

Build the macOS application bundle:

```sh
scripts/build_app.sh
```

Verify the normal GUI launch path (the smoke test closes the app afterward):

```sh
scripts/run_app_launch_smoke.sh
```

For device- and display-dependent acceptance, use
[`docs/MANUAL_E2E_CHECKLIST.md`](docs/MANUAL_E2E_CHECKLIST.md).

The result is written to `packaging/Aura DAW.app`. Generated bundles, plugins,
rendered audio, logs, and local SDKs are intentionally excluded from Git.

Generate release SBOMs and a checksum for the complete app bundle:

```sh
scripts/generate_release_metadata.sh release-metadata "packaging/Aura DAW.app"
```

This writes CycloneDX SBOMs, `SHA256SUMS`, and a portable app ZIP under
`release-metadata/`.

Verify the generated release files before publishing:

```sh
scripts/verify_release_metadata.sh release-metadata
```

To review a dirty checkout without deleting user work, generate a classified
status report:

```sh
scripts/classify_worktree_changes.sh
```

The report separates product source, tooling, documentation, third-party
assets, and generated artifacts so cleanup can be reviewed before committing.

For a release candidate, run the strict gate:

```sh
cmake -S . -B build
cmake --build build --target aura-verify-release
```

For a lightweight packaged-app smoke (build, headless readiness, signature,
SBOM, and checksum verification), run:

```sh
scripts/run_release_smoke.sh
```

VST3, third-party plugin, hardware-device, signing, and notarization checks need
their corresponding local SDKs, fixtures, devices, and credentials. A missing
capability must be reported as unavailable, never as a successful test.

## Distribution signing and notarization

Local development bundles are ad-hoc signed. For distribution, provide a
Developer ID Application certificate and a notarization profile through the
macOS Keychain (never commit credentials):

```sh
codesign --force --deep --options runtime \
  --sign "Developer ID Application: YOUR TEAM" "packaging/Aura DAW.app"
ditto -c -k --sequesterRsrc --keepParent \
  "packaging/Aura DAW.app" "Aura-DAW.zip"
xcrun notarytool submit "Aura-DAW.zip" --keychain-profile "aura-notary" --wait
xcrun stapler staple "packaging/Aura DAW.app"
spctl --assess --type execute --verbose=4 "packaging/Aura DAW.app"
```

Replace the certificate and Keychain profile with values owned by the release
operator. `spctl` is expected to reject the local ad-hoc build until this
distribution flow has been completed.
