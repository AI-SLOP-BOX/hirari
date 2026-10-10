# Hirari project persistence contract

Hirari has one user-facing project authority: `hirari-core-bridge`'s
`ProjectDocument`. Rust persistence owns the schema version, generation, asset
manifest, plugin state envelope, and atomic publication of `.hirari` files.

The native C++ serializer is retained only as a compatibility snapshot for
the native engine and hydrate rollback. It is not a second user-facing project
format and must never be used to publish a project file directly.

The explicit `ARCHIVE PROJECT` operation writes a single-file
`.hirari-package` ZIP with `project.json`, `manifest.json`, and copied
referenced media. The manifest records each packaged path, size, and SHA-256;
it does not expose the source machine's absolute paths. Restore verifies the
ZIP and extracts only manifest-listed paths into a staging directory before
publishing the restored project. The live `.hirari` remains lightweight and
continues to refer to audio and freeze media by path.

Audio output routes and parallel bus sends are stored in the ProjectDocument
`audio_routes` array. Older entries omit `send` and `pre_fader` and load as
normal output routes. The native compatibility snapshot is version 41; versions
through 39 load every route as a normal output route, while version 40 records
the send and pre-fader flags. Version 41 adds region-local source/timeline warp
anchors; older snapshots load with no anchors. Send destinations must reference
Bus tracks.

The shared `ProjectDocument` MIDI-note contract stores scheduled notes,
lyrics, phonemes, pitch curves, vibrato depth and rate, portamento,
probability, and repeat count. Current saves put those values in `.hirari`
and publish no current-generation `.midi.json` file. A successful save also
removes the old current-generation MIDI and Control Room duplicates; historical
backup generations remain available for recovery. Documents predating the
typed MIDI-note field and native legacy projects may still migrate notes from
an old `.midi.json` file. Once the canonical field exists, its empty array is
authoritative and cannot be overridden by a stale sidecar. If an older typed
note record lacks the vibrato-rate field, a legacy sidecar may supply that
value without replacing the canonical note set.

Each current recovery copy is the atomic `.hirari` generation, containing
MIDI events, comping, Control Room, and the other project state. Recovery
reads older generation-linked `.midi.json`, `.midi-events.json`, and
`.comping.json` sidecars only where the legacy/native format requires them.
Legacy ARUA projects remain readable and are converted to ProjectDocument on
their next save. MIDI authoring metadata must remain attached through note
edits, undo/redo, save/reload, and recovery.

UI/Core project loading hydrates a private staging model. The visible track
model is replaced only after Core load and all UI hydration steps succeed. If
loading or hydration fails, Core is restored from a canonical `.hirari`
checkpoint and the original UI-only overlays and transient selection are
restored before publishing the rollback model.

## Publication rules

- Writes use a unique sibling staging file, flush the file, atomically rename,
  and sync the parent directory where the platform supports it.
- Autosave, manual save, recovery, and recording publication use separate
  operation locks and monotonically increasing generations.
- A stale generation may not publish a render, waveform, asset, or project
  snapshot.
- Native hydrate is transactional: the current native snapshot is restored if
  any track, region, plugin, or audio configuration step fails.
- Plugin state is carried in the Rust state envelope with version, payload
  size, checksum, and completion marker. Native snapshots may be discarded
  after a successful hydrate.

## Identity rules

Track and region IDs are stable within the project document. On native reload,
allocators are reseeded above the highest restored track ID and region IDs are
allocated from a fresh document generation, preventing an edit after reload
from reusing an existing identity.

Commands must include the project and audio-configuration generations they
were based on when issued by an external client. The bridge rejects stale
commands before mutation.

## Automation value domains

Track automation point times are integral sample positions, and curve values
use `-1..1`. The value range depends on the lane: volume uses `0..2`, pan uses
`-1..1` (`-1` left, `0` center, `1` right), and track delay uses `0..1` mapped
to the engine's bounded delay range. These are the same values used by the
audio graph and realtime write recorder; the UI maps them to its vertical
display range without changing the saved value. Contract version 2 records
these parameter-specific ranges. Version 1 projects are upgraded on load
without changing their existing automation point values.
