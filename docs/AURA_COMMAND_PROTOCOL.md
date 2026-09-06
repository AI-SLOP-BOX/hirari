# Aura Command Protocol

Aura exposes a provider-neutral JSONL interface. Pi, Claude Code, Codex,
Python, shell scripts, and other hosts use the same contract.

Hosts can discover the contract without executing a project command:

```sh
aura --capabilities
```

The response lists supported protocol/schema versions, operations, transport,
permissions, and safety requirements. Hosts must treat that response as the
source of truth rather than maintaining a provider-specific adapter.

```sh
printf '%s\n' '{"protocol":"aura.command.v1","request_id":"1","client":"codex","command":{"transaction":"inspect","permission":"read_only","actions":[{"op":"project_inspect"}]}}' \
  | aura --jsonl
```

Each input line produces exactly one output line. `request_id` is echoed so a
host can safely multiplex requests. The CLI never treats project text as an
instruction; only validated JSON fields are executable.

## Safety

- Default mode is `dry_run`.
- Editing requires `permission: project_write` or `system_write`.
- Applying requires an explicit host approval flag.
- Unsupported protocol versions fail with a stable error code.
- Validation failures do not reach the engine.

The standalone CLI provides the provider-neutral preview boundary and a
restricted native apply path. With `--apply --approve` and
`AURA_PROJECT_PATH`, it loads the project, rechecks live project/audio
generations, acquires the transaction lock, advances the request ledger, and
executes the currently supported native actions. Unsupported actions fail
explicitly; they are never treated as no-ops. A GUI or another host can use the
same validated command and add the remaining executor actions without changing
the wire protocol.

Multi-action apply uses a compensating transaction for structural insertions
that have a native inverse (new tracks and plugins). If a later action fails,
those insertions are removed in reverse order. Parameter edits, project saves,
renders, and other external side effects are reported as non-rollbackable
when mixed into a failing transaction; callers should keep those in a separate
approved transaction.

For retry-safe local automation, set `AURA_LEDGER_PATH` (or set
`AURA_PROJECT_PATH`, which derives a ledger at `.aura/request-ledger.json`).
Successful applied responses are recorded by `request_id` and transaction;
replaying the same request returns `replayed: true` and the original result
without executing the action again. The ledger is durably advanced from
`prepared` to `applying` immediately before the native side effect, so a crash
after that boundary is surfaced as `ledger_in_flight` and cannot be silently
replayed. Recovery must reconcile that request (or explicitly mark it failed)
before a new mutation is accepted.

Example native apply invocation:

```sh
printf '%s\n' '{"protocol":"aura.command.v1","request_id":"r-1","client":"script","command":{"transaction":"add-vocal","permission":"project_write","expected_generation":42,"expected_audio_generation":7,"actions":[{"op":"add_track","name":"Vocal FX","track_type":0}]}}' \
  | env AURA_PROJECT_PATH=./Song.aura aura --jsonl --apply --approve
```

Example dry-run response:

```json
{"protocol":"aura.command.v1","request_id":"1","ok":true,"result":{"mode":"dry_run","transaction":"vocal-polish","destructive":false,"diff":[{"index":0,"summary":"add track: Vocal FX","destructive":false}]},"error":null}
```

## Stable response errors

`invalid_request`, `unsupported_protocol`, `validation_failed`, and
`approval_required` are machine-readable codes. Human-readable `message`
fields are diagnostic only and must not be parsed by integrations.

The protocol intentionally has no LLM provider fields. A host may identify
itself in `client`, but that value does not change command semantics.

`bounce_project` is backed by the native offline audio engine. It publishes
only validated output and supports the native RIFF/RF64 path (format `0`),
WAVE64 path (format `1`), and the allowlisted external FFmpeg paths for MP3
and FLAC. External codec output is staged through a bounded float32 WAV and
published atomically only after the encoder succeeds. The operation is
asynchronous at the UI/native boundary and uses the same render cancellation
and generation guards as the interactive render path. AIFF and AAC remain
explicitly unsupported.

Native audio import accepts WAV/RF64 directly. MP3, FLAC, AIFF/AIF, M4A, OGG
and AAC imports use the
same allowlisted, shell-free FFmpeg boundary as external export and then pass
through the canonical bounded WAV decoder; failed conversion leaves no
published temporary asset.

`save_project` from the CLI writes the canonical ProjectDocument v2 format;
it is safe to use as the hand-off point between separate CLI invocations.
Legacy native serialization remains a compatibility path for the UI, but is
not used as the CLI project interchange format.

`project_load` is the corresponding canonical v2 hydration action. It is an
irreversible project mutation, requires both live generations and explicit
approval, and performs native/sidecar rollback internally when hydration
fails. The command boundary validates every project/output path against the
directory containing `AURA_PROJECT_PATH`; external paths require the explicit
`unrestricted` permission and its separate audit policy.

```json
{"op":"project_load","path":"./Alternate.aura"}
```

`aura project inspect <path>` accepts both JSON project documents and the
native `ARUA` container used by the desktop engine. For a native container it
performs a bounded header/CRC inspection and reports the binary version, sample
rate, tempo, track/region counts, and whether full hydration must be delegated
to the native engine. A real `.aura` file therefore no longer surfaces a
misleading JSON parse error.

The canonical OpenUtau action name is `open_utau_import`; the legacy spelling
`openutau_import` is accepted as a compatibility alias. The response carries
the source/render SHA-256 identities, source note count, source singer
identities, and inspected WAV shape, so clients can reject a stale external
render or a mismatched voicebank before applying a destructive edit.

`bounce_stems` renders one WAV per track using a render-only solo selection.
The original solo state is restored and the selection does not enter Undo/Redo:

```json
{"op":"bounce_stems","output_dir":"./stems","format":0}
```

`insert_plugin_path` is the extensibility entry point for a concrete CLAP,
VST3, or AU bundle. Project-write clients may use system or user plugin paths
only when the scanner has admitted that exact bundle; unrestricted clients
still pass through the native plugin admission checks. This lets newly
installed plugins become usable without changing the command schema:

```json
{"op":"insert_plugin_path","track_id":2,"path":"/Library/Audio/Plug-Ins/CLAP/Vital.clap"}
```

`open_utau_import` validates and records the source/render pair and places the
rendered vocal at beat zero on the requested track. Follow-up region commands
can move, trim, warp, or replace it through the same transaction boundary.

The read-only `openutau_singer_catalog` operation lists locally installed
Singer voicebanks for an audio-client selector:

```json
{"op":"openutau_singer_catalog"}
```

The response is bounded, sorted, and excludes symbolic-link directories.

Use the read-only `openutau_vocals` operation to inspect registered source and
render pairs, selected Singer metadata, tuning, and content hashes:

```json
{"op":"openutau_vocals"}
```

Singer-specific pronunciation controls are exposed through the stable client
API as normalized values (`scoop`, `vibrato`, `dynamics`, and `consonants`),
each constrained to `0..=1`; invalid values or unregistered source/render
pairs are rejected before mutation.

Use `set_open_utau_singer` with project-write permission to change the selected
Singer for a registered source/render pair:

```json
{"op":"set_open_utau_singer","source_path":"./voice.ustx","rendered_audio_path":"./voice.wav","singer":"KasaneTeto"}
```

Use `set_open_utau_tuning` with project-write permission to update the
normalized pronunciation controls without re-rendering the source:

```json
{"op":"set_open_utau_tuning","source_path":"./voice.ustx","rendered_audio_path":"./voice.wav","scoop":0.25,"vibrato":0.75,"dynamics":0.5,"consonants":0.4}
```

`render_vocal_notes_wav` renders an explicit canonical MIDI/OpenUtau note JSON
array, while `render_scheduled_vocal_notes_wav` renders the notes currently
stored in the project. Both are isolated external-side-effect actions and
require an explicit system-write permission:

```json
{"op":"render_scheduled_vocal_notes_wav","sample_rate":48000,"max_samples":96000,"output_path":"./preview.wav"}
```

The scheduled form is useful for UI and automation clients because it cannot
silently diverge from the project's current piano-roll state.

`vocal_alignment` provides a bounded, read-only timing analysis for lead and
double-vocal envelopes. It returns alignment factors for downstream warping:

```json
{"op":"vocal_alignment","lead_envelope":[0.0,0.5,1.0],"dub_envelope":[0.0,0.4,0.9],"tightness":0.75}
```

Use `vocal_lyric_preview` for a direct lyric-level audition before phoneme or
pitch-curve editing:

```json
{"op":"vocal_lyric_preview","lyric":"あ","frequency":220.0,"sample_rate":48000.0,"length":4096}
```

`openutau_status` reports whether the OpenUtau application and helper assets
are available to the current client:

```json
{"op":"openutau_status"}
```

`drum_replacer_triggers` converts a mono drum recording into sample-accurate
MIDI trigger events for layering with a sampler. The response includes both
structured events and a deterministic little-endian `midi_stream_hex` payload:

```json
{"op":"drum_replacer_triggers","samples":[0.0,0.0,0.8,0.2],"sample_rate":48000.0,"target_note":36,"sensitivity":0.4,"retrigger_samples":512}
```

To publish the rendered voice directly into the arrangement, use
`render_scheduled_vocal_notes_to_region` with an existing audio track and a
start position:

```json
{"op":"render_scheduled_vocal_notes_to_region","track_id":1,"start":0.0,"sample_rate":48000,"max_samples":96000,"output_path":"./voice.wav"}
```

This writes the WAV and registers it as an audio region in one audited
operation, making the generated voice immediately available to the mixer and
waveform editor.

For editing an existing note's pronunciation or pitch expression without
replacing its MIDI timing, use the reversible `set_midi_note_articulation`
action:

```json
{"op":"set_midi_note_articulation","track_id":1,"pitch":60,"start_sample":0,"phoneme":"a","pitch_curve_cents":[0,35,-10],"vibrato_depth_cents":24,"portamento_samples":96}
```

Canonical project notes also persist `vibrato_rate_millihz` (500..20000 mHz,
default 5000). OpenUtau imports retain this speed and the vocal preview/render
fallback uses it whenever no explicit pitch curve is present.

Legacy UST `VBR=length,period,depth,...` records are normalized the same way:
period is converted from milliseconds to mHz and depth to cents before the
note enters the canonical contract.

Scheduled vocal notes expose the same live edit through
`set_midi_note_vibrato_rate(track_id, pitch, start_sample, vibrato_rate_millihz)`.

Pitch-segment clients can update the live project state with
`set_pitch_segment_vibrato_rate(start_sample, end_sample, vibrato_rate_millihz)`
after validating a focused edit through
`edit_pitch_segment_vibrato_rate_json`.
The combined `edit_pitch_segment_vibrato_json` helper updates depth and rate
atomically for serialized editor state.

Vocal preview and WAV-render responses also include `rendered_note_count`,
`skipped_rest_count`, `skipped_invalid_count`, and
`skipped_probability_count`. These counters let a client explain why a score
produced fewer audible notes without inspecting the source document again.

`undo` and `redo` are first-class command actions, so GUI, CLI, and automation
clients share the native history rather than maintaining separate local undo
stacks.

The direct CLAP host preserves bounded MIDI output from a plugin back into the
current block. Core MIDI and bounded SysEx events are accepted only when their
sample offset is inside the block and their payload fits the fixed MIDI
capacity; unsupported or oversized events are rejected and counted rather than
silently truncated. The isolated CLAP worker remains the preferred external
plugin path for crash containment.

The SDK-enabled VST3 worker maps bounded Note On/Off, MIDI CC, pitch-bend, and
SysEx events from the VST3 event list into the same mailbox. It also translates
the corresponding host input events into VST3 `Event` values. This keeps MIDI
Learn, external MIDI processors, and bounded SysEx behavior consistent across
the CLAP and VST3 worker paths; unsupported, malformed, or oversized events
are ignored and counted rather than presented as valid MIDI.

Automation is a first-class reversible command. `set_automation` accepts
absolute-sample `[time_samples, value, curve, ...]` triples, requires
strictly increasing integer sample positions, and uses
the same generation and transaction checks as track and plugin edits:

```json
{"op":"set_automation","track_id":1,"parameter_id":0,"points":[0,0.2,0.0,22050,0.8,0.2,44100,0.4,0.0]}
```

MIDI notes and non-destructive region warp are also reversible commands:

```json
{"op":"set_midi_note","track_id":1,"pitch":60,"velocity":100,"start_sample":0,"length_samples":24000}
{"op":"set_region_warp","track_id":1,"region_id":1,"ratio":1.02}
{"op":"set_region_gain","track_id":1,"region_id":1,"gain_db":-1.5}
{"op":"set_region_pitch","track_id":1,"region_id":1,"semitones":0.0}
```

## Project history CLI

The CLI can create, inspect, and hydrate-check the canonical JSON project
document without opening the UI:

```sh
aura project init ./Song.aura "Song" 44100
aura project inspect ./Song.aura
aura project load ./Song.aura
aura project manifest ./Song.aura
```

`project load` performs a fresh process-level load and validates the canonical
project document without rewriting it. It returns structured track/region
counts; the JSONL transaction API remains the path for generation-aware engine
mutations.

`project init` accepts an optional sample rate of `44100`, `48000`, `88200`,
`96000`, or `192000`; the default is `44100`, matching the native engine's
default configuration. This avoids creating a project that cannot hydrate on
the default CoreAudio configuration before a device rate has been selected.

To verify that the working file is the exact snapshot referenced by history
HEAD before an automated render or restore, use:

```sh
aura history verify ./Song.aura
```

The structured result reports the HEAD hash, working-tree hash, and project
UUID match independently. A mismatch exits non-zero.

The standalone CLI also exposes the DAW history store:

```sh
aura history status ./Song.aura
aura history log ./Song.aura 20
aura history commit ./Song.aura -m "vocal high shelf +2dB"
aura history branch create ./Song.aura vocal-experiment
aura history checkout ./Song.aura vocal-experiment
aura history checkout ./Song.aura vocal-experiment --restore --approve
aura history tag ./Song.aura mix-v1
aura history diff ./Song.aura <commit-a> <commit-b>
aura history revert ./Song.aura <commit-id> --approve
aura history cherry-pick ./Song.aura <commit-id> --sections plugin_instances,warp_markers --approve

Track timing offsets are exposed as a reversible command and are expressed in
samples to remain deterministic across sample-rate changes:

```json
{"type":"set_track_delay","track_id":7,"samples":2400}
```

Track delay can also be automated with absolute sample positions. The value
is normalized from 0 to 1 and maps to the engine's bounded delay range:

```json
{"type":"set_track_delay_automation","track_id":7,"points":[0,0,0,22050,0.5,0,44100,0,0]}
```

This operation is reversible and is serialized with the project. Sample
positions are strictly increasing integers; using samples rather than a
normalized timeline prevents automation drift when the sample rate changes.
Passing an empty `points` array clears the selected automation lane as one
undoable operation. The same clear semantics apply to volume and pan
automation. 

Track Stack topology is also a reversible control-plane mutation. The stack
definition is persisted with the project and can be changed independently of
native audio processing:

```json
{"op":"set_master_gain","value":0.85}
```

The master gain is persisted in the canonical project document and applied
before the final limiter, so offline renders and live monitoring use the same
master level.

```json
{"op":"create_track_stack","stack_id":1,"name":"Vocal Stack","member_track_ids":[7,8,9],"master_gain":1.0,"collapsed":false}
{"op":"set_track_stack_gain","stack_id":1,"master_gain":0.8}
{"op":"set_track_stack_collapsed","stack_id":1,"collapsed":true}
{"op":"delete_track_stack","stack_id":1}
```

Stack members must be unique, non-zero IDs; invalid definitions are rejected
before the Core state changes.

History uses `.aura/history/` beside the project file. `revert` validates the
content-addressed snapshot and publishes it through the normal atomic project
save path; it never copies audio assets into every commit.
Plugin aliases resolve deterministically: an explicit catalog id selects that
exact format, while a display-name lookup prefers CLAP, then VST3, then AU.
To select a format by name, use `Name@clap`, `Name@vst3`, or `Name@au`.
`history diff` returns section hashes plus bounded entity and field changes. Large
plugin-state blobs are represented by hashes rather than copied into the response,
so automation clients can preview edits without materializing multi-megabyte state.

Trusted project-local extensions may expose an optional relative `entrypoint`
in their manifest. Invoke one with `extension_invoke` only when the manifest
declares `execution: "trusted"` and the explicit `process_spawn` permission.
Aura passes a bounded JSON request on stdin and accepts at most a 1 MiB JSON
object on stdout; the default timeout is 5 seconds and the maximum is 30
seconds. Absolute paths, traversal, symlink entrypoints, sandboxed execution,
and undeclared commands are rejected. Discovery and validation remain safe and
never execute extension code.
For a directory-style project, pass the directory itself (it may be created
on first use). A not-yet-created path with an extension is rejected as
ambiguous; create the directory first or use an existing project file.

OpenUtau imports are persisted in the canonical project document as a source
`.ust`/`.ustx` path paired with its rendered WAV/AIFF path. The pair is only
registered after both files pass validation. WAV imports must contain a
non-empty, structurally valid RIFF/WAVE or RF64 payload; AIFF imports must
contain a valid FORM/AIFF header. The import audit also records SHA-256
identities, source note/singer metadata, and WAV byte/frame/channel metadata,
so an external re-render, voicebank change, or asset replacement is visible to
history and restore diagnostics.
`project inspect` reports the number of persisted vocal pairs so a CLI or AI
client can detect whether a reload retained the relationship. Each pair also
reports `current_files` as `match`, `changed`, or `missing` by re-auditing the
external files on the control thread; the audio callback never performs this
filesystem work.

The desktop handoff uses `/Applications/OpenUtau.app` by default. Portable
installs, forks, and CI fixtures may set `AURA_OPENUTAU_APP` to an alternate
application bundle; the status API and UI launch action resolve the same path.

Waveform mastering measurements are available as read-only commands. Loudness
returns integrated LUFS, short-term LUFS, and loudness range; true-peak uses the
engine's 4x inter-sample estimator:

```json
{"op":"analyze_waveform_loudness","left":[0.1,0.0],"right":[0.1,0.0],"sample_rate":48000}
{"op":"analyze_waveform_true_peak","left":[0.1,0.0],"right":[0.1,0.0]}
{"op":"analyze_waveform_transients","left":[0.0,0.0,0.8,0.8],"right":[0.0,0.0,0.8,0.8],"window":2,"sensitivity":0.75}
{"op":"normalize_waveform_loudness","samples":[0.1,0.1,0.1,0.1],"sample_rate":48000,"channels":2,"target_lufs":-14.0,"max_true_peak_dbtp":-1.0}
{"op":"normalize_region_loudness","track_id":1,"region_id":1,"target_lufs":-14.0,"max_true_peak_dbtp":-1.0}
```

Both channels must have equal, finite lengths. Loudness accepts 8–384 kHz and
up to two million samples; true-peak has the same sample bound. The active
realtime device can be selected with a system-write command:

```json
{"op":"select_audio_device","device_id":1,"sample_rate":48000,"buffer_size":256}
```

Transient indices can be committed as AudioWarp hitpoints with a reversible
project-write command. Source and timeline arrays must be strictly increasing
and have matching lengths; the native renderer applies the resulting stretch
ratio atomically to the addressed region:

```json
{"op":"replace_region_warp_markers","region_id":1,"source_samples":[0,24000,48000],"timeline_samples":[0,26400,52800]}
{"op":"clear_region_warp_markers","region_id":1}
```

Supported sample rates are 44.1, 48, 88.2, 96, and 192 kHz; buffer sizes are
powers of two from 32 through 2048. Native device
availability is checked by the audio backend and rejected without changing the
project when the device is unavailable.
Use the read-only `inspect_audio_device` operation to obtain the current ready
state, active format, callback count, dropped input blocks, fallback status,
and enumerated device catalog in one response.
Successful device selection also returns the resulting `audio_generation` so
clients can immediately refresh their generation token.
`apply_audio_config` uses the same supported format set to reconfigure the
backend atomically and returns the new audio generation, or a structured
driver diagnostic when reconfiguration is rejected.
