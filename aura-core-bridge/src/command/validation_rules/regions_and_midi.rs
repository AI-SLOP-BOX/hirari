fn validate_regions_and_midi_action(action: &CommandAction) -> Result<(), String> {
    match action {
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
            _ => {}
    }
    Ok(())
}
