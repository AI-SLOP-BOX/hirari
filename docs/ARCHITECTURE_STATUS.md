# Aura Studio Architecture Status

This document records the current production source of truth while the
legacy native paths are being retired. It is intentionally explicit about
what is canonical and what remains compatibility or contract-test code.

## Runtime source of truth

| Area | Canonical path | Compatibility / test-only paths |
| --- | --- | --- |
| Application | `aura-ui` Rust/Slint binary | Historical app bundles outside `packaging/` |
| Project model | `aura-core-bridge::ProjectDocument` | Native serializer used for rollback snapshots and legacy import |
| Runtime engine | `aura-core-bridge::AuraCore` + its owned native `AudioEngine` | Header/translation-unit contracts under `tests/` |
| Native audio graph | The `AudioEngine` owned by one `AuraCore` session | Older direct `AuraUnifiedEngine` entry points |
| External plugins | Isolated worker process and format adapter | Legacy in-process host wrappers |
| Release bundle | `packaging/Aura DAW.app` | Historical bundles are not release inputs |
| Release gate | `scripts/verify_release_bundle.sh` via `aura-verify-release` | Ad-hoc smoke scripts are capability tests |

## UI and rendering split

The desktop shell is Slint. The default Cargo configuration selects Slint's
WGPU 29 backend and installs a rendering notifier that uses the same device and
queue as the Slint frame. Shared plot frames are uploaded as RGBA textures and
imported back as Slint images for Arrange, Mixer, Synth, Waveform, Mastering
and Piano Roll surfaces. An opt-in `gpu-canvas` feature keeps a standalone
native `wgpu` 0.20 WGSL pipeline for renderer-level tests and integrations;
it is not a second window. Slint continues to own layout, accessibility and
input while dense plot pixels are produced by the GPU path. The separate C++
graphics layer under `src/graphics/` remains a compatibility path for Metal
and opt-in Vulkan experiments. The notifier tracks waveform, spectrum, meter,
and piano-note streams independently, so a meter-only update does not rebuild
the other three textures.

## Project lifecycle rules

1. A project load is a control-thread transaction.
2. All filesystem inputs are validated before mutating the native graph.
3. Native hydration creates a rollback snapshot before the first mutation.
4. Any failed track, region, parameter, or layout restore rolls back the
   native graph and reports failure to the caller.
5. A successful load removes the rollback snapshot.
6. Audio configuration and sandbox IPC generations must not be reused by a
   later project session.

## Release capabilities

The release bundle currently has a mandatory CLAP worker smoke. AU and VST3
are capability-gated because their SDKs, host permissions, and real fixtures
are platform-dependent:

| Capability | Required for every bundle | Required when enabled in CI |
| --- | ---: | ---: |
| App bundle and native worker | Yes | Yes |
| CLAP worker handshake/process/recovery | Yes | Yes |
| AU real-device smoke | No | Yes on macOS AU jobs |
| VST3 SDK worker/E2E | No | Yes when `AURA_VST3_SDK` and fixture are present |

The build target `aura-verify-release` is the strict entry point. It builds
the app, builds the CLAP fixture, runs the full test gate, and then verifies
the bundle. Architecture can be overridden with `-DAURA_RELEASE_ARCH=...` for
cross-builds; the bundle verifier remains the authority for the actual binary
architecture.

## Legacy asset policy

Files outside the canonical paths above must not be silently restored or
deleted by a feature change. A legacy asset is removable only when one of the
following is true:

- the replacement path is listed in this document;
- a release script no longer references it; and
- the removal is intentional and recorded in the change description.

This keeps the large Rust/Slint migration auditable without treating old
artifacts as production inputs.

## Next migration boundaries

- Replace boolean/empty-string FFI failures with a typed bridge error at new
  API boundaries first; retain compatibility wrappers until callers migrate.
- Consolidate WAV read/write behind one validated format contract.
- Add AU/VST3 capability rows only when a real fixture and runtime smoke exist.
- Extend generation checks to every native cache commit, not only Rust
  waveform caches.
