fn validate_track_and_mixer_action(action: &CommandAction) -> Result<(), String> {
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
            _ => {}
    }
    Ok(())
}
