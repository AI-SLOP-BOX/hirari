fn validate_project_and_extensions_action(action: &CommandAction) -> Result<(), String> {
    match action {
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
    Ok(())
}
