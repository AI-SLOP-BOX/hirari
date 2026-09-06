fn atomic_write_midi_sidecar(path: &Path, data: &[u8]) -> bool {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        if let Ok(directory) = fs::File::open(parent) {
            directory.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
        return false;
    }
    true
}

pub(crate) fn save_ui_midi_notes(project_path: &str, tracks: &slint::VecModel<Z_Track>) -> bool {
    let persisted: Vec<PersistedTrackNotes> = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .map(|track| PersistedTrackNotes {
            track_id: track.id,
            notes: track
                .piano_roll_notes
                .iter()
                .map(|note| PersistedNote {
                    pitch: note.pitch,
                    start_beat: note.start_beat,
                    length_beats: note.length_beats,
                    velocity: note.velocity,
                    articulation: note.articulation,
                    vibrato_amount: note.vibrato_amount,
                    vibrato_rate_millihz: note.vibrato_rate_millihz,
                    lyric: note.lyric.to_string(),
                })
                .collect(),
        })
        .collect();

    // Never erase a previously loaded arrangement just because the UI model
    // has not finished hydrating yet (this can happen after plugin scan,
    // device recovery, or a fast save immediately after launch).  An empty
    // in-memory snapshot is ambiguous; an existing non-empty sidecar is not.
    let current_note_count: usize = persisted.iter().map(|track| track.notes.len()).sum();
    if current_note_count == 0 {
        let existing_path = midi_notes_path(project_path);
        if let Ok(existing_data) = fs::read(&existing_path) {
            if let Ok(existing) = serde_json::from_slice::<Vec<PersistedTrackNotes>>(&existing_data)
            {
                let existing_note_count: usize =
                    existing.iter().map(|track| track.notes.len()).sum();
                if existing_note_count > 0 {
                    return true;
                }
            }
        }
    }
    let Ok(data) = serde_json::to_vec_pretty(&persisted) else {
        return false;
    };
    atomic_write_midi_sidecar(&midi_notes_path(project_path), &data)
}

pub(crate) fn load_ui_midi_notes(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &AuraCore,
) -> bool {
    let persisted = match fs::read(midi_notes_path(project_path))
        .ok()
        .and_then(|data| serde_json::from_slice::<Vec<PersistedTrackNotes>>(&data).ok())
    {
        Some(persisted) => persisted,
        None => {
            // Native project files already carry the canonical scheduled MIDI
            // model. The sidecar is retained only for older UI metadata; a
            // missing or malformed sidecar must never make an otherwise valid
            // project fail to open or silently erase its notes.
            crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
            return true;
        }
    };
    // Older projects and recovery snapshots can legitimately renumber runtime
    // track IDs while preserving their serialized order. Prefer the stable ID
    // when it matches, but fall back to the corresponding row when none of the
    // persisted IDs can be resolved. Otherwise a valid MIDI sidecar appears
    // empty after restore and never reaches the native MIDI snapshot.
    let has_resolved_track_id = tracks
        .iter()
        .any(|track| persisted.iter().any(|entry| entry.track_id == track.id));
    for row in 0..tracks.row_count() {
        let Some(mut track) = tracks.row_data(row) else {
            continue;
        };
        let saved = if has_resolved_track_id {
            persisted.iter().find(|entry| entry.track_id == track.id)
        } else {
            persisted.get(row)
        };
        let Some(saved) = saved else {
            continue;
        };
        let notes = saved
            .notes
            .iter()
            .map(|note| ZNote {
                pitch: note.pitch.clamp(0, 127),
                start_beat: note.start_beat.max(0.0),
                length_beats: note.length_beats.max(0.015625),
                velocity: note.velocity.clamp(1, 127),
                articulation: note.articulation,
                vibrato_amount: note.vibrato_amount,
                vibrato_rate_millihz: note.vibrato_rate_millihz,
                selected: false,
                lyric: note.lyric.clone().into(),
            })
            .collect::<Vec<_>>();
        track.piano_roll_notes = slint::ModelRc::new(slint::VecModel::from(notes));
        replace_track(tracks, row, track);
    }
    true
}

/// Hydrates every project-scoped UI model from one authoritative Core load.
/// Callers must treat `false` as a failed transaction and avoid publishing a
/// new project path or success status.
pub(crate) fn hydrate_project_models(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &aura_core_bridge::AuraCore,
) -> bool {
    // Project loading may publish the native layout one control-thread turn
    // after load_project() returns, especially when a sandbox plugin is being
    // restored. Poll the readiness condition instead of treating that narrow
    // window as a corrupt/empty project. This runs only on the control path;
    // the audio callback never waits here.
    let mut native_ready = false;
    let deadline = Instant::now() + std::time::Duration::from_millis(500);
    while Instant::now() < deadline {
        if sync_tracks_from_engine_allow_empty(tracks, core) {
            native_ready = true;
            break;
        }
        // Yield the control thread while waiting for the published native
        // snapshot.  Do not make project hydration depend on scheduler timing
        // or a fixed sleep interval.
        thread::yield_now();
    }
    native_ready && load_ui_midi_notes(project_path, tracks, core) && {
        sync_midi_notes_to_core(project_path, tracks, core);
        true
    }
}

pub(crate) use crate::ui::track_model::{
    fallback_template_tracks, sync_midi_notes_to_core, sync_tracks_from_engine,
    sync_tracks_from_engine_allow_empty,
};

pub use crate::ui::app::run;

#[cfg(test)]
mod tests {
    use super::{atomic_write_midi_sidecar, PersistedNote, UiSettings};
    use std::fs;

    #[test]
    fn midi_sidecar_publish_is_atomic_and_leaves_no_temp_file() {
        let root = std::env::temp_dir().join(format!(
            "aura-ui-sidecar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Song.aura.midi.json");
        fs::write(&path, br#"[{"old":true}]"#).unwrap();
        assert!(atomic_write_midi_sidecar(&path, br#"[{"new":true}]"#));
        assert_eq!(fs::read(&path).unwrap(), br#"[{"new":true}]"#);
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .count(),
            0
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ui_settings_keep_beginner_mode_backward_compatible() {
        let legacy: UiSettings = serde_json::from_str(r#"{"project_path":"/tmp/song.aura"}"#)
            .expect("legacy settings must remain readable");
        assert_eq!(legacy.project_path, "/tmp/song.aura");
        assert!(legacy.beginner_mode);
    }

    #[test]
    fn ui_settings_round_trip_preserves_mode_and_project_path() {
        let settings = UiSettings {
            project_path: "/tmp/song.aura".into(),
            audio_library_path: String::new(),
            beginner_mode: false,
            focus_mode: true,
            workspace_preset: "arrange".into(),
            plugin_favorites: Vec::new(),
            onboarding_completed: true,
            onboarding_step: 8,
        };
        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: UiSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.project_path, settings.project_path);
        assert!(!decoded.beginner_mode);
        assert!(decoded.focus_mode);
        assert!(decoded.onboarding_completed);
        assert_eq!(decoded.onboarding_step, 8);
    }

    #[test]
    fn midi_sidecar_rate_is_backward_compatible_and_roundtrips() {
        let legacy: PersistedNote = serde_json::from_str(
            r#"{"pitch":60,"start_beat":0.0,"length_beats":1.0,"velocity":100,"articulation":0,"lyric":"a"}"#,
        ).unwrap();
        assert_eq!(legacy.vibrato_rate_millihz, 5000);
        let current = PersistedNote {
            pitch: 60,
            start_beat: 0.0,
            length_beats: 1.0,
            velocity: 100,
            articulation: 0,
            vibrato_amount: 0.7,
            vibrato_rate_millihz: 8500,
            lyric: "a".into(),
        };
        let decoded: PersistedNote =
            serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        assert_eq!(decoded.vibrato_rate_millihz, 8500);
        assert!((decoded.vibrato_amount - 0.7).abs() < f32::EPSILON);
    }
}
