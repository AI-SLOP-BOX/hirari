# Aura DAW roadmap

The roadmap is ordered by evidence and user safety, not feature count.

## Preview quality gate

Current evidence already covers clean Rust setup, serialized workspace tests,
launch/headless initialization, track add/rename/delete persistence, waveform
cache invalidation, audio-format contracts, and software device transition.
The remaining bullets below require additional hardware or external fixtures.

- Reproduce build and tests from a clean macOS checkout.
- Pass launch, playback, seek, tempo, track editing, save/restart/open, missing
  media, device loss, and device recovery workflows.
- Verify PCM16, PCM24, float WAV, mono/stereo, 44.1/48/96 kHz, malformed input,
  Unicode paths, and sample-rate conversion.
- Record callback deadlines, dropouts, memory, file descriptors, and finalized
  recordings across the supported buffer-size matrix.

## Plugin and integration gate

- Publish fixture-based CLAP, AU, and SDK-enabled VST3 results separately.
- Exercise instruments and effects, editor windows, automation, state restore,
  restart, quarantine, and bounded failure recovery.
- Complete the OpenUtau edit/render/re-import round trip with timing, lyric,
  pitch, and rendered-audio evidence.

## Post-publication priority

- Add the licensed ARA2 SDK adapter and validate document, region, random-access,
  analysis, and note-segment exchange with a real partner plug-in.
- Expand Windows ASIO and third-party AU/CLAP compatibility matrices on native
  hardware; do not advertise those results from source-only CI.

## Release gate

- Test Apple Silicon, Intel, and universal app artifacts.
- Produce signed and notarized bundles, checksums, an SBOM, dependency and
  license reports, and a machine-readable capability matrix.
- Complete accessibility, multiple window-size, Retina, empty/loading/error,
  and keyboard-focus verification.

Completion is tracked by executable checks and evidence in
`docs/RELEASE_READINESS.md`; unchecked capability claims are not promoted by
roadmap text alone.
