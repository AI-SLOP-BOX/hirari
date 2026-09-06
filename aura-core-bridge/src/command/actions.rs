#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CommandAction {
    ControlInspect,
    /// Return the canonical piano-roll/vocal note list, including lyrics.
    InspectMidiNotes,
    /// Return the canonical persisted chord-track event list.
    InspectChordTrack,
    /// Add one chord event to the canonical chord track.
    AddChordEvent {
        tick: u64,
        root: u8,
        intervals: Vec<u8>,
        name: String,
    },
    /// Place a generated voicing into a MIDI track.
    PlaceGeneratedChord {
        track_id: u32,
        start_sample: u64,
        length_samples: u64,
        velocity: u8,
        root: i32,
        octave: i32,
        quality: u32,
    },
    RemoveChordEventsRange {
        start_tick: u64,
        end_tick: u64,
    },
    ClearChordTrack,
    /// Expand a code-pad chord into a deterministic MIDI voicing.
    GenerateChord {
        root: i32,
        octave: i32,
        quality: u32,
    },
    SuggestNextChords {
        last_chord_name: String,
    },
    GenerateArpeggio {
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    },
    PlaceArpeggio {
        track_id: u32,
        start_sample: u64,
        step_samples: u64,
        gate_samples: u64,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    },
    /// Resolve a General MIDI percussion pitch to a stable drum-lane label.
    DescribeDrumLane {
        pitch: u8,
    },
    /// Analyze a bounded audio sample window and return deterministic
    /// compressor/gate suggestions without mutating the project.
    AnalyzeDynamics {
        samples: Vec<f32>,
        /// Optional owner track for UI/API diagnostics.  The analysis remains
        /// read-only and can still be used as a generic sample-window check.
        #[serde(default)]
        track_id: Option<u32>,
    },
    /// Analyze bounded stereo mix metrics without mutating project state.
    AnalyzeMix {
        left: Vec<f32>,
        right: Vec<f32>,
        #[serde(default)]
        reference_left: Vec<f32>,
        #[serde(default)]
        reference_right: Vec<f32>,
        #[serde(default)]
        ab_left: Vec<f32>,
        #[serde(default)]
        ab_right: Vec<f32>,
    },
    /// Find non-destructive silence ranges for event splitting.
    AnalyzeSilence {
        samples: Vec<f32>,
        #[serde(default = "default_silence_threshold")]
        threshold: f32,
        #[serde(default = "default_silence_min_length")]
        min_length: u32,
    },
    /// Split one audio region at the boundaries of detected silent runs.
    SplitRegionAtSilence {
        track_id: u32,
        region_id: u32,
        samples: Vec<f32>,
        #[serde(default = "default_silence_threshold")]
        threshold: f32,
        #[serde(default = "default_silence_min_length")]
        min_length: u32,
    },
    /// Produce a bounded, non-destructive vocal pitch-correction preview.
    PreviewVocalPitchCorrection {
        samples: Vec<f32>,
        #[serde(default = "default_preview_sample_rate")]
        sample_rate: f64,
        #[serde(default = "default_preview_speed")]
        speed: f32,
        #[serde(default = "default_preview_timing_ratio")]
        timing_ratio: f32,
    },
    /// Analyze a sample window and apply the four core compressor settings
    /// to an internal Aura/Compressor in one undo transaction.
    ApplyDynamicsSuggestion {
        track_id: u32,
        plugin_index: u32,
        samples: Vec<f32>,
    },
    /// Search the canonical project snapshot without mutating it.
    ProjectSearch {
        query: String,
    },
    ExtensionCatalog {
        root: String,
    },
    ExtensionValidate {
        root: String,
        extension_id: String,
        command_id: String,
        payload: serde_json::Value,
    },
    ExtensionInvoke {
        root: String,
        extension_id: String,
        command_id: String,
        payload: serde_json::Value,
        #[serde(default = "default_extension_timeout_ms")]
        timeout_ms: u64,
    },
    ExtensionSetEnabled {
        root: String,
        extension_id: String,
        enabled: bool,
    },
    AddTrack {
        name: String,
        #[serde(default)]
        track_type: u32,
    },
    AddAuxTrack {
        name: String,
    },
    RemoveTrack {
        track_id: u32,
    },
    DuplicateTrack {
        track_id: u32,
    },
    AddVcaGroup {
        group_id: u32,
        #[serde(default = "default_one")]
        gain: f32,
    },
    AssignTrackToVca {
        track_id: u32,
        group_id: u32,
    },
    SetVcaGroupGain {
        group_id: u32,
        gain: f32,
    },
    SetPluginFavorite {
        id: String,
        favorite: bool,
    },
    PluginSearch {
        #[serde(default)]
        query: String,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        favorites_only: bool,
    },
    AddPlugin {
        track_id: u32,
        plugin_type: u32,
    },
    FreezeTrack {
        track_id: u32,
        total_samples: u64,
        /// Optional project-local cache path. Without it the freeze remains
        /// an in-memory runtime snapshot.
        #[serde(default)]
        path: Option<String>,
    },
    /// Freeze using the current project end, avoiding callers having to
    /// guess the render length from a stale UI snapshot.
    FreezeTrackToProjectEnd {
        track_id: u32,
    },
    UnfreezeTrack {
        track_id: u32,
    },
    TrackFreezeStatus {
        track_id: u32,
    },
    RemovePlugin {
        track_id: u32,
        plugin_index: u32,
    },
    MovePlugin {
        track_id: u32,
        from_index: u32,
        to_index: u32,
    },
    SetPluginParameter {
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
        value: f32,
    },
    SetPluginBypass {
        track_id: u32,
        plugin_index: u32,
        bypassed: bool,
    },
    SetMacroValue {
        macro_index: u32,
        value: f32,
    },
    AddMacroMapping {
        mapping_id: String,
        macro_index: u32,
        target_instance_id: String,
        target_parameter_id: String,
        #[serde(default)]
        min: f32,
        #[serde(default = "default_one")]
        max: f32,
        #[serde(default)]
        curve: f32,
        #[serde(default)]
        invert: bool,
    },
    RemoveMacroMapping {
        mapping_id: String,
    },
    AddMidiLearnMapping {
        mapping_id: String,
        device_id: String,
        channel: u32,
        controller: u32,
        target_instance_id: String,
        target_parameter_id: String,
        #[serde(default)]
        min: f32,
        #[serde(default = "default_one")]
        max: f32,
        #[serde(default)]
        curve: f32,
        #[serde(default)]
        pickup: bool,
    },
    RemoveMidiLearnMapping {
        mapping_id: String,
    },
    HumanizeMidi {
        timing_beats: f32,
        velocity: i32,
        seed: u64,
    },
    QuantizeMidi {
        grid_beats: f32,
        strength: f32,
    },
    ApplyMidiSwing {
        subdivision_beats: f32,
        amount: f32,
    },
    /// Apply a validated, data-driven Logical Editor rule to all canonical MIDI notes.
    ApplyMidiLogicalRule {
        rule: crate::midi_logical_editor::MidiLogicalRule,
    },
    TakeMixSnapshot {
        name: String,
        #[serde(default)]
        states: std::collections::HashMap<u32, f32>,
    },
    CaptureMixSnapshot {
        name: String,
    },
    DiffMixSnapshots {
        first: usize,
        second: usize,
    },
    RecallMixSnapshot {
        index: usize,
    },
    ApplyMixSnapshot {
        index: usize,
    },
    InsertNamedPlugin {
        track_id: u32,
        alias: String,
    },
    /// Insert a concrete installed plugin bundle.  This is the extensibility
    /// path for plugins that are not yet known by the catalog alias list.
    InsertPluginPath {
        track_id: u32,
        path: String,
    },
    #[serde(alias = "openutau_import")]
    OpenUtauImport {
        track_id: u32,
        source_path: String,
        rendered_audio_path: String,
    },
    /// Read-only structured note inspection for the in-DAW vocal editor.
    OpenUtauNotes {
        source_path: String,
    },
    /// Import the structured UST/USTX note stream into the canonical project
    /// MIDI model. The rendered vocal remains a separate audio-region import.
    OpenUtauImportMidi {
        track_id: u32,
        source_path: String,
        sample_rate: u32,
        ticks_per_beat: u32,
    },
    AddAudioRegion {
        track_id: u32,
        path: String,
        start: f64,
    },
    ReplaceRegionAudio {
        track_id: u32,
        region_id: u32,
        path: String,
    },
    PluginCatalog,
    SetVolume {
        track_id: u32,
        value: f32,
    },
    SetEq {
        track_id: u32,
        low_band: f32,
        low_cut: f32,
        high_band: f32,
        high_cut: f32,
    },
    /// Apply a bounded, relative gain-staging correction to a track fader.
    ApplyGainStaging {
        track_id: u32,
        gain_db: f32,
    },
    SetMasterGain {
        value: f32,
    },
    SetTrackDelay {
        track_id: u32,
        samples: u32,
    },
    SetLowLatencyMode {
        enabled: bool,
    },
    SetTonalScale {
        root: i32,
        scale_type: u32,
    },
    CreateTrackStack {
        stack_id: u32,
        name: String,
        member_track_ids: Vec<u32>,
        #[serde(default = "default_one")]
        master_gain: f32,
        #[serde(default)]
        collapsed: bool,
    },
    DeleteTrackStack {
        stack_id: u32,
    },
    UpsertMarker {
        marker_id: u32,
        label: String,
        beat: f64,
        #[serde(default)]
        color: String,
    },
    DeleteMarker {
        marker_id: u32,
    },
    SetTrackStackGain {
        stack_id: u32,
        master_gain: f32,
    },
    SetTrackStackCollapsed {
        stack_id: u32,
        collapsed: bool,
    },
    SetPan {
        track_id: u32,
        value: f32,
    },
    SetMute {
        track_id: u32,
        muted: bool,
    },
    SetSolo {
        track_id: u32,
        solo: bool,
    },
    SetTrackArmed {
        track_id: u32,
        armed: bool,
    },
    SetPhaseInvert {
        track_id: u32,
        inverted: bool,
    },
    SetRoute {
        source_id: u32,
        dest_id: u32,
        enabled: bool,
    },
    /// Set the gain of a normal audio route.  This is separate from
    /// feedback-route gain because normal sends are allowed to participate
    /// in the ordinary acyclic graph and must retain their own undo record.
    SetRouteGain {
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    },
    SetFeedbackRoute {
        source_id: u32,
        dest_id: u32,
        gain: f32,
        enabled: bool,
    },
    SetSidechainLink {
        source_id: u32,
        dest_id: u32,
        tap_point: u32,
        plugin_index: u32,
        enabled: bool,
    },
    MoveRegion {
        track_id: u32,
        region_id: u32,
        start: f64,
    },
    SplitRegion {
        track_id: u32,
        region_id: u32,
        beat: f64,
    },
    SplitRegionWithCrossfade {
        track_id: u32,
        region_id: u32,
        beat: f64,
        ratio: f32,
    },
    DuplicateRegion {
        track_id: u32,
        region_id: u32,
        start: f64,
    },
    RemoveRegion {
        track_id: u32,
        region_id: u32,
    },
    SetRegionFades {
        track_id: u32,
        region_id: u32,
        fade_in: f32,
        fade_out: f32,
    },
    SetRegionTrim {
        track_id: u32,
        region_id: u32,
        start: f32,
        end: f32,
    },
    SetRegionLoop {
        track_id: u32,
        region_id: u32,
        count: u32,
    },
    SetRegionReverse {
        track_id: u32,
        region_id: u32,
        reverse: bool,
    },
    SetRegionMuted {
        track_id: u32,
        region_id: u32,
        muted: bool,
    },
    TransportPlay,
    TransportPause,
    TransportStop,
    SetPlayhead {
        position: u64,
    },
    SetLoop {
        enabled: bool,
    },
    SetMetronome {
        enabled: bool,
    },
    SetCycleRange {
        start_sample: u64,
        end_sample: u64,
        enabled: bool,
    },
    RecordArm {
        sample_rate: f32,
        channels: u16,
        max_frames: u64,
    },
    RecordStart {
        sample_rate: f32,
        channels: u16,
        max_frames: u64,
        start_sample: u64,
        /// Input frames to consume before opening the recording take.
        /// Defaults to zero for the legacy immediate-start behavior.
        #[serde(default)]
        count_in_frames: u64,
    },
    RecordStop,
    SelectRecordingTake {
        index: u32,
    },
    RegisterCompTake {
        take_id: u32,
        name: String,
        start_sample: u64,
        end_sample: u64,
    },
    SelectCompTake {
        take_id: u32,
    },
    RemoveCompTake {
        take_id: u32,
    },
    SetCompSegments {
        segments: Vec<CompSegmentCommand>,
    },
    RecordCommit {
        track_id: u32,
        project_path: Option<String>,
    },
    SetTempo {
        bpm: f32,
    },
    SetTimeSignature {
        beat: f64,
        numerator: u8,
        denominator: u8,
    },
    SetAutomation {
        track_id: u32,
        parameter_id: u32,
        /// Flat [time_samples, value, curve, ...] triples. Times are
        /// strictly increasing integer sample positions; values are 0..=1.
        points: Vec<f64>,
    },
    SetTrackDelayAutomation {
        track_id: u32,
        /// Flat [time_samples, normalized_delay, curve, ...] triples.
        points: Vec<f64>,
    },
    SetMidiNote {
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        #[serde(default)]
        lyric: String,
        #[serde(default)]
        phoneme: String,
        #[serde(default)]
        pitch_curve_cents: Vec<i16>,
        #[serde(default)]
        vibrato_depth_cents: u16,
        #[serde(default)]
        portamento_samples: u32,
    },
    ClearMidiNotes,
    RemoveMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
    },
    TransposeMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    },
    MoveMidiNotesRange {
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    },
    SetRegionWarp {
        track_id: u32,
        region_id: u32,
        ratio: f64,
    },
    SetRegionGain {
        track_id: u32,
        region_id: u32,
        gain_db: f32,
    },
    SetRegionPitch {
        track_id: u32,
        region_id: u32,
        semitones: f32,
    },
    SetRegionAudioNoteSegment {
        track_id: u32,
        region_id: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        #[serde(default)]
        formant_offset_cents: f64,
    },
    ClearRegionAudioNoteSegments {
        track_id: u32,
        region_id: u32,
    },
    WarpRegionAudioNoteSegment {
        track_id: u32,
        region_id: u32,
        segment_start_seconds: f64,
        new_start_seconds: f64,
        new_end_seconds: f64,
    },
    RemoveRegionAudioNoteSegment {
        track_id: u32,
        region_id: u32,
        segment_start_seconds: f64,
    },
    SetTrackName {
        track_id: u32,
        name: String,
    },
    Undo,
    Redo,
    ProjectInspect,
    RenderTargetCatalog,
    ProjectLoad {
        path: String,
    },
    SaveProject {
        path: String,
    },
    BounceProject {
        path: String,
        #[serde(default)]
        format: u32,
    },
    BounceStems {
        output_dir: String,
        #[serde(default)]
        format: u32,
        /// Optional explicit render targets. An empty list preserves the
        /// legacy behaviour of exporting every audio track.
        #[serde(default)]
        track_ids: Vec<u32>,
        /// Tail appended after the project end, in seconds.
        #[serde(default = "default_stem_tail_seconds")]
        tail_seconds: f32,
        #[serde(default)]
        pre_fader: bool,
        #[serde(default = "default_include_inserts")]
        include_inserts: bool,
    },
}

fn default_extension_timeout_ms() -> u64 {
    5_000
}
fn default_preview_sample_rate() -> f64 {
    48_000.0
}
fn default_preview_speed() -> f32 {
    1.0
}
fn default_preview_timing_ratio() -> f32 {
    1.0
}
fn default_silence_threshold() -> f32 {
    0.001
}
fn default_silence_min_length() -> u32 {
    256
}


