pub fn validate(document: CommandDocument) -> Result<ValidatedCommand, String> {
    let transaction = document.transaction.trim().to_owned();
    if document.schema_version != 1 || document.command_version != 1 {
        return Err("unsupported command schema or version".into());
    }
    if !valid_token(&transaction, 128) {
        return Err("transaction must be 1..=128 characters".into());
    }
    if document.actions.is_empty() || document.actions.len() > 256 {
        return Err("actions must contain 1..=256 entries".into());
    }
    for action in &document.actions {
        match action {
            CommandAction::AddTrack { name, .. }
            | CommandAction::AddAuxTrack { name }
            | CommandAction::SetTrackName { name, .. } => {
                if name.trim().is_empty() || name.len() > 256 {
                    return Err("track names must be 1..=256 characters".into());
                }
            }
            CommandAction::SetVolume { value, .. }
                if !value.is_finite() || !(-2.0..=2.0).contains(value) =>
            {
                return Err("volume must be finite and within -2..=2".into());
            }
            CommandAction::ApplyGainStaging { gain_db, .. }
                if !gain_db.is_finite() || !(-24.0..=24.0).contains(gain_db) =>
            {
                return Err("gain staging correction must be finite and within -24..=24 dB".into());
            }
            CommandAction::SetEq {
                low_band,
                low_cut,
                high_band,
                high_cut,
                ..
            } if [low_band, low_cut, high_band, high_cut]
                .iter()
                .any(|value| !value.is_finite()) =>
            {
                return Err("EQ parameters must be finite".into());
            }
            CommandAction::SetTrackDelay { samples, .. } if *samples > 8192 => {
                return Err("track delay must be within 0..=8192 samples".into());
            }
            CommandAction::SetTonalScale { root, scale_type }
                if !(-128..=127).contains(root) || *scale_type > 10 =>
            {
                return Err("tonal scale root/type is outside the supported range".into());
            }
            CommandAction::GenerateChord {
                root,
                octave,
                quality,
            } if !(-128..=127).contains(root) || !(-1..=10).contains(octave) || *quality > 5 => {
                return Err("chord root, octave, or quality is outside the supported range".into());
            }
            CommandAction::SuggestNextChords { last_chord_name }
                if last_chord_name.len() > 128 || last_chord_name.contains('\0') =>
            {
                return Err("chord name is invalid".into());
            }
            CommandAction::GenerateArpeggio {
                pitches,
                velocities,
                pattern,
                octaves,
                steps,
            } if pitches.is_empty()
                || pitches.len() > 128
                || velocities.len() != pitches.len()
                || pitches.iter().any(|pitch| *pitch > 127)
                || velocities.contains(&0)
                || *pattern > 3
                || *octaves == 0
                || *octaves > 4
                || *steps == 0
                || *steps > 4096 =>
            {
                return Err("arpeggio parameters are invalid".into());
            }
            CommandAction::PlaceArpeggio {
                track_id,
                step_samples,
                gate_samples,
                pitches,
                velocities,
                pattern,
                octaves,
                steps,
                ..
            } if *track_id == 0
                || *step_samples == 0
                || *gate_samples == 0
                || *gate_samples > *step_samples
                || pitches.is_empty()
                || pitches.len() > 128
                || velocities.len() != pitches.len()
                || pitches.iter().any(|pitch| *pitch > 127)
                || velocities.contains(&0)
                || *pattern > 3
                || *octaves == 0
                || *octaves > 4
                || *steps == 0
                || *steps > 4096 =>
            {
                return Err("arpeggio placement parameters are invalid".into());
            }
            CommandAction::AddChordEvent {
                root,
                intervals,
                name,
                ..
            } if *root > 127
                || intervals.len() > 32
                || intervals.iter().any(|interval| *interval > 127)
                || name.trim().is_empty()
                || name.chars().count() > 128
                || name.contains('\0') =>
            {
                return Err("chord event fields are invalid".into());
            }
            CommandAction::PlaceGeneratedChord {
                track_id,
                length_samples,
                velocity,
                root,
                octave,
                quality,
                ..
            } if *track_id == 0
                || *length_samples == 0
                || *velocity == 0
                || !(-128..=127).contains(root)
                || !(-1..=10).contains(octave)
                || *quality > 5 =>
            {
                return Err("generated chord placement fields are invalid".into());
            }
            CommandAction::RemoveChordEventsRange {
                start_tick,
                end_tick,
            } if start_tick > end_tick => {
                return Err("chord event range is reversed".into());
            }
            CommandAction::SetPan { value, .. }
                if !value.is_finite() || !(-1.0..=1.0).contains(value) =>
            {
                return Err("pan must be finite and within -1..=1".into());
            }
            CommandAction::SetPluginParameter { value, .. } if !value.is_finite() => {
                return Err("plugin parameter must be finite".into());
            }
            CommandAction::AddVcaGroup { group_id, gain }
            | CommandAction::SetVcaGroupGain { group_id, gain }
                if *group_id == 0 || !gain.is_finite() || !(0.0..=8.0).contains(gain) =>
            {
                return Err(
                    "VCA group id must be nonzero and gain must be finite within 0..=8".into(),
                );
            }
            CommandAction::SetPluginFavorite { id, .. } if !valid_token(id, 256) => {
                return Err("plugin favorite id must be 1..=256 token characters".into());
            }
            CommandAction::AssignTrackToVca { track_id, group_id }
                if *track_id == 0 || *group_id == 0 =>
            {
                return Err("VCA track and group ids must be nonzero".into());
            }
            CommandAction::SetMacroValue { macro_index, value }
                if *macro_index >= 128 || !value.is_finite() || !(0.0..=1.0).contains(value) =>
            {
                return Err("macro index must be below 128 and value within 0..=1".into());
            }
            CommandAction::AddMacroMapping {
                mapping_id,
                macro_index,
                target_instance_id,
                target_parameter_id,
                min,
                max,
                curve,
                ..
            } => {
                if !valid_token(mapping_id, 128)
                    || *macro_index >= 128
                    || !valid_token(target_instance_id, 256)
                    || target_parameter_id.trim().is_empty()
                    || target_parameter_id.len() > 128
                    || !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || !curve.is_finite()
                    || !(-1.0..=1.0).contains(curve)
                {
                    return Err("invalid macro mapping".into());
                }
            }
            CommandAction::RemoveMacroMapping { mapping_id } if !valid_token(mapping_id, 128) => {
                return Err("invalid macro mapping id".into());
            }
            CommandAction::AddMidiLearnMapping {
                mapping_id,
                device_id,
                channel,
                controller,
                target_instance_id,
                target_parameter_id,
                min,
                max,
                curve,
                ..
            } => {
                if !valid_token(mapping_id, 128)
                    || device_id.trim().is_empty()
                    || device_id.len() > 256
                    || *channel > 15
                    || *controller > 16_383
                    || !valid_token(target_instance_id, 256)
                    || target_parameter_id.trim().is_empty()
                    || target_parameter_id.len() > 128
                    || !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || !curve.is_finite()
                    || !(-1.0..=1.0).contains(curve)
                {
                    return Err("invalid MIDI learn mapping".into());
                }
            }
            CommandAction::RemoveMidiLearnMapping { mapping_id }
                if !valid_token(mapping_id, 128) =>
            {
                return Err("invalid MIDI mapping id".into());
            }
            CommandAction::HumanizeMidi {
                timing_beats,
                velocity,
                ..
            } if !timing_beats.is_finite()
                || !(-4.0..=4.0).contains(timing_beats)
                || !(-127..=127).contains(velocity) =>
            {
                return Err("MIDI humanize parameters are out of range".into());
            }
            CommandAction::ApplyMidiSwing {
                subdivision_beats,
                amount,
            } if !subdivision_beats.is_finite()
                || !(0.001..=16.0).contains(subdivision_beats)
                || !amount.is_finite()
                || !(-1.0..=1.0).contains(amount) =>
            {
                return Err("MIDI swing parameters are out of range".into());
            }
            CommandAction::QuantizeMidi {
                grid_beats,
                strength,
            } if !grid_beats.is_finite()
                || !(0.001..=16.0).contains(grid_beats)
                || !strength.is_finite()
                || !(0.0..=1.0).contains(strength) =>
            {
                return Err("MIDI quantize parameters are out of range".into());
            }
            CommandAction::SetAutomation { points, .. } => {
                if points.len() % 3 != 0 || points.len() > 24576 {
                    return Err(
                        "automation points must contain 0..=8192 time/value/curve triples".into(),
                    );
                }
                let mut previous_time = f64::NEG_INFINITY;
                for triple in points.chunks_exact(3) {
                    let time = triple[0];
                    let value = triple[1];
                    let curve = triple[2];
                    if !time.is_finite() || !value.is_finite() || !curve.is_finite() {
                        return Err("automation points must be finite".into());
                    }
                    if time < 0.0 || time.fract() != 0.0 {
                        return Err(
                            "automation time must be a non-negative integer sample position".into(),
                        );
                    }
                    if !(0.0..=1.0).contains(&value) {
                        return Err("automation value must be normalized within 0..=1".into());
                    }
                    if !(-1.0..=1.0).contains(&curve) {
                        return Err("automation curve must be normalized within -1..=1".into());
                    }
                    if time <= previous_time {
                        return Err("automation times must be strictly increasing".into());
                    }
                    previous_time = time;
                }
            }
            CommandAction::SetTrackDelayAutomation { points, .. } => {
                if points.len() % 3 != 0 || points.len() > 24576 {
                    return Err("track delay automation must contain 0..=8192 triples".into());
                }
                let mut previous_time = f64::NEG_INFINITY;
                for triple in points.chunks_exact(3) {
                    let time = triple[0];
                    if !time.is_finite()
                        || time < 0.0
                        || time.fract() != 0.0
                        || time <= previous_time
                        || !triple[1].is_finite()
                        || !(0.0..=1.0).contains(&triple[1])
                        || !triple[2].is_finite()
                        || !(-1.0..=1.0).contains(&triple[2])
                    {
                        return Err("track delay automation requires increasing sample times and normalized values".into());
                    }
                    previous_time = time;
                }
            }
            CommandAction::CreateTrackStack {
                stack_id,
                name,
                member_track_ids,
                master_gain,
                ..
            } => {
                let mut unique_members =
                    std::collections::HashSet::with_capacity(member_track_ids.len());
                if *stack_id == 0
                    || name.trim().is_empty()
                    || name.len() > 256
                    || member_track_ids.is_empty()
                    || member_track_ids.contains(&0)
                    || member_track_ids
                        .iter()
                        .any(|id| !unique_members.insert(*id))
                    || !master_gain.is_finite()
                    || !(0.0..=2.0).contains(master_gain)
                {
                    return Err("invalid track stack definition".into());
                }
            }
            CommandAction::DeleteTrackStack { stack_id } => {
                if *stack_id == 0 {
                    return Err("track stack id must be nonzero".into());
                }
            }
            CommandAction::UpsertMarker {
                marker_id,
                label,
                beat,
                color,
            } => {
                if *marker_id == 0
                    || label.trim().is_empty()
                    || label.len() > 128
                    || label.contains('\0')
                    || !beat.is_finite()
                    || *beat < 0.0
                    || color.len() > 32
                    || color.contains('\0')
                {
                    return Err("invalid arrangement marker".into());
                }
            }
            CommandAction::DeleteMarker { marker_id } => {
                if *marker_id == 0 {
                    return Err("marker id must be nonzero".into());
                }
            }
            CommandAction::SetTrackStackGain { master_gain, .. } => {
                if !master_gain.is_finite() || !(0.0..=2.0).contains(master_gain) {
                    return Err("track stack gain is outside 0..=2".into());
                }
            }
            CommandAction::SetMasterGain { value } => {
                if !value.is_finite() || !(0.0..=2.0).contains(value) {
                    return Err("master gain is outside 0..=2".into());
                }
            }
            CommandAction::RegisterCompTake {
                take_id,
                name,
                start_sample,
                end_sample,
            } => {
                if *take_id == 0
                    || name.trim().is_empty()
                    || name.len() > 128
                    || name.contains('\0')
                    || *end_sample <= *start_sample
                {
                    return Err("invalid comp take metadata".into());
                }
            }
            CommandAction::SelectCompTake { take_id } => {
                if *take_id == 0 {
                    return Err("comp take id must be nonzero".into());
                }
            }
            CommandAction::RemoveCompTake { take_id } => {
                if *take_id == 0 {
                    return Err("comp take id must be nonzero".into());
                }
            }
            CommandAction::SetCompSegments { segments } => {
                if segments.len() > 262_144 {
                    return Err("too many comp segments".into());
                }
                let mut sorted = segments.iter().collect::<Vec<_>>();
                sorted.sort_by_key(|segment| segment.start_sample);
                for segment in &sorted {
                    if segment.take_id == 0
                        || segment.length_samples == 0
                        || segment.crossfade_samples as u64 > segment.length_samples
                        || segment
                            .start_sample
                            .checked_add(segment.length_samples)
                            .is_none()
                    {
                        return Err("invalid comp segment bounds".into());
                    }
                }
                if sorted.windows(2).any(|pair| {
                    pair[0]
                        .start_sample
                        .checked_add(pair[0].length_samples)
                        .is_none_or(|end| end > pair[1].start_sample)
                }) {
                    return Err("comp segments overlap".into());
                }
            }
            CommandAction::SetMidiNote {
                pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme,
                pitch_curve_cents,
                ..
            } => {
                if *pitch > 127 || *velocity == 0 || *length_samples == 0 {
                    return Err(
                        "MIDI note pitch/velocity must be 0..=127 and length must be positive"
                            .into(),
                    );
                }
                if start_sample.checked_add(*length_samples).is_none() {
                    return Err("MIDI note range overflows the sample timeline".into());
                }
                if lyric.len() > 1_024 || lyric.contains('\0') {
                    return Err(
                        "MIDI note lyric must be at most 1024 bytes and contain no NUL".into(),
                    );
                }
                if phoneme.len() > 128 || phoneme.contains('\0') {
                    return Err(
                        "MIDI note phoneme must be at most 128 bytes and contain no NUL".into(),
                    );
                }
                if pitch_curve_cents.len() > 256 {
                    return Err("MIDI note pitch curve must contain at most 256 points".into());
                }
            }
            CommandAction::RemoveMidiNotesRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("MIDI note range must be ordered and non-empty".into());
            }
            CommandAction::TransposeMidiNotesRange {
                start_sample,
                end_sample,
                semitones,
                ..
            } if start_sample >= end_sample || !(-127..=127).contains(semitones) => {
                return Err("MIDI transpose range or semitones are invalid".into());
            }
            CommandAction::MoveMidiNotesRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("MIDI move range must be ordered and non-empty".into());
            }
            CommandAction::SetRegionWarp { ratio, .. }
                if !ratio.is_finite() || !(0.25..=4.0).contains(ratio) =>
            {
                return Err("region warp ratio must be finite and within 0.25..=4.0".into());
            }
            CommandAction::SetRegionGain { gain_db, .. }
                if !gain_db.is_finite() || !(-24.0..=24.0).contains(gain_db) =>
            {
                return Err("region gain must be finite and within -24..=24 dB".into());
            }
            CommandAction::SetRegionPitch { semitones, .. }
                if !semitones.is_finite() || !(-48.0..=48.0).contains(semitones) =>
            {
                return Err("region pitch must be finite and within -48..=48 semitones".into());
            }
            CommandAction::SetRegionAudioNoteSegment {
                start_seconds,
                end_seconds,
                pitch_offset_cents,
                formant_offset_cents,
                ..
            } if !start_seconds.is_finite()
                || !end_seconds.is_finite()
                || *end_seconds <= *start_seconds
                || *end_seconds - *start_seconds > 24.0 * 60.0
                || !pitch_offset_cents.is_finite()
                || pitch_offset_cents.abs() > 4800.0
                || !formant_offset_cents.is_finite()
                || formant_offset_cents.abs() > 2400.0 =>
            {
                return Err("audio note segment timing or offsets are invalid".into());
            }
            CommandAction::WarpRegionAudioNoteSegment {
                segment_start_seconds,
                new_start_seconds,
                new_end_seconds,
                ..
            } if !segment_start_seconds.is_finite()
                || *segment_start_seconds < 0.0
                || !new_start_seconds.is_finite()
                || *new_start_seconds < 0.0
                || !new_end_seconds.is_finite()
                || *new_end_seconds <= *new_start_seconds
                || *new_end_seconds - *new_start_seconds > 24.0 * 60.0 =>
            {
                return Err("audio note segment warp timing is invalid".into());
            }
            CommandAction::RemoveRegionAudioNoteSegment {
                segment_start_seconds,
                ..
            } if !segment_start_seconds.is_finite() || *segment_start_seconds < 0.0 => {
                return Err("audio note segment start must be finite and non-negative".into());
            }
            CommandAction::SetTempo { bpm }
                if !bpm.is_finite() || !(20.0..=400.0).contains(bpm) =>
            {
                return Err("tempo must be finite and within 20..=400 BPM".into());
            }
            CommandAction::SetTimeSignature {
                beat,
                numerator,
                denominator,
            } if !beat.is_finite()
                || *beat < 0.0
                || *numerator == 0
                || *numerator > 32
                || !matches!(*denominator, 1 | 2 | 4 | 8 | 16 | 32) =>
            {
                return Err("time signature must be a valid beat and meter".into());
            }
            CommandAction::RecordArm {
                sample_rate,
                channels,
                max_frames,
            } => {
                if !sample_rate.is_finite()
                    || !(8_000.0..=384_000.0).contains(sample_rate)
                    || *channels == 0
                    || *channels > 64
                    || *max_frames == 0
                    || *max_frames > 16_777_216
                {
                    return Err("recording format is outside supported limits".into());
                }
            }
            CommandAction::RecordStart {
                sample_rate,
                channels,
                max_frames,
                count_in_frames,
                ..
            } => {
                if !sample_rate.is_finite()
                    || !(8_000.0..=384_000.0).contains(sample_rate)
                    || *channels == 0
                    || *channels > 64
                    || *max_frames == 0
                    || *max_frames > 16_777_216
                    || *count_in_frames > 16_777_216
                {
                    return Err("recording format is outside supported limits".into());
                }
            }
            CommandAction::SetCycleRange {
                start_sample,
                end_sample,
                ..
            } if start_sample >= end_sample => {
                return Err("cycle range must be ordered and non-empty".into());
            }
            CommandAction::SplitRegion { beat, .. } if !beat.is_finite() || *beat <= 0.0 => {
                return Err("region split beat must be finite and positive".into());
            }
            CommandAction::SplitRegionWithCrossfade { beat, ratio, .. }
                if !beat.is_finite()
                    || *beat <= 0.0
                    || !ratio.is_finite()
                    || !(0.0..=1.0).contains(ratio) =>
            {
                return Err(
                    "crossfade split requires a positive beat and ratio within 0..=1".into(),
                );
            }
            CommandAction::DuplicateRegion { start, .. } if !start.is_finite() || *start < 0.0 => {
                return Err("region duplicate start must be finite and non-negative".into());
            }
            CommandAction::MoveRegion { start, .. } if !start.is_finite() || *start < 0.0 => {
                return Err("region start must be finite and non-negative".into());
            }
            CommandAction::RemoveRegion {
                track_id,
                region_id,
            } if *track_id == 0 || *region_id == 0 => {
                return Err("region removal requires non-zero track and region ids".into());
            }
            CommandAction::SetRegionFades {
                fade_in, fade_out, ..
            } if !fade_in.is_finite()
                || !fade_out.is_finite()
                || *fade_in < 0.0
                || *fade_out < 0.0
                || *fade_in > 1.0
                || *fade_out > 1.0 =>
            {
                return Err("region fades must be finite and within 0..=1".into());
            }
            CommandAction::SetRegionTrim { start, end, .. }
                if !start.is_finite()
                    || !end.is_finite()
                    || *start < 0.0
                    || *end > 1.0
                    || *start >= *end =>
            {
                return Err("region trim must be finite, ordered, and within 0..=1".into());
            }
            CommandAction::SetRegionLoop { count, .. } if !(1..=1024).contains(count) => {
                return Err("region loop count must be within 1..=1024".into());
            }
            CommandAction::InsertNamedPlugin { alias, .. }
                if alias.trim().is_empty() || alias.len() > 256 =>
            {
                return Err("plugin alias must be 1..=256 characters".into());
            }
            CommandAction::InsertPluginPath { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("plugin path must be 1..=4096 characters".into());
            }
            CommandAction::FreezeTrack {
                track_id,
                total_samples,
                ..
            } if *track_id == 0 || *total_samples == 0 || *total_samples > 64 * 1024 * 1024 => {
                return Err("freeze track id or sample range is invalid".into());
            }
            CommandAction::FreezeTrackToProjectEnd { track_id } if *track_id == 0 => {
                return Err("freeze track id is invalid".into());
            }
            CommandAction::SetFeedbackRoute {
                source_id,
                dest_id,
                gain,
                ..
            } if *source_id == *dest_id || !gain.is_finite() || !(0.0..=2.0).contains(gain) => {
                return Err("feedback route requires distinct nodes and gain within 0..=2".into());
            }
            CommandAction::SetRouteGain {
                source_id,
                dest_id,
                gain,
                enabled,
            } if *source_id == *dest_id
                || !gain.is_finite()
                || !(0.0..=2.0).contains(gain)
                || (*enabled && *gain <= 0.0) =>
            {
                return Err("route gain requires distinct nodes, a finite gain within 0..=2, and positive gain when enabled".into());
            }
            CommandAction::FreezeTrack {
                path: Some(path), ..
            } if path.trim().is_empty() || path.len() > 4096 => {
                return Err("freeze cache path must be 1..=4096 characters".into());
            }
            CommandAction::OpenUtauImport {
                source_path,
                rendered_audio_path,
                ..
            } if source_path.trim().is_empty()
                || rendered_audio_path.trim().is_empty()
                || source_path.len() > 4096
                || rendered_audio_path.len() > 4096 =>
            {
                return Err("OpenUtau paths must be 1..=4096 characters".into());
            }
            CommandAction::OpenUtauNotes { source_path }
                if source_path.trim().is_empty()
                    || source_path.len() > 4096
                    || source_path.contains('\0') =>
            {
                return Err("OpenUtau source path must be 1..=4096 characters without NUL".into());
            }
            CommandAction::OpenUtauImportMidi {
                track_id,
                source_path,
                sample_rate,
                ticks_per_beat,
            } if *track_id == 0
                || source_path.trim().is_empty()
                || source_path.len() > 4096
                || source_path.contains('\0')
                || *sample_rate == 0
                || *sample_rate > 384_000
                || *ticks_per_beat == 0
                || *ticks_per_beat > 32_768 =>
            {
                return Err(
                    "OpenUtau MIDI import has invalid track, path, sample rate, or PPQ".into(),
                );
            }
            CommandAction::AddAudioRegion { path, start, .. }
                if path.trim().is_empty()
                    || path.len() > 4096
                    || !start.is_finite()
                    || *start < 0.0 =>
            {
                return Err("audio region path/start is invalid".into());
            }
            CommandAction::ReplaceRegionAudio { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("replacement audio path is invalid".into());
            }
            CommandAction::ProjectLoad { path }
            | CommandAction::SaveProject { path }
            | CommandAction::BounceProject { path, .. }
                if path.trim().is_empty() || path.len() > 4096 =>
            {
                return Err("file paths must be 1..=4096 characters".into());
            }
            CommandAction::BounceStems { output_dir, .. }
                if output_dir.trim().is_empty() || output_dir.len() > 4096 =>
            {
                return Err("stem output directory must be 1..=4096 characters".into());
            }
            CommandAction::BounceStems {
                track_ids,
                tail_seconds,
                ..
            } => {
                if track_ids.len() > 4096 || track_ids.contains(&0) {
                    return Err("stem track selection is invalid".into());
                }
                let mut unique = std::collections::HashSet::with_capacity(track_ids.len());
                if track_ids.iter().any(|id| !unique.insert(id)) {
                    return Err("stem track selection contains duplicates".into());
                }
                if !tail_seconds.is_finite() || !(0.0..=60.0).contains(tail_seconds) {
                    return Err("stem tail_seconds must be finite and within 0..=60".into());
                }
            }
            CommandAction::RecordCommit {
                project_path: Some(path),
                ..
            } if path.trim().is_empty() || path.len() > 4096 => {
                return Err("recording project path must be 1..=4096 characters".into());
            }
            CommandAction::ExtensionCatalog { root }
                if root.trim().is_empty() || root.len() > 4096 =>
            {
                return Err("extension root must be 1..=4096 characters".into());
            }
            CommandAction::ProjectSearch { query }
                if query.trim().is_empty() || query.len() > 256 || query.contains('\0') =>
            {
                return Err("project search query must be 1..=256 characters without NUL".into());
            }
            CommandAction::AnalyzeDynamics { samples, .. }
                if samples.len() > 262_144 || samples.iter().any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "dynamics analysis samples must be finite and contain at most 262144 values"
                        .into(),
                );
            }
            CommandAction::AnalyzeMix { left, right, .. }
                if left.len() > 262_144
                    || right.len() > 262_144
                    || left.iter().chain(right).any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "mix analysis requires finite channels with at most 262144 samples each".into(),
                );
            }
            CommandAction::AnalyzeSilence {
                samples,
                threshold,
                min_length,
            } if samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !threshold.is_finite()
                || !(0.0..=1.0).contains(threshold)
                || *min_length == 0
                || *min_length as usize > 262_144 =>
            {
                return Err("silence analysis requires finite samples, threshold 0..=1, and a valid minimum length".into());
            }
            CommandAction::SplitRegionAtSilence {
                track_id,
                region_id,
                samples,
                threshold,
                min_length,
            } if *track_id == 0
                || *region_id == 0
                || samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !threshold.is_finite()
                || !(0.0..=1.0).contains(threshold)
                || *min_length == 0
                || *min_length > 262_144 =>
            {
                return Err("silence split parameters are invalid or exceed bounds".into());
            }
            CommandAction::PreviewVocalPitchCorrection {
                samples,
                sample_rate,
                speed,
                timing_ratio,
            } if samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite())
                || !sample_rate.is_finite()
                || !(8_000.0..=384_000.0).contains(sample_rate)
                || !speed.is_finite()
                || !(0.0..=1.0).contains(speed)
                || !timing_ratio.is_finite()
                || !(0.25..=4.0).contains(timing_ratio) =>
            {
                return Err("vocal preview requires finite samples, sample rate 8000..384000, and speed 0..=1".into());
            }
            CommandAction::ApplyDynamicsSuggestion {
                track_id, samples, ..
            } if *track_id == 0
                || samples.len() > 262_144
                || samples.iter().any(|sample| !sample.is_finite()) =>
            {
                return Err(
                    "dynamics target is invalid or samples exceed 262144 finite values".into(),
                );
            }
            CommandAction::ExtensionValidate {
                root,
                extension_id,
                command_id,
                ..
            } if root.trim().is_empty()
                || root.len() > 4096
                || !valid_token(extension_id, 128)
                || !valid_token(command_id, 128) =>
            {
                return Err("extension validation request is invalid".into());
            }
            CommandAction::ExtensionInvoke {
                root,
                extension_id,
                command_id,
                timeout_ms,
                ..
            } if root.trim().is_empty()
                || root.len() > 4096
                || root.contains('\0')
                || !valid_token(extension_id, 128)
                || !valid_token(command_id, 128)
                || !(1..=30_000).contains(timeout_ms) =>
            {
                return Err("extension invocation request is invalid".into());
            }
            CommandAction::ExtensionSetEnabled {
                root, extension_id, ..
            } => {
                if root.trim().is_empty()
                    || root.len() > 4096
                    || root.contains('\0')
                    || !valid_token(extension_id, 128)
                {
                    return Err("extension activation request is invalid".into());
                }
            }
            CommandAction::TakeMixSnapshot { name, states } => {
                if name.trim().is_empty()
                    || name.len() > 128
                    || states.len() > 65_536
                    || states.values().any(|value| !value.is_finite())
                {
                    return Err("mix snapshot name or state is invalid".into());
                }
            }
            CommandAction::DiffMixSnapshots { .. }
            | CommandAction::ApplyMidiLogicalRule { .. }
            | CommandAction::CaptureMixSnapshot { .. }
            | CommandAction::RecallMixSnapshot { .. }
            | CommandAction::ApplyMixSnapshot { .. } => {}
            _ => {}
        }
    }
    if document.permission == Permission::ReadOnly
        && document.actions.iter().any(|action| {
            !matches!(
                action,
                CommandAction::ControlInspect
                    | CommandAction::InspectMidiNotes
                    | CommandAction::InspectChordTrack
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
                    | CommandAction::TrackFreezeStatus { .. }
                    | CommandAction::DiffMixSnapshots { .. }
                    | CommandAction::RecallMixSnapshot { .. }
            )
        })
    {
        return Err("read_only permission allows project inspection only".into());
    }
    let mutation_class = document
        .actions
        .iter()
        .map(mutation_class)
        .max_by_key(|class| match class {
            MutationClass::ReadOnly => 0,
            MutationClass::Reversible => 1,
            MutationClass::Irreversible => 2,
            MutationClass::ExternalSideEffect => 3,
        })
        .unwrap_or(MutationClass::ReadOnly);
    let destructive = document.actions.iter().any(is_destructive);
    if mutation_class == MutationClass::ExternalSideEffect
        && !matches!(
            document.permission,
            Permission::SystemWrite | Permission::Unrestricted
        )
    {
        return Err("external side effects require system_write permission".into());
    }
    Ok(ValidatedCommand {
        schema_version: document.schema_version,
        command_version: document.command_version,
        transaction,
        permission: document.permission,
        expected_generation: document.expected_generation,
        expected_audio_generation: document.expected_audio_generation,
        actions: document.actions,
        destructive,
        mutation_class,
    })
}

