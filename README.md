# Aura DAW

Aura is an open-source, programmable macOS DAW engine and early technical
preview. It combines a Rust control/UI layer, a C++ audio engine, isolated
plugin workers, project persistence, offline rendering and a JSON command
boundary intended for CLI and automation clients.

Aura is not yet a production replacement for Logic Pro. Hardware, plugin, and
long-running stability claims are capability-specific; see
[release readiness](./docs/RELEASE_READINESS.md) for the current evidence and
known gaps.

## System requirements

- macOS 14 or newer (Apple Silicon or Intel 64-bit)
- Xcode Command Line Tools, Rustup, CMake 3.20+, and approximately 10 GB of
  free disk space for a clean checkout plus build artifacts
- A clean workspace typically builds in 5–15 minutes on a current Mac; a
  first build may take longer while Cargo downloads dependencies
- FFmpeg is optional and only needed for the extended MP3/FLAC checks

Linux is supported for portable Rust/native contract checks. The production
GUI, CoreAudio, Audio Units, signing, and application bundle are macOS-only.

![Aura Cinematic Suite Mockup](https://images.unsplash.com/photo-1598488035139-bdbb2231ce04?auto=format&fit=crop&q=80&w=1200)

## Current capabilities

*   **Programmable control surface**: CLI/JSON commands with generations,
    permissions, transactions, history and dry-run-oriented inspection.
*   **Isolated plugin workers**: CLAP and macOS AU paths are exercised through
    the native worker. VST3 is enabled only in SDK-configured builds.
*   **Audio engine**: Recording, project persistence and recovery, offline
    rendering, routing, PDC, sidechain snapshots and bounded diagnostics.
*   **Open integration boundaries**: OpenUtau import/render integration and
    project-local extension commands without hard-coding a vendor UI. Trusted
    extensions can opt into a bounded JSON process protocol; discovery remains
    safe and sandboxed manifests are never auto-executed.
*   **Modular architecture**: A Rust control layer, C++ DSP/native boundary,
    Slint UI and testable command contracts.

### Adding third-party plugins

Aura scans the standard CLAP, VST3 and Audio Unit locations. Portable or
project-local plugin folders can be added without changing the source by
setting `AURA_PLUGIN_PATHS` to a native path list before launching Aura. Each
entry may be either a plugin folder or a concrete `.clap`, `.vst3`, or
`.component` bundle. The catalog records the format, worker capability and a
SHA-256 binary fingerprint; insertion still goes through the same admission
and permission checks as standard locations.

For example, on macOS:

```sh
AURA_PLUGIN_PATHS="$PWD/Plugins:$HOME/Audio/Portable" aura plugin list
```

This is an extension point, not a redistribution mechanism: plugin binaries,
vendor SDKs and voicebanks remain subject to their own licenses.

## 🛠 Project Structure

*   `/aura-ui`: The primary Rust-based GUI and command surface, built with
    Slint and Rust.
*   `/aura-core-bridge`: High-performance C++/Rust FFI layer for engine communication.
*   `/src (Root Core)`: The underlying **AuraUnifiedEngine** (Unified DSP Kernel), with generic external-plugin integration points.

## Building and contributing

See [INSTALL.md](./INSTALL.md) for a reproducible local build and strict release
verification. Contributions should follow [CONTRIBUTING.md](./CONTRIBUTING.md),
and security issues should use the private process in
[SECURITY.md](./SECURITY.md).

The workspace is self-contained: `aura-core-bridge` and `aura-ui` are ordinary
tracked directories, not private or optional submodules. A fork needs only a
normal clone:

```sh
git clone <your-fork-url> aura
cd aura
scripts/setup_dev.sh
```

`scripts/verify_repository_health.sh` fails if either workspace member is
accidentally converted back to a gitlink or its manifest is omitted. CI runs
this check before compiling so an incomplete clone cannot be published as a
passing build.

Planned quality gates are listed in [ROADMAP.md](./ROADMAP.md), and user-visible
changes are recorded in [CHANGELOG.md](./CHANGELOG.md). Participation is
governed by [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

### Five-minute first run

```sh
scripts/setup_dev.sh
cargo test --workspace --locked
scripts/build_app.sh
open "packaging/Aura DAW.app"
```

Create an audio track, import a WAV, press Play, adjust the track fader, then
use Save and reopen the project to verify the persistence path. For the exact
hardware, plugin, OpenUtau, and release checks, follow
[INSTALL.md](./INSTALL.md) and [release readiness](./docs/RELEASE_READINESS.md).
The device- and display-dependent acceptance steps are listed in the
[manual E2E checklist](./docs/MANUAL_E2E_CHECKLIST.md).

To check whether local renders, packaging outputs, or fixture checkouts have
inflated a working copy, run the read-only footprint audit:

```sh
scripts/audit_workspace_footprint.sh
```

It reports the largest generated areas and exact duplicate file contents. It
does not delete anything; set `AURA_WORKSPACE_MAX_BYTES` to enforce a local
size budget before packaging.

`AURA_RELEASE_MODE=1` also makes the packaging and release verification paths
run the strict source hygiene check automatically, so a signed release cannot
be built from a checkout that still tracks local fixture clones.

The repository also marks fixture clones and generated trees with
`export-ignore`, keeping them out of `git archive` source distributions even
before the checkout is fully cleaned up.

For a source/release review, also run `AURA_STRICT_SOURCE_HYGIENE=1
scripts/audit_repository_hygiene.sh`; this rejects tracked fixture clones such
as `third_party_synths` and `.openutau-review`.

### Build your own DAW on Aura

Aura's GUI is an API client, not an engine dependency. The Slint UI and CLI
both queue mix renders through the versioned `aura.core.v1` boundary, while
the C++ engine and Rust Core remain independent of Slint. A normal mix can be
rendered without opening a window or an audio device:

```sh
cargo run -p aura-core-bridge --bin aura -- render mix song.aura mix.wav
```

The same boundary is available to a custom Rust client:

```sh
cargo run -p aura-core-bridge --example headless_render -- song.aura mix.wav
```

Aura can also be used without an Aura project. This builds a temporary audio
graph, inserts a catalog plugin through the normal sandbox/native admission
path, applies gain, and renders the result:

```sh
cargo run -p aura-core-bridge --example process_audio -- \
  input.wav output.wav "Aura Compressor" -3
```

Replace `Aura Compressor` with a catalog alias such as `Plugin Name@clap` or
`Plugin Name@vst3` when that worker backend is available in the build. The
example contains no Slint code and does not create or load an Aura project.

See `aura-core-bridge/src/stable_api.rs` for the compatibility-versioned
request and result types. Future GUI, Python, WebSocket, and AI adapters should
use this boundary rather than calling the native bridge directly.

The cross-media foundation is `aura.timeline.v1`. `MasterClock` is the shared
coordinate system for audio samples, video frames, subframes, and SMPTE; the
same timeline can carry a piecewise tempo map and neutral parameter bindings
such as `audio.synth.cutoff -> vfx.glow.intensity`. Audio and VFX cores remain
separate modules and can adopt this contract independently.

`aura.events.v1` provides the companion subscription contract. Consumers keep
an `EventCursor`, receive ordered envelopes with a generation number, and are
explicitly told when bounded history was missed so they can refresh a snapshot
before applying changes. This makes GUI, VFX, CLI, and AI clients use the same
command/event model instead of polling every parameter.

VFX-side parameter connections use `aura.vfx-bindings.v1`. A binding graph can
interpolate values on the shared clock, for example
`audio.synth.cutoff -> vfx.glow.intensity`, while keeping Audio and VFX cores
independent. The same graph can later consume MIDI, marker, or stem-derived
events without changing either engine.

For an OpenUtau render round-trip, provide a real renderer command whose `$1`
and `$2` arguments are the source `.ust/.ustx` and output WAV:

```sh
AURA_OPENUTAU_SOURCE=... AURA_OPENUTAU_RENDER_OUTPUT=... \
AURA_OPENUTAU_RENDER_COMMAND='your-renderer "$1" "$2"' \
scripts/run_openutau_roundtrip.sh
```

### Feature status and known limitations

The current preview includes project persistence, MIDI/chord-track data,
warp-marker metadata, routed offline rendering, loudness telemetry, and
isolated worker lifecycle checks. The engine also includes SDK-disabled VST3
component/controller creation, macOS AU/VST3 native-editor attach/detach,
asynchronous waveform decoding, measured-HRTF injection, Vibrato Rate
editing, and native-renderer-backed export queues. Hardware, vendor SDK, and
long-running stability claims remain capability-specific. The next major
integration target is formal ARA2 partner-host exchange; VariAudio-style
note-level pitch/formant editing, phase-coherent multitrack warp, Windows
ASIO certification, Dolby Atmos object metadata, and physical-controller
certification remain future work. See [UNIMPLEMENTED_GAPS.md](./docs/UNIMPLEMENTED_GAPS.md)
and [release readiness](./docs/RELEASE_READINESS.md) for evidence and exact
boundaries.

## ⚖️ Licensing

Aura-owned source is published under the **MIT License**; see
[LICENSE](./LICENSE). This does not relicense third-party code or
installed plugin binaries.

The GUI uses Slint, which is offered under GPL-3.0-only or one of Slint's
separate licenses. Distributors must select and comply with an applicable Slint
license for combined binaries; Aura's MIT license applies only to Aura-owned
source and does not override Slint's terms.

The packaged desktop application uses Slint's royalty-free option and carries
the required `AboutSlint` attribution on the Help/Diagnostics surface. The
royalty-free terms cover Slint as part of an application; they do not grant a
right to redistribute Slint as a standalone library, and do not permit an
application that exposes Slint's APIs. Aura's stable API is intentionally
Slint-free; distributors must not re-export Slint types or handles. A
distributor choosing the GPL option must follow GPL-3.0 instead.

Aura may also ship optional GPL-3.0-only tools or frontends as separate
components. Those components are clearly marked and do not change the MIT
license of the engine, stable API, or other Aura-owned modules. A distributor
that combines a GPL component into one application must comply with GPL-3.0;
the MIT-only core remains available for permissive forks and proprietary
integrations.

OpenUtau is integrated through a bridge and is kept as a separately licensed
upstream project. Surge XT, Vital, voicebanks, and user audio assets are not
redistributed by the source repository. See
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md) and the
[publication scope](./docs/PUBLICATION_SCOPE.md).

When building a combined distribution with third-party components, also read
[LICENSE-COMBINED-DISTRIBUTION.md](./LICENSE-COMBINED-DISTRIBUTION.md).

Public capability claims and the strict release gate are documented in
[docs/RELEASE_READINESS.md](./docs/RELEASE_READINESS.md).

Local preview bundles use an ad-hoc signature for launch testing. Before
commercial distribution, run `scripts/sign_and_notarize_release.sh` with
`AURA_CODESIGN_IDENTITY` set to a Developer ID Application identity; set
`AURA_NOTARY_PROFILE` to require notarization, stapling, and validation.

---
Copyright (c) 2024-2026 Aura DAW Project.
