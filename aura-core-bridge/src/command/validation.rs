#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidatedCommand {
    pub schema_version: u32,
    pub command_version: u32,
    pub transaction: String,
    pub permission: Permission,
    pub expected_generation: Option<u64>,
    pub expected_audio_generation: Option<u64>,
    pub actions: Vec<CommandAction>,
    pub destructive: bool,
    pub mutation_class: MutationClass,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CommandDiff {
    pub index: usize,
    pub summary: String,
    pub destructive: bool,
}

pub fn diff(command: &ValidatedCommand) -> Vec<CommandDiff> {
    command
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let summary = match action {
                CommandAction::ControlInspect => "inspect macro and MIDI control mappings".into(),
                CommandAction::InspectMidiNotes => "inspect canonical MIDI notes and lyrics".into(),
                CommandAction::InspectChordTrack => "inspect canonical chord track".into(),
                CommandAction::AddChordEvent { tick, root, name, .. } => format!("add chord event {name} at tick {tick} root {root}"),
                CommandAction::PlaceGeneratedChord { track_id, start_sample, .. } => format!("place generated chord on track {track_id} at sample {start_sample}"),
                CommandAction::SuggestNextChords { last_chord_name } => format!("suggest next chords after {last_chord_name}"),
                CommandAction::GenerateArpeggio { steps, pattern, .. } => format!("generate arpeggio {steps} steps pattern {pattern}"),
                CommandAction::PlaceArpeggio { track_id, start_sample, steps, .. } => format!("place arpeggio on track {track_id} at {start_sample} for {steps} steps"),
                CommandAction::RemoveChordEventsRange { start_tick, end_tick } => format!("remove chord events from tick {start_tick} to {end_tick}"),
                CommandAction::ClearChordTrack => "clear chord track".into(),
                CommandAction::GenerateChord { root, octave, quality } => format!("generate chord root {root}, octave {octave}, quality {quality}"),
                CommandAction::DescribeDrumLane { pitch } => format!("describe drum lane pitch {pitch}"),
                CommandAction::AnalyzeDynamics { samples, track_id } => format!("analyze {} audio samples for dynamics suggestions{}", samples.len(), track_id.map(|id| format!(" on track {id}")).unwrap_or_default()),
                CommandAction::AnalyzeMix { left, right, .. } => format!("analyze stereo mix ({} left / {} right samples)", left.len(), right.len()),
                CommandAction::AnalyzeSilence { samples, threshold, min_length } => format!("analyze {} audio samples for silence at threshold {threshold} (minimum {min_length})", samples.len()),
                CommandAction::SplitRegionAtSilence { track_id, region_id, samples, .. } => format!("split region {region_id} on track {track_id} at detected silence ({} samples)", samples.len()),
                CommandAction::PreviewVocalPitchCorrection { samples, sample_rate, speed, timing_ratio } => format!("preview vocal pitch/timing correction for {} samples at {sample_rate} Hz speed {speed} timing {timing_ratio}", samples.len()),
                CommandAction::ApplyDynamicsSuggestion { track_id, plugin_index, samples } => format!("apply dynamics suggestion to plugin {plugin_index} on track {track_id} from {} samples", samples.len()),
                CommandAction::ProjectSearch { query } => format!("search project for: {query}"),
                CommandAction::ExtensionCatalog { root } => format!("discover extensions in {root}"),
                CommandAction::ExtensionInvoke { extension_id, command_id, .. } => format!("invoke extension command {extension_id}.{command_id}"),
                CommandAction::ExtensionValidate { extension_id, command_id, .. } => format!("validate extension command {extension_id}.{command_id}"),
                CommandAction::ExtensionSetEnabled { extension_id, enabled, .. } => format!("{} extension {extension_id}", if *enabled { "enable" } else { "disable" }),
                CommandAction::AddTrack { name, .. } => format!("add track: {name}"),
                CommandAction::AddAuxTrack { name } => format!("add Aux track: {name}"),
                CommandAction::RemoveTrack { track_id } => format!("remove track {track_id}"),
                CommandAction::DuplicateTrack { track_id } => format!("duplicate track {track_id}"),
                CommandAction::AddVcaGroup { group_id, gain } => format!("add VCA group {group_id} at gain {gain}"),
                CommandAction::AssignTrackToVca { track_id, group_id } => format!("assign track {track_id} to VCA group {group_id}"),
                CommandAction::SetVcaGroupGain { group_id, gain } => format!("set VCA group {group_id} gain to {gain}"),
                CommandAction::SetPluginFavorite { id, favorite } => format!("{} plugin favorite: {id}", if *favorite { "set" } else { "clear" }),
                CommandAction::PluginSearch { query, tag, favorites_only } => format!("search plugins query={query:?} tag={tag:?} favorites_only={favorites_only}"),
                CommandAction::AddPlugin {
                    track_id,
                    plugin_type,
                } => format!("insert plugin type {plugin_type} on track {track_id}"),
                CommandAction::FreezeTrack { track_id, total_samples, path } => format!("freeze track {track_id} for {total_samples} samples{}", path.as_deref().map(|value| format!(" into {value}")).unwrap_or_default()),
                CommandAction::FreezeTrackToProjectEnd { track_id } => format!("freeze track {track_id} to project end"),
                CommandAction::UnfreezeTrack { track_id } => format!("unfreeze track {track_id}"),
                CommandAction::TrackFreezeStatus { track_id } => format!("inspect freeze status for track {track_id}"),
                CommandAction::RemovePlugin { track_id, plugin_index } => format!("remove plugin {plugin_index} from track {track_id}"),
                CommandAction::MovePlugin { track_id, from_index, to_index } => format!("move plugin {from_index} to {to_index} on track {track_id}"),
                CommandAction::SetPluginParameter { track_id, plugin_index, parameter_id, value } => format!("set plugin {plugin_index} parameter {parameter_id} on track {track_id}: {value}"),
                CommandAction::SetPluginBypass { track_id, plugin_index, bypassed } => format!("{} plugin {plugin_index} on track {track_id}", if *bypassed { "bypass" } else { "enable" }),
                CommandAction::SetMacroValue { macro_index, value } => format!("set macro {macro_index}: {value}"),
                CommandAction::AddMacroMapping { mapping_id, macro_index, target_instance_id, target_parameter_id, .. } => format!("map macro {macro_index} ({mapping_id}) to {target_instance_id}:{target_parameter_id}"),
                CommandAction::RemoveMacroMapping { mapping_id } => format!("remove macro mapping {mapping_id}"),
                CommandAction::AddMidiLearnMapping { mapping_id, device_id, controller, target_instance_id, target_parameter_id, .. } => format!("map MIDI {device_id}:{controller} ({mapping_id}) to {target_instance_id}:{target_parameter_id}"),
                CommandAction::RemoveMidiLearnMapping { mapping_id } => format!("remove MIDI mapping {mapping_id}"),
                CommandAction::HumanizeMidi { timing_beats, velocity, seed } => format!("humanize MIDI timing {timing_beats}, velocity {velocity}, seed {seed}"),
                CommandAction::ApplyMidiSwing { subdivision_beats, amount } => format!("apply MIDI swing {amount} at {subdivision_beats} beats"),
                CommandAction::QuantizeMidi { grid_beats, strength } => format!("quantize MIDI to {grid_beats} beats ({strength})"),
                CommandAction::ApplyMidiLogicalRule { .. } => "apply MIDI Logical Editor rule".into(),
                CommandAction::TakeMixSnapshot { name, .. } => format!("capture MixConsole snapshot {name}"),
                CommandAction::CaptureMixSnapshot { name } => format!("capture live MixConsole snapshot {name}"),
                CommandAction::DiffMixSnapshots { first, second } => format!("compare MixConsole snapshots {first} and {second}"),
                CommandAction::RecallMixSnapshot { index } => format!("recall MixConsole snapshot {index}"),
                CommandAction::ApplyMixSnapshot { index } => format!("apply MixConsole snapshot {index}"),
                CommandAction::InsertNamedPlugin { track_id, alias } =>
                    format!("insert installed plugin {alias} on track {track_id}"),
                CommandAction::InsertPluginPath { track_id, path } =>
                    format!("insert plugin bundle {path} on track {track_id}"),
                CommandAction::OpenUtauImport { track_id, source_path, rendered_audio_path } =>
                    format!("attach OpenUtau source {source_path} and render {rendered_audio_path} to track {track_id}"),
                CommandAction::OpenUtauNotes { source_path } =>
                    format!("inspect OpenUtau notes from {source_path}"),
                CommandAction::OpenUtauImportMidi { track_id, source_path, .. } =>
                    format!("import OpenUtau MIDI notes from {source_path} into track {track_id}"),
                CommandAction::AddAudioRegion { track_id, path, start } => format!("add audio region {path} to track {track_id} at {start}"),
                CommandAction::ReplaceRegionAudio { track_id, region_id, path } => format!("replace audio for region {region_id} on track {track_id}: {path}"),
                CommandAction::PluginCatalog => "inspect installed plugin catalog".into(),
                CommandAction::SetVolume { track_id, value } => {
                    format!("set track {track_id} volume: {value}")
                }
                CommandAction::ApplyGainStaging { track_id, gain_db } => format!("apply {gain_db:.2} dB gain staging to track {track_id}"),
                CommandAction::SetEq { track_id, .. } => format!("set EQ on track {track_id}"),
                CommandAction::SetTrackDelay { track_id, samples } => {
                    format!("set track {track_id} delay: {samples} samples")
                }
                CommandAction::SetLowLatencyMode { enabled } => {
                    format!("{} low-latency monitoring", if *enabled { "enable" } else { "disable" })
                }
                CommandAction::SetTonalScale { root, scale_type } => {
                    format!("set tonal scale root {root}, type {scale_type}")
                }
                CommandAction::SetPan { track_id, value } => {
                    format!("set track {track_id} pan: {value}")
                }
                CommandAction::SetMute { track_id, muted } => format!("set track {track_id} mute: {muted}"),
                CommandAction::SetSolo { track_id, solo } => format!("set track {track_id} solo: {solo}"),
                CommandAction::SetTrackArmed { track_id, armed } => format!("{} recording arm for track {track_id}", if *armed { "enable" } else { "disable" }),
                CommandAction::SetPhaseInvert { track_id, inverted } => format!("{} phase invert on track {track_id}", if *inverted { "enable" } else { "disable" }),
                CommandAction::SetRoute { source_id, dest_id, enabled } => format!("{} route {source_id} -> {dest_id}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetRouteGain { source_id, dest_id, gain, enabled } => format!("{} route {source_id} -> {dest_id} gain {gain}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetFeedbackRoute { source_id, dest_id, gain, enabled } => format!("{} feedback route {source_id} -> {dest_id} gain {gain}", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetSidechainLink { source_id, dest_id, tap_point, plugin_index, enabled } => format!("{} sidechain {source_id} -> {dest_id} tap {tap_point} plugin {plugin_index}", if *enabled { "enable" } else { "disable" }),
                CommandAction::MoveRegion { track_id, region_id, start } => format!("move region {region_id} on track {track_id} to beat {start}"),
                CommandAction::SplitRegion { track_id, region_id, beat } => format!("split region {region_id} on track {track_id} at beat {beat}"),
                CommandAction::SplitRegionWithCrossfade { track_id, region_id, beat, ratio } => format!("split region {region_id} on track {track_id} at beat {beat} with crossfade {ratio}"),
                CommandAction::DuplicateRegion { track_id, region_id, start } => format!("duplicate region {region_id} on track {track_id} at beat {start}"),
                CommandAction::RemoveRegion { track_id, region_id } => format!("remove region {region_id} from track {track_id}"),
                CommandAction::SetRegionFades { track_id, region_id, fade_in, fade_out } => format!("set region {region_id} on track {track_id} fades: {fade_in}/{fade_out}"),
                CommandAction::SetRegionTrim { track_id, region_id, start, end } => format!("trim region {region_id} on track {track_id}: {start}..{end}"),
                CommandAction::SetRegionLoop { track_id, region_id, count } => format!("loop region {region_id} on track {track_id}: {count}"),
                CommandAction::SetRegionReverse { track_id, region_id, reverse } => format!("{} reverse region {region_id} on track {track_id}", if *reverse { "enable" } else { "disable" }),
                CommandAction::SetRegionMuted { track_id, region_id, muted } => format!("{} mute region {region_id} on track {track_id}", if *muted { "enable" } else { "disable" }),
                CommandAction::TransportPlay => "start transport".into(),
                CommandAction::TransportPause => "pause transport".into(),
                CommandAction::TransportStop => "stop transport".into(),
                CommandAction::SetPlayhead { position } => format!("set playhead: {position}"),
                CommandAction::SetLoop { enabled } => format!("{} cycle loop", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetMetronome { enabled } => format!("{} metronome", if *enabled { "enable" } else { "disable" }),
                CommandAction::SetCycleRange { start_sample, end_sample, enabled } => format!("{} cycle range {start_sample}..{end_sample}", if *enabled { "enable" } else { "disable" }),
                CommandAction::RecordArm { sample_rate, channels, max_frames } => format!("arm recording: {sample_rate} Hz, {channels} ch, {max_frames} frames"),
                CommandAction::RecordStart { sample_rate, channels, max_frames, start_sample, count_in_frames } => format!("start recording: {sample_rate} Hz, {channels} ch from {start_sample} for {max_frames} frames (count-in {count_in_frames})"),
                CommandAction::RecordStop => "stop recording capture".into(),
                CommandAction::SelectRecordingTake { index } => format!("select recording take {index}"),
                CommandAction::RegisterCompTake { take_id, name, .. } => format!("register comp take {take_id}: {name}"),
                CommandAction::SelectCompTake { take_id } => format!("select comp take {take_id}"),
                CommandAction::RemoveCompTake { take_id } => format!("remove comp take {take_id}"),
                CommandAction::SetCompSegments { segments } => format!("set {} comp segments", segments.len()),
                CommandAction::RecordCommit { track_id, project_path } => format!("commit recording to track {track_id}{}", project_path.as_deref().map(|path| format!(": {path}")).unwrap_or_default()),
                CommandAction::SetTempo { bpm } => format!("set tempo: {bpm} BPM"),
                CommandAction::SetTimeSignature { beat, numerator, denominator } => {
                    format!("set time signature at beat {beat}: {numerator}/{denominator}")
                }
                CommandAction::SetAutomation { track_id, parameter_id, points } => {
                    format!("set automation track {track_id} parameter {parameter_id} ({} points)", points.len() / 3)
                }
                CommandAction::SetTrackDelayAutomation { track_id, points } => {
                    format!("set track {track_id} delay automation ({} points)", points.len() / 3)
                }
                CommandAction::CreateTrackStack { stack_id, name, member_track_ids, .. } => {
                    format!("create track stack {stack_id} {name} ({} members)", member_track_ids.len())
                }
                CommandAction::DeleteTrackStack { stack_id } => format!("delete track stack {stack_id}"),
                CommandAction::UpsertMarker { marker_id, label, beat, .. } => format!("set marker {marker_id} {label} at {beat:.2} beats"),
                CommandAction::DeleteMarker { marker_id } => format!("delete marker {marker_id}"),
                CommandAction::SetTrackStackGain { stack_id, master_gain } => {
                    format!("set track stack {stack_id} gain {master_gain}")
                }
                CommandAction::SetTrackStackCollapsed { stack_id, collapsed } => {
                    format!("{} track stack {stack_id}", if *collapsed { "collapse" } else { "expand" })
                }
                CommandAction::SetMasterGain { value } => format!("set master gain {value}"),
                CommandAction::SetMidiNote { track_id, pitch, velocity, start_sample, length_samples, .. } => {
                    format!("set MIDI note track {track_id} pitch {pitch} velocity {velocity} at {start_sample} for {length_samples} samples")
                }
                CommandAction::ClearMidiNotes => "clear all MIDI notes".into(),
                CommandAction::RemoveMidiNotesRange { track_id, start_sample, end_sample } => format!("remove MIDI notes on track {track_id} in {start_sample}..{end_sample}"),
                CommandAction::TransposeMidiNotesRange { track_id, start_sample, end_sample, semitones } => format!("transpose MIDI notes on track {track_id} in {start_sample}..{end_sample} by {semitones}"),
                CommandAction::MoveMidiNotesRange { track_id, start_sample, end_sample, delta_samples } => format!("move MIDI notes on track {track_id} in {start_sample}..{end_sample} by {delta_samples} samples"),
                CommandAction::SetRegionWarp { track_id, region_id, ratio } => {
                    format!("set region {region_id} on track {track_id} warp ratio: {ratio}")
                }
                CommandAction::SetRegionGain { track_id, region_id, gain_db } => {
                    format!("set region {region_id} on track {track_id} gain: {gain_db} dB")
                }
                CommandAction::SetRegionPitch { track_id, region_id, semitones } => {
                    format!("set region {region_id} on track {track_id} pitch: {semitones} semitones")
                }
                CommandAction::SetRegionAudioNoteSegment { track_id, region_id, start_seconds, end_seconds, pitch_offset_cents, .. } => {
                    format!("edit region {region_id} on track {track_id} pitch segment {start_seconds}..{end_seconds}: {pitch_offset_cents} cents")
                }
                CommandAction::ClearRegionAudioNoteSegments { track_id, region_id } => {
                    format!("clear pitch segments on region {region_id} on track {track_id}")
                }
                CommandAction::WarpRegionAudioNoteSegment { track_id, region_id, segment_start_seconds, new_start_seconds, new_end_seconds } => {
                    format!("warp pitch segment {segment_start_seconds} on region {region_id} track {track_id} to {new_start_seconds}..{new_end_seconds}")
                }
                CommandAction::RemoveRegionAudioNoteSegment { track_id, region_id, segment_start_seconds } => {
                    format!("remove pitch segment {segment_start_seconds} on region {region_id} track {track_id}")
                }
                CommandAction::SetTrackName { track_id, name } => {
                    format!("rename track {track_id}: {name}")
                }
                CommandAction::Undo => "undo transaction".into(),
                CommandAction::Redo => "redo transaction".into(),
                CommandAction::ProjectInspect => "inspect project".into(),
                CommandAction::RenderTargetCatalog => "list renderable track, bus, and master targets".into(),
                CommandAction::ProjectLoad { path } => format!("load project: {path}"),
                CommandAction::SaveProject { path } => format!("save project: {path}"),
                CommandAction::BounceProject { path, format } => {
                    format!("bounce format {format}: {path}")
                }
                CommandAction::BounceStems { output_dir, format, track_ids, tail_seconds, pre_fader, include_inserts } => {
                    if track_ids.is_empty() {
                        format!("bounce all stems format {format} tail {tail_seconds}s pre_fader={pre_fader} inserts={include_inserts} into: {output_dir}")
                    } else {
                        format!("bounce {} selected stems format {format} tail {tail_seconds}s pre_fader={pre_fader} inserts={include_inserts} into: {output_dir}", track_ids.len())
                    }
                }
            };
            CommandDiff {
                index,
                summary,
                destructive: is_destructive(action),
            }
        })
        .collect()
}

pub fn mutation_class(action: &CommandAction) -> MutationClass {
    match action {
        CommandAction::ControlInspect
        | CommandAction::InspectMidiNotes
        | CommandAction::InspectChordTrack
        | CommandAction::GenerateChord { .. }
        | CommandAction::SuggestNextChords { .. }
        | CommandAction::GenerateArpeggio { .. }
        | CommandAction::PlaceArpeggio { .. }
        | CommandAction::DescribeDrumLane { .. }
        | CommandAction::AnalyzeDynamics { .. }
        | CommandAction::AnalyzeMix { .. }
        | CommandAction::AnalyzeSilence { .. }
        | CommandAction::PreviewVocalPitchCorrection { .. }
        | CommandAction::ProjectSearch { .. }
        | CommandAction::ExtensionCatalog { .. }
        | CommandAction::ExtensionValidate { .. }
        | CommandAction::ProjectInspect
        | CommandAction::RenderTargetCatalog
        | CommandAction::PluginCatalog
        | CommandAction::PluginSearch { .. }
        | CommandAction::TrackFreezeStatus { .. } => MutationClass::ReadOnly,
        CommandAction::OpenUtauNotes { .. } => MutationClass::ReadOnly,
        CommandAction::ExtensionInvoke { .. } => MutationClass::ExternalSideEffect,
        CommandAction::ExtensionSetEnabled { .. } => MutationClass::Reversible,
        CommandAction::OpenUtauImportMidi { .. } => MutationClass::Reversible,
        CommandAction::ProjectLoad { .. }
        | CommandAction::RemoveTrack { .. }
        | CommandAction::RemovePlugin { .. }
        | CommandAction::MovePlugin { .. }
        | CommandAction::ClearMidiNotes => MutationClass::Irreversible,
        CommandAction::RemoveMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::AddChordEvent { .. } => MutationClass::Reversible,
        CommandAction::PlaceGeneratedChord { .. } => MutationClass::Reversible,
        CommandAction::RemoveChordEventsRange { .. } | CommandAction::ClearChordTrack => {
            MutationClass::Reversible
        }
        CommandAction::TransposeMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::MoveMidiNotesRange { .. } => MutationClass::Reversible,
        CommandAction::ApplyMidiLogicalRule { .. } => MutationClass::Reversible,
        CommandAction::TakeMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::CaptureMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::DiffMixSnapshots { .. } => MutationClass::ReadOnly,
        CommandAction::RecallMixSnapshot { .. } => MutationClass::ReadOnly,
        CommandAction::ApplyMixSnapshot { .. } => MutationClass::Reversible,
        CommandAction::SaveProject { .. }
        | CommandAction::BounceProject { .. }
        | CommandAction::BounceStems { .. } => MutationClass::ExternalSideEffect,
        CommandAction::RecordCommit { .. } => MutationClass::ExternalSideEffect,
        CommandAction::Undo
        | CommandAction::Redo
        | CommandAction::AddTrack { .. }
        | CommandAction::AddAuxTrack { .. }
        | CommandAction::AddVcaGroup { .. }
        | CommandAction::AssignTrackToVca { .. }
        | CommandAction::SetVcaGroupGain { .. }
        | CommandAction::SetPluginFavorite { .. }
        | CommandAction::AddPlugin { .. }
        | CommandAction::FreezeTrack { .. }
        | CommandAction::FreezeTrackToProjectEnd { .. }
        | CommandAction::UnfreezeTrack { .. }
        | CommandAction::DuplicateTrack { .. }
        | CommandAction::SetPluginParameter { .. }
        | CommandAction::SetPluginBypass { .. }
        | CommandAction::SetMacroValue { .. }
        | CommandAction::AddMacroMapping { .. }
        | CommandAction::RemoveMacroMapping { .. }
        | CommandAction::AddMidiLearnMapping { .. }
        | CommandAction::RemoveMidiLearnMapping { .. }
        | CommandAction::HumanizeMidi { .. }
        | CommandAction::ApplyMidiSwing { .. }
        | CommandAction::QuantizeMidi { .. }
        | CommandAction::InsertNamedPlugin { .. }
        | CommandAction::InsertPluginPath { .. }
        | CommandAction::OpenUtauImport { .. }
        | CommandAction::AddAudioRegion { .. }
        | CommandAction::ReplaceRegionAudio { .. }
        | CommandAction::SetVolume { .. }
        | CommandAction::ApplyGainStaging { .. }
        | CommandAction::SetEq { .. }
        | CommandAction::SetMasterGain { .. }
        | CommandAction::SetTrackDelay { .. }
        | CommandAction::SetLowLatencyMode { .. }
        | CommandAction::SetTonalScale { .. }
        | CommandAction::SetPan { .. }
        | CommandAction::SetMute { .. }
        | CommandAction::SetSolo { .. }
        | CommandAction::SetTrackArmed { .. }
        | CommandAction::SetPhaseInvert { .. }
        | CommandAction::SetRoute { .. }
        | CommandAction::SetRouteGain { .. }
        | CommandAction::SetFeedbackRoute { .. }
        | CommandAction::SetSidechainLink { .. }
        | CommandAction::MoveRegion { .. }
        | CommandAction::SplitRegion { .. }
        | CommandAction::SplitRegionWithCrossfade { .. }
        | CommandAction::SplitRegionAtSilence { .. }
        | CommandAction::DuplicateRegion { .. }
        | CommandAction::RemoveRegion { .. }
        | CommandAction::SetRegionFades { .. }
        | CommandAction::SetRegionTrim { .. }
        | CommandAction::SetRegionLoop { .. }
        | CommandAction::SetRegionReverse { .. }
        | CommandAction::SetRegionMuted { .. }
        | CommandAction::TransportPlay
        | CommandAction::TransportPause
        | CommandAction::TransportStop
        | CommandAction::SetPlayhead { .. }
        | CommandAction::SetLoop { .. }
        | CommandAction::SetMetronome { .. }
        | CommandAction::SetCycleRange { .. }
        | CommandAction::SetTempo { .. }
        | CommandAction::SetTimeSignature { .. }
        | CommandAction::SetAutomation { .. }
        | CommandAction::SetTrackDelayAutomation { .. }
        | CommandAction::CreateTrackStack { .. }
        | CommandAction::DeleteTrackStack { .. }
        | CommandAction::UpsertMarker { .. }
        | CommandAction::DeleteMarker { .. }
        | CommandAction::SetTrackStackGain { .. }
        | CommandAction::SetTrackStackCollapsed { .. }
        | CommandAction::SetMidiNote { .. }
        | CommandAction::SetRegionWarp { .. }
        | CommandAction::SetRegionGain { .. }
        | CommandAction::SetRegionPitch { .. }
        | CommandAction::SetRegionAudioNoteSegment { .. }
        | CommandAction::ClearRegionAudioNoteSegments { .. }
        | CommandAction::WarpRegionAudioNoteSegment { .. }
        | CommandAction::RemoveRegionAudioNoteSegment { .. }
        | CommandAction::SetTrackName { .. }
        | CommandAction::ApplyDynamicsSuggestion { .. } => MutationClass::Reversible,
        CommandAction::RecordArm { .. }
        | CommandAction::RecordStart { .. }
        | CommandAction::RecordStop
        | CommandAction::SelectRecordingTake { .. }
        | CommandAction::RegisterCompTake { .. }
        | CommandAction::SelectCompTake { .. }
        | CommandAction::RemoveCompTake { .. }
        | CommandAction::SetCompSegments { .. } => MutationClass::Reversible,
    }
}

fn is_destructive(action: &CommandAction) -> bool {
    !matches!(
        mutation_class(action),
        MutationClass::ReadOnly | MutationClass::Reversible
    )
}


