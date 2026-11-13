# TEXTUAL PATCH — Hirari project file specification (v1)

**Status**: conformance suite lands with this document (see
`hirari-core-bridge/src/project/project_contracts_tests.rs`). Not yet
published externally; publication waits on a second correct reader agreeing
on every conformance vector.
**Schema**: `PROJECT_SCHEMA_VERSION = 1`, `PROJECT_CONTRACT_VERSION = 2`.

## 1. Purpose

This document defines the on-disk format Hirari writes for a project file
(`.hirari`) and for the control-plane portion of a project document. It is
the format referenced by every persistence surface: save/load, autosave,
undo history export, project archive, and the derived DAWproject exchange
(`docs/architecture/DAWPROJECT.md`-equivalent, see §9). The format is a
stable text document: JSON, pretty-printed, written atomically.

The goal is specification-grade engineering inside a private format: written
spec, versioned, conformance-tested, deterministic. A public standard is not
claimed.

## 2. Notation

A project document is a single JSON object. JSON is limited to the
following profile:

- **Encoding**: UTF-8, no BOM, LF or CRLF line endings permitted on read;
  writers emit LF.
- **Integers**: where the Rust model carries a `u64`/`u32`, the JSON value
  is a JSON number. All writers use `serde_json::to_vec_pretty`, which prints
  `48000.0` for the f64 literal `48000_f64`. Readers MUST accept
  `48000.0`, `48000`, and `4.8e4` — they denote the same IEEE-754 double.
- **Floats**: IEEE-754 doubles. `NaN`, `+Inf`, `-Inf` are forbidden in every
  finite-domain field (validated at load: see contracts for the specific
  range of each field). Serialized as e.g. `0.5`, `1.0`, `2.5e-3`.
- **Strings**: Unicode scalar sequences. `\uXXXX` escapes are legal on
  read; writers emit unescaped UTF-8. NUL (`U+0000`) is forbidden in every
  string field and rejected at load.
- **Arrays/objects**: standard JSON. Object key order is **not** semantic;
  readers MUST NOT depend on it. Writers emit keys in declaration order of
  the Rust struct for byte-stable output. Key order of unspecified structs
  is unspecified and may change between minor versions without notice.
- **Optional fields**: fields with `#[serde(default)]` may be omitted.
  Consumers MUST accept their absence and treat it as the documented
  default.
- **`Option<T>` fields** may be absent or explicitly `null`. Both are legal
  on read. Writers emit `null` unless `skip_serializing_if` applies.
- **Unknown keys**: at the document top level and in every individual
  contract struct, unknown keys are **ignored** (minor-release tolerance:
  a new file read by an older minor reader drops only the unknown keys).

### Superseded positional fields removed

None. The current major schema keeps every field that any released reader
consumes.

## 3. Canonical ordering

- Object order is unspecified semantically (see §2). For byte-comparison
  workflows (`diff_project_json`) the reference implementation emits keys in
  Rust struct declaration order.
- Array order is significant everywhere. Automation lanes are ordered by
  `(time, then left-to-right evaluation order)`; plugin lanes by
  parameter index ascending; regions by start then id; tempo and
  time-signature events by beat ascending.

## 4. Determinism guarantees

- **Save** is atomic: write to `<path>.tmp-<pid>-<seq>` in the same
  directory, `fsync` the temp file, rename onto the destination, `fsync`
  the parent directory. Readers never observe a torn file.
- **Emit** is byte-stable: same value → same bytes on every platform
  (floats use the shortest round-trippable form via `serde_json`).
- **diff_project_json** (`hirari-core-bridge/src/project/project.rs`) emits
  JSON-pointer-like paths for every changed leaf, suitable for history and
  autosave reporting. Capitalization and escaping follow RFC 6901
  (`~`→`~0`, `/`→`~1`).
- **Hash comparisons**: `PluginInstanceContract::binary_hash` is the
  SHA-256 fingerprint of the plug-in binary captured at admission;
  unchanged across save/load.

## 5. Versioning policy

Two numbers gate the format. Both are on the document root.

| Field | Constant | Current | Reader rule |
| --- | --- | --- | --- |
| `schema_version` | `PROJECT_SCHEMA_VERSION` | `1` | must equal exactly `1`; any other value rejects the document at load |
| `contract_version` | `PROJECT_CONTRACT_VERSION` | `2` | `1` is auto-upgraded to `2` at load; missing gets the default `2`; any other value rejects |
| `metadata.version` | — | `1` | must equal `PROJECT_SCHEMA_VERSION`; otherwise rejects |

**Minor additions** (new optional keys on existing structs) are permitted
without a version bump. Older readers ignore them and re-emit the document
without the unknown keys, which is allowed (see §2).

**Major bumps** (renaming/retyping keys, changing semantics of existing
fields) require incrementing `PROJECT_SCHEMA_VERSION`. Older readers must
reject the document outright; upgrading is explicit, never silent.

**Contract versions** track the semantic shape of the contracts — the new
curve shape (`Curve::Shaped`) and the builtin dataflow device are contract
version 2. Contract version 1 documents are upgraded in memory at load and
validated under the new rules; unknown contract versions reject.

## 6. Field tables

### 6.1 Root (`ProjectDocument`)

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schema_version` | u32 | yes | must be `1` |
| `contract_version` | u32 | no, default `2` | see §5 |
| `project_id` | UUID string | yes, generated | stable across path moves |
| `metadata` | `ProjectMetadata` | yes | see 6.2 |
| `sample_rate` | f64 | yes | integral Hz, `1..=384000` inclusive, validated |
| `master_gain` | f32 | no, default `1.0` | `0.0..=2.0` |
| `cycle_start_sample`, `cycle_end_sample` | u64 | no, default `0` | |
| `cycle_enabled`, `metronome_enabled` | bool | no, default `false` | |
| `tracks` | `ProjectTrack[]` | yes (may be empty) | see 6.3 |
| `aux_track_ids` | u32[] | no, default `[]` | ids of bus tracks |
| `regions` | `ProjectRegion[]` | no, default `[]` | |
| `plugin_instances` | `PluginInstanceContract[]` | no, default `[]` | see 6.4 |
| `midi_learn_mappings`, `macro_mappings` | contracts | no, `[]` | |
| `midi_notes` | `MidiNoteContract[]` | no, `[]` | musical model |
| `chord_track` | `ChordEvent[]` | no, `[]` | |
| `midi_events` | `MIDIEvent[]` | no, `[]` | control-plane MIDI incl. SysEx |
| `tempo_events` | `TempoEventContract[]` | no, `[]` | |
| `time_signature_events` | `TimeSignatureEventContract[]` | no, `[]` | |
| `warp_markers` | `WarpMarkerContract[]` | no, `[]` | |
| `render_targets` | `RenderTargetContract[]` | no, `[]` | |
| `freeze_artifacts` | `FreezeArtifactContract[]` | no, `[]` | |
| `sidechain_routes`, `feedback_routes`, `audio_routes` | contracts | no, `[]` | routing |
| `audio_input_assignments` | `TrackAudioInputAssignment[]` | no, `[]` | device uid untouched by save/load |
| `step_sequencer_patterns`, `openutau_vocals`, `comp_takes`, `comp_segments`, `track_stacks`, `markers`, `vca_groups`, `hardware_inserts`, `control_room` | contracts/structs | no, defaults | see source for per-field tables |

### 6.2 `metadata`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | non-empty after trim |
| `version` | u32 | yes | must equal `PROJECT_SCHEMA_VERSION` |
| `bpm` | f32 | yes | finite, `20.0..=300.0` |
| `tracks_count` | u32 | yes | |
| `key_root` | i32 | no, default `0` | -1 unspecified |
| `scale_type` | u32 | no, default `0` | |

### 6.3 `tracks[]` (`ProjectTrack`)

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | u32 | yes | unique, nonzero |
| `name` | string | yes | |
| `track_type` | string | yes | one of `Audio`, `Midi`, `Instrument`, `Bus`, `Aux`, `Vocal`; unknown values reject at hydration |
| `volume`, `pan` | f32 | yes | gain `0.0..=2.0`, pan `-1.0..=1.0` |
| `muted`, `solo`, `record_armed`, `phase_invert` | bool | yes/no default `false` | |
| `track_delay_samples` | u32 | no, default `0` | |
| `volume_automation`, `pan_automation`, `track_delay_automation` | `AutomationPointContract[]` | no, `[]` | see 6.5 |
| `plugin_automation` | `PluginAutomationLane[]` | no, `[]` | per-plugin parameter lanes |
| `plugin_types`, `plugin_bypasses`, `plugin_parameter_values`, `plugin_states`, `plugin_gui_states`, `plugin_state_versions` | heterogeneous | no, `[]` | insert-slot state |
| `sandbox_plugin_paths`, `sandbox_plugin_states`, `sandbox_plugin_state_versions` | heterogeneous | no, `[]` | sandboxed inserts |
| `expression_map`, `expression_map_pro` | contracts | no, `[]/null` | |
| `step_sequencer_patterns`, `openutau_vocals` | contracts | no, `[]` | |

### 6.4 `plugin_instances[]` (`PluginInstanceContract`)

External (third-party) plug-ins have no `kind` key; every byte of their
state stays in `state_blob` (opaque, checksummed by `binary_hash` at
admission). Built-in devices carry `kind`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `instance_id` | string | yes | unique, non-empty |
| `format` | `PluginFormat` | yes | `Clap`, `Vst3`, `AudioUnit`, `BuiltIn` |
| `bundle_path`, `plugin_id` | string | yes | `plugin_id` non-empty; `bundle_path` may be synthetic for built-ins |
| `state_blob`, `gui_state` | bytes (base64 in JSON? — see below) | no, `[]` | opaque; `state_blob` ≤ 4 MiB, `gui_state` ≤ 1 MiB |
| `binary_hash` | string | no, `""` | SHA-256 admission fingerprint; ≤ 64 hex chars or empty |
| `capability`, `plugin_version`, `architecture` | strings | no, `""` | capability verdict at admission |
| `kind` | `BuiltinDataflow` | no | built-in devices only |
| `parameter_ids`, `parameter_values` | arrays | no, `[]` | parallel arrays, values finite `0.0..=1.0` |
| `bypassed`, `offline`, `quarantined`, `state_schema_version`, `state_generation`, `latency_samples`, `plugin_version`, `architecture` | scalars | no, defaults | `state_schema_version ≤ 1` |

`BuiltinDataflow` (tagged branch, strict by design — unknown keys are
**rejected**, the AI edit boundary):

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `units` | `BuiltinUnit[]` | yes | ≤ 64 |
| `units[].id` | string | yes | unique per device, ≤ 256 bytes |
| `units[].kind` | string | yes | ≤ 256 bytes |
| `units[].parameters` | object<string,f64> | no, `{}` | ≤ 256 entries, values finite |
| `links` | `BuiltinLink[]` | no, `[]` | ≤ 512 |
| `links[].from`, `links[].to` | `"unitid:port"` | yes | both endpoints must reference declared units; port names non-empty |

### 6.5 `AutomationPointContract` / `Curve`

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `time` | f64 | yes | integral, finite, ≥ 0 |
| `value` | f32 | yes | finite |
| `curve` | `Curve` | no, default shortcut `0.0` | see below |

`curve` serializes in one of two shapes — a bare number (legacy shortcut)
or an object (authored shapes). Both are valid; a document may mix them
per point. There is never more than one shape per point.

| Wire shape | Semantics |
| --- | --- |
| `0.5` | `Curve::Shortcut(0.5)` — sigmoid weight in `-1.0..=1.0`; linear at `0.0` |
| `{"kind":"linear"}` | identity ease |
| `{"kind":"step"}` | holds left value; jumps at the segment end |
| `{"kind":"bezier","p":[{"x":0.25,"y":0.1},{"x":0.75,"y":0.9}]}` | cubic Bézier through the two control points (x in `[0,1]`, non-decreasing, y in `[0,1]`) |

Old projects with `"curve": 0.5` load unchanged; byte-identical round-trip.
New shapes load with their tagged form.

## 7. Conformance vectors

The conformance suite (contract test suite in
`hirari-core-bridge/src/project/project_contracts_tests.rs`) executes these
vectors. Any reader is presumed conformant only after agreeing on all of
them.

1. `{"time":48000.0,"value":0.5,"curve":0.5}` round-trips byte-identically
   as `Curve::Shortcut(0.5)`.
2. `{"time":96000.0,"value":1.0}` loads to `Curve::Shortcut(0.0)`.
3. `{"time":1.0,"value":0.25,"curve":{"kind":"linear"}}` round-trips as
   `Curve::Shaped(Shape::Linear)`.
4. `{"kind":"bezier","p":[{"x":0.25,"y":0.1},{"x":0.75,"y":0.9}]}` parses
   and fails validation when control-point `x` inverts or leaves `[0,1]`.
5. A document whose root `contract_version` is `1` upgrades in memory and
   validates as version `2`; `contract_version` `3` rejects.
6. Unknown keys at the root and in ordinary contract structs are ignored;
   unknown keys **inside** a builtin device branch reject at parse.
7. A built-in device survives save→load byte-identically (units, links,
   `state_blob`, `binary_hash`).
8. The canonical emit of a document value is reproduced exactly by
   re-parsing the emitted bytes (parse ∘ emit = identity).
9. `diff_project_json` on two documents with one leaf difference reports
   exactly one JSON-pointer-like path.
10. **(Editor boundary)** hand-edited documents with a `units[]` entry
    whose `kind` is absent from the document's own unit list parse; links
    referencing undeclared units reject at validation.

## 8. Reference implementation

`hirari-core-bridge/src/project/project.rs` (document model),
`hirari-core-bridge/src/project/project_contracts.rs` (contract types and
validation), `hirari-core-bridge/src/project/project_contracts_tests.rs`
(conformance). The reader and writer share one serde model; there is no
separate "parser".

## 9. DAWproject mapping

Export writes every track as a DAWproject `<Track>` with built-in Devices
mapped to the plugin slot; import restores built-in devices within
`type <= 10` and counts anything else as unsupported. Automation exports
as `interpolation="linear"` — DAWproject has no sigmoid or Bézier curve
shapes, so all non-linear easing is written as piecewise-linear points
(values are normalized). Plugin parameter curves export likewise as
`<RealPoint>` normalized samples.

| Hirari | DAWproject |
| --- | --- |
| `ProjectTrack.id` | `<Track id="t-{id}">` |
| `ProjectTrack.track_type` Audio/Midi/Instrument | audio/midi device tracks |
| `volume`/`pan` | `<Pan>`, `<Volume>` |
| `AutomationPointContract[]` (volume/pan/delay) | `<Points interpolation="linear">` |
| `plugin_instances[]` | `<Devices>` custom device entries |
| `kind` (builtin device) | custom device entries; restores as built-in |
| warp markers | `<WarpMarker>` |
| tempo events | `<Tempo>` |
| time signature events | `<TimeSignature>` |
| markers | `<Marker>` |
| MIDI notes | `<Notes>` |

## 10. AI editing boundary

The AI editor may edit **only** the `kind` branch of a built-in device.
Everything else in the file is treated as read-only to automated tooling.

**Allowed diff**: add/remove `units`, reorder `links`, edit `parameters`
(all keys inside `kind`).

```diff
-"units":[{"id":"gain-1","kind":"builtin/gain","parameters":{"level":0.5}}]
+"units":[{"id":"gain-1","kind":"builtin/gain","parameters":{"level":0.25}},
+  {"id":"limiter-1","kind":"builtin/limiter","parameters":{"ceiling":-1.0}}]
```

**Blocked diffs** (rejected at load or verification):

```diff
-"state_blob":"AAEC"
+"state_blob":"AAEF"                 # opaque third-party state — untouched
-"binary_hash":"abc...64"
+"binary_hash":"def...64"            # admission fingerprint stays as-is
-"schema_version":1
+"schema_version":2                  # rejects at load
-"units":[{"id":"gain-1","kind":"builtin/gain","bogus":1}]
+"units":[{"id":"gain-1","kind":"builtin/gain"}]  # unknown key rejects at parse
-"links":[{"from":"ghost:out","to":"u:in"}]
+"links":[{"from":"gain-1:out","to":"u:in"}]    # ghost unit rejects at validation
```
