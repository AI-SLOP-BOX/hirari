use super::app_state::*;
use crate::ui::sync::replace_track;
use hirari_core_bridge::HirariCore;
use slint::Model;
use std::fs;
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(crate) fn snapshot_audio_input_assignments(
    tracks: &slint::VecModel<Z_Track>,
) -> Option<Vec<hirari_core_bridge::project::TrackAudioInputAssignment>> {
    let mut assignments = Vec::new();
    for track in tracks.iter() {
        if track.r#type != "Audio" && track.r#type != "Vocal" {
            continue;
        }
        let track_id = u32::try_from(track.id).ok()?;
        let channels =
            crate::ui::track_model::parse_recording_input_channels(track.input.as_str())?;
        assignments.push(hirari_core_bridge::project::TrackAudioInputAssignment {
            track_id,
            device_uid: (!track.input_endpoint_uid.is_empty())
                .then(|| track.input_endpoint_uid.to_string()),
            device_name: (!track.input_endpoint_name.is_empty())
                .then(|| track.input_endpoint_name.to_string()),
            channels,
        });
    }
    Some(assignments)
}

fn apply_audio_input_assignments(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    let Ok(document) = hirari_core_bridge::project::ProjectDocument::load(project_path) else {
        return false;
    };
    let endpoint_uid = document
        .audio_input_assignments
        .iter()
        .filter_map(|assignment| assignment.device_uid.as_deref())
        .next();
    if let Some(endpoint_uid) = endpoint_uid {
        let current_uid = core.audio_input_device_uid();
        if current_uid != endpoint_uid && !core.is_playing() && !core.recording_preview_active() {
            let matching_id =
                serde_json::from_str::<serde_json::Value>(&core.list_audio_devices_json())
                    .ok()
                    .and_then(|catalog| catalog.as_array().cloned())
                    .and_then(|devices| {
                        devices.into_iter().find(|device| {
                            device.get("uid").and_then(serde_json::Value::as_str)
                                == Some(endpoint_uid)
                                && device
                                    .get("input")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false)
                        })
                    })
                    .and_then(|device| device.get("id").and_then(serde_json::Value::as_u64))
                    .and_then(|id| u32::try_from(id).ok());
            if let Some(device_id) = matching_id {
                let sample_rate = match core.get_sample_rate().round() as u32 {
                    44_100 | 48_000 | 88_200 | 96_000 | 192_000 => {
                        core.get_sample_rate().round() as u32
                    }
                    _ => 48_000,
                };
                let buffer_size = match core.get_buffer_size() {
                    32 | 64 | 128 | 256 | 512 | 1024 | 2048 => core.get_buffer_size(),
                    _ => 256,
                };
                let _ = core.select_audio_device(device_id, sample_rate, buffer_size);
            }
        }
    }
    let assignments = document
        .audio_input_assignments
        .into_iter()
        .map(|assignment| {
            (
                assignment.track_id,
                (
                    assignment.channels,
                    assignment.device_uid,
                    assignment.device_name,
                ),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut routing_changed = false;
    for row in 0..tracks.row_count() {
        let Some(mut track) = tracks.row_data(row) else {
            continue;
        };
        if track.r#type != "Audio" && track.r#type != "Vocal" {
            continue;
        }
        let track_id = match u32::try_from(track.id) {
            Ok(track_id) => track_id,
            Err(_) => return false,
        };
        let (channels, endpoint_uid, endpoint_name) = assignments
            .get(&track_id)
            .map(|(channels, uid, name)| (channels.clone(), uid.clone(), name.clone()))
            .unwrap_or_else(|| (vec![0, 1], None, None));
        let Some(input) = crate::ui::track_model::format_recording_input_channels(&channels) else {
            return false;
        };
        let endpoint_uid = endpoint_uid.unwrap_or_else(|| core.audio_input_device_uid());
        let channel_names = core.audio_device_input_channel_names(&endpoint_uid);
        track.input_bus_name =
            crate::ui::track_model::recording_input_bus_label_with_names(&channels, &channel_names)
                .or_else(|| crate::ui::track_model::recording_input_bus_label(&channels))
                .unwrap_or_else(|| "Input".to_owned())
                .into();
        let endpoint_name = endpoint_name.unwrap_or_else(|| core.audio_input_device_name());
        routing_changed |= track.input.as_str() != input
            || track.input_first_channel != channels[0] as i32
            || track.input_endpoint_uid.as_str() != endpoint_uid
            || track.input_endpoint_name.as_str() != endpoint_name;
        track.input = input.into();
        track.input_first_channel = channels[0] as i32;
        track.input_endpoint_uid = endpoint_uid.into();
        track.input_endpoint_name = endpoint_name.into();
        tracks.set_row_data(row, track);
    }
    if routing_changed {
        crate::ui::project_state::mark_ui_routing_changed();
    }
    true
}

fn save_project_checkpoint_with_ui_state(
    project_path: &str,
    name: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    let Ok(mut document) = core.project_document_snapshot_v2(name, core.get_tempo()) else {
        return false;
    };
    let Some(assignments) = snapshot_audio_input_assignments(tracks) else {
        return false;
    };
    document.audio_input_assignments = assignments;
    HirariCore::finalize_project_document_snapshot_v2(project_path, &mut document).is_ok()
        && document.save_atomic(project_path).is_ok()
}

/// Offers explicit path repair before a project-load transaction begins.
/// Selected files are written to the project only after every missing
/// reference has been resolved, using the project's normal generation backup.
pub(crate) fn resolve_missing_project_media(project_path: &str) -> Result<Option<Vec<u8>>, String> {
    let Ok(mut document) = hirari_core_bridge::project::ProjectDocument::load(project_path) else {
        // Native/legacy formats use their own reader and do not expose typed
        // media references here.
        return Ok(None);
    };
    let missing = document
        .missing_media_references(project_path)
        .map_err(|error| format!("could not inspect project media: {error}"))?;
    if missing.is_empty() {
        return Ok(None);
    }
    let original_project_bytes = fs::read(project_path)
        .map_err(|error| format!("could not checkpoint project before relinking: {error}"))?;

    let project_parent = Path::new(project_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for reference in missing {
        let filename = Path::new(&reference)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("media"));
        let mut dialog = rfd::FileDialog::new()
            .set_title(format!(
                "Relink project media: {}",
                Path::new(&reference).display()
            ))
            .set_file_name(filename.to_string_lossy().into_owned());
        if project_parent.is_dir() {
            dialog = dialog.set_directory(project_parent);
        }
        let Some(replacement) = dialog.pick_file() else {
            return Err(format!("media relink cancelled: {reference}"));
        };
        document
            .relink_missing_media(
                project_path,
                &reference,
                replacement.to_string_lossy().as_ref(),
            )
            .map_err(|error| format!("could not relink {reference}: {error}"))?;
    }

    // save_atomic retains the previous project as a recovery generation.
    // Thus a crash after relinking still leaves both the repaired document
    // and the exact pre-repair version available.
    document
        .save_atomic(project_path)
        .map_err(|error| format!("could not save repaired media references: {error}"))?;
    Ok(Some(original_project_bytes))
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn serialize_ui_midi_notes(_tracks: &slint::VecModel<Z_Track>) -> Option<String> {
    // Legacy serializer retained for migration contract coverage. Current
    // saves persist notes in ProjectDocument and do not publish this payload.
    Some("[]".to_owned())
}

pub(crate) struct PreparedUiProjectSave {
    pub(crate) path: String,
    pub(crate) document: hirari_core_bridge::project::ProjectDocument,
    step_sequencer_patterns_json: String,
}

/// Captures UI-owned project state while `HirariCore` is still on the UI
/// thread. Project data is published as one canonical ProjectDocument.
pub(crate) fn prepare_ui_project_save(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
    step_sequencer_patterns_json: &str,
) -> Option<PreparedUiProjectSave> {
    if project_path.trim().is_empty() {
        return None;
    }
    let path = Path::new(project_path);
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Hirari Project");
    let document = core
        .project_document_snapshot_v2(name, core.get_tempo())
        .ok()?;
    prepare_ui_project_save_from_document(
        project_path,
        tracks,
        step_sequencer_patterns_json,
        document,
    )
}

pub(crate) fn prepare_ui_project_save_from_document(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    step_sequencer_patterns_json: &str,
    document: hirari_core_bridge::project::ProjectDocument,
) -> Option<PreparedUiProjectSave> {
    if project_path.trim().is_empty() {
        return None;
    }
    prepare_ui_project_save_from_captured_state(
        project_path,
        snapshot_audio_input_assignments(tracks)?,
        step_sequencer_patterns_json,
        document,
    )
}

pub(crate) fn prepare_ui_project_save_from_captured_state(
    project_path: &str,
    audio_input_assignments: Vec<hirari_core_bridge::project::TrackAudioInputAssignment>,
    step_sequencer_patterns_json: &str,
    mut document: hirari_core_bridge::project::ProjectDocument,
) -> Option<PreparedUiProjectSave> {
    if project_path.trim().is_empty() {
        return None;
    }
    document.audio_input_assignments = audio_input_assignments;
    Some(PreparedUiProjectSave {
        path: project_path.to_owned(),
        document,
        step_sequencer_patterns_json: step_sequencer_patterns_json.to_owned(),
    })
}

/// Publishes a prepared immutable session snapshot. This function touches only
/// owned Rust data and the filesystem, so it is safe to run on the save worker.
pub(crate) fn publish_prepared_ui_project_save(mut save: PreparedUiProjectSave) -> bool {
    if !attach_step_sequencer_patterns(&mut save) {
        return false;
    }
    if HirariCore::finalize_project_document_snapshot_v2(&save.path, &mut save.document).is_err() {
        return false;
    }
    save.document.save_atomic(&save.path).is_ok()
}

/// Parses and validates the potentially large UI-owned sequencer payload on
/// the bounded publication worker. Track/region filtering keeps patterns that
/// no longer refer to the captured project out of the canonical document.
pub(crate) fn attach_step_sequencer_patterns(save: &mut PreparedUiProjectSave) -> bool {
    let Ok(mut patterns) = serde_json::from_str::<
        Vec<hirari_core_bridge::project::StepSequencerPatternContract>,
    >(&save.step_sequencer_patterns_json) else {
        return false;
    };
    patterns.retain(|pattern| {
        save.document
            .regions
            .iter()
            .any(|region| region.id == pattern.region_id && region.track_id == pattern.track_id)
            && save.document.tracks.iter().any(|track| {
                track.id == pattern.track_id
                    && matches!(track.track_type.as_str(), "Midi" | "MIDI" | "Instrument")
            })
    });
    save.document.step_sequencer_patterns = patterns;
    true
}

pub(crate) fn load_ui_midi_notes(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    // Once the canonical document explicitly contains `midi_notes`, an empty
    // array means the user cleared the notes. Do not let a leftover migration
    // sidecar resurrect them. Older documents and native legacy projects
    // still use the sidecar migration below.
    if project_has_canonical_midi_note_field(project_path) {
        if !core.midi_notes_snapshot().is_empty()
            && project_needs_legacy_vibrato_rate_migration(project_path)
        {
            let sidecar = midi_notes_path(project_path);
            if let Ok(bytes) = fs::read(sidecar) {
                if let Ok(persisted) = serde_json::from_slice::<Vec<PersistedTrackNotes>>(&bytes) {
                    apply_legacy_vibrato_rates(persisted, tracks, core);
                    return true;
                }
            }
        }
        crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
        return true;
    }
    load_ui_midi_notes_from_path(&midi_notes_path(project_path), tracks, core)
}

fn project_has_canonical_midi_note_field(project_path: &str) -> bool {
    fs::read(project_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .as_object()
                .map(|object| object.contains_key("midi_notes"))
        })
        .unwrap_or(false)
}

fn project_needs_legacy_vibrato_rate_migration(project_path: &str) -> bool {
    fs::read(project_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("midi_notes")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .is_some_and(|notes| {
            notes.iter().any(|note| {
                !note
                    .as_object()
                    .is_some_and(|object| object.contains_key("vibrato_rate_millihz"))
            })
        })
}

fn load_ui_midi_notes_from_path(
    sidecar_path: &Path,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    let persisted = match fs::read(sidecar_path)
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
    if !core.midi_notes_snapshot().is_empty() {
        // The typed ProjectDocument now contains the scheduled notes and
        // their authoring state. Older UI sidecars may restore only the
        // vibrato-rate value that predates the shared contract; never let
        // their stale note rows replace newer Core notes.
        apply_legacy_vibrato_rates(persisted, tracks, core);
        return true;
    }
    // Legacy projects can have piano-roll notes only in the UI sidecar.
    // Import those once, then the caller synchronizes them into Core.
    apply_persisted_ui_midi_notes(persisted, tracks)
}

fn apply_legacy_vibrato_rates(
    persisted: Vec<PersistedTrackNotes>,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) {
    crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
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
        let mut notes: Vec<ZNote> = track.piano_roll_notes.iter().collect();
        for note in &mut notes {
            if let Some(saved_note) = saved.notes.iter().find(|saved_note| {
                saved_note.pitch.clamp(0, 127) == note.pitch
                    && (saved_note.start_beat - note.start_beat).abs() <= 0.00001
                    && (saved_note.length_beats - note.length_beats).abs() <= 0.00001
            }) {
                note.vibrato_rate_millihz = saved_note.vibrato_rate_millihz.clamp(500, 20_000);
            }
        }
        track.piano_roll_notes = slint::ModelRc::new(slint::VecModel::from(notes));
        crate::ui::sync::replace_track(tracks, row, track);
    }
}

fn apply_persisted_ui_midi_notes(
    persisted: Vec<PersistedTrackNotes>,
    tracks: &slint::VecModel<Z_Track>,
) -> bool {
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
                region_id: note.region_id.max(0),
                midi_channel: i32::from(note.midi_channel.min(15)),
                pitch: note.pitch.clamp(0, 127),
                start_beat: note.start_beat.max(0.0),
                length_beats: note.length_beats.max(0.015625),
                velocity: note.velocity.clamp(1, 127),
                articulation: note.articulation,
                vibrato_amount: note.vibrato_amount,
                vibrato_rate_millihz: note.vibrato_rate_millihz,
                phoneme: note.phoneme.clone().into(),
                pitch_curve_cents: slint::ModelRc::new(slint::VecModel::from(
                    note.pitch_curve_cents
                        .iter()
                        .map(|value| (*value).clamp(i16::MIN as i32, i16::MAX as i32))
                        .collect::<Vec<_>>(),
                )),
                portamento_samples: note.portamento_samples.min(i32::MAX as u32) as i32,
                probability: note.probability.clamp(0, 100),
                repeat_count: note.repeat_count.max(1),
                selected: false,
                lyric: note.lyric.clone().into(),
            })
            .collect::<Vec<_>>();
        track.piano_roll_notes = slint::ModelRc::new(slint::VecModel::from(notes));
        replace_track(tracks, row, track);
    }
    true
}

fn load_ui_midi_backup_notes(
    project_path: &str,
    generation: u32,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> bool {
    let load_matching_legacy_sidecar = || -> Option<Vec<PersistedTrackNotes>> {
        let project_backup = PathBuf::from(format!("{project_path}.bak.{generation}"));
        let sidecar_backup = PathBuf::from(format!("{project_path}.midi.json.bak.{generation}"));
        let project_bytes = fs::read(&project_backup).ok()?;
        let project_metadata = fs::metadata(&project_backup).ok()?;
        let project_modified_ns = project_metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let project_checksum =
            hirari_core_bridge::persistence::PersistenceOrchestrator::calculate_checksum(
                &project_bytes,
            );
        let bytes = fs::read(sidecar_backup).ok()?;
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
        let saved_checksum = value
            .get("hirari_project_checksum")
            .and_then(serde_json::Value::as_u64)?;
        let saved_modified_ns = value
            .get("hirari_project_modified_ns")
            .and_then(serde_json::Value::as_str)?
            .parse::<u128>()
            .ok()?;
        if saved_checksum != project_checksum || saved_modified_ns != project_modified_ns {
            return None;
        }
        serde_json::from_value(value.get("notes")?.clone()).ok()
    };
    let Some(persisted) = load_matching_legacy_sidecar() else {
        // The Core project is independently complete. A missing, old, or
        // mismatched UI sidecar must not make an otherwise valid recovery
        // generation unloadable.
        crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
        return true;
    };
    if !core.midi_notes_snapshot().is_empty() {
        apply_legacy_vibrato_rates(persisted, tracks, core);
        true
    } else {
        apply_persisted_ui_midi_notes(persisted, tracks)
    }
}

/// Hydrates every project-scoped UI model from one authoritative Core load.
/// Callers must treat `false` as a failed transaction and avoid publishing a
/// new project path or success status.
pub(crate) fn hydrate_project_models(
    project_path: &str,
    tracks: &slint::VecModel<Z_Track>,
    core: &hirari_core_bridge::HirariCore,
) -> bool {
    hydrate_project_models_with_notes(tracks, core, |tracks, core| {
        apply_audio_input_assignments(project_path, tracks, core)
            && load_ui_midi_notes(project_path, tracks, core)
            && sync_midi_notes_to_core(project_path, tracks, core)
    })
}

/// Hydrates a selected recovery generation. Core notes are authoritative;
/// only a verified matching UI sidecar can migrate legacy UI-only values.
/// Never overlay the current project's sidecar onto an older native snapshot.
pub(crate) fn hydrate_project_backup_models(
    project_path: &str,
    generation: u32,
    tracks: &slint::VecModel<Z_Track>,
    core: &hirari_core_bridge::HirariCore,
) -> bool {
    hydrate_project_models_with_notes(tracks, core, |tracks, core| {
        let backup_path = format!("{project_path}.bak.{generation}");
        apply_audio_input_assignments(&backup_path, tracks, core)
            && load_ui_midi_backup_notes(project_path, generation, tracks, core)
            && sync_midi_notes_to_core(project_path, tracks, core)
    })
}

fn hydrate_project_models_with_notes<F>(
    tracks: &slint::VecModel<Z_Track>,
    core: &hirari_core_bridge::HirariCore,
    hydrate_notes: F,
) -> bool
where
    F: FnOnce(&slint::VecModel<Z_Track>, &HirariCore) -> bool,
{
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
    if !native_ready {
        return false;
    }
    crate::ui::track_model::reset_project_scoped_track_overlays(tracks);
    // The first successful native sync already published the new track rows.
    // Clear same-ID overlays in place, then seed canonical notes before
    // applying any legacy sidecar metadata.
    crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
    hydrate_notes(tracks, core)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectLoadTransactionError {
    CheckpointFailed,
    MediaRelinkFailed,
    LoadFailed,
    HydrationFailed,
    RollbackFailed,
}

struct ProjectLoadCheckpoint(PathBuf);

impl Drop for ProjectLoadCheckpoint {
    fn drop(&mut self) {
        let base = self.0.to_string_lossy();
        for suffix in [
            "",
            ".midi.json",
            ".midi-events.json",
            ".comping.json",
            ".control-room.json",
        ] {
            let _ = fs::remove_file(format!("{base}{suffix}"));
        }
    }
}

/// Retained UI-thread rollback state for a Recover As save whose filesystem
/// publication runs asynchronously. If the worker fails before the user edits
/// the recovered snapshot, the original Core state and UI-owned notes can be
/// restored without leaving the recovery transaction half committed.
pub(crate) struct PendingRecoveryRollback {
    checkpoint: ProjectLoadCheckpoint,
    original_tracks: Vec<Z_Track>,
}

impl PendingRecoveryRollback {
    pub(crate) fn restore(
        self,
        tracks: &slint::VecModel<Z_Track>,
        core: &HirariCore,
    ) -> Result<(), ProjectLoadTransactionError> {
        let checkpoint_path = self.checkpoint.0.to_string_lossy();
        let staged_tracks = slint::VecModel::from(self.original_tracks.clone());
        if core.load_project_v2(&checkpoint_path).is_ok()
            && hydrate_project_models(&checkpoint_path, &staged_tracks, core)
        {
            restore_project_load_ui_snapshot(&staged_tracks, &self.original_tracks);
            tracks.set_vec(staged_tracks.iter().collect::<Vec<_>>());
            return Ok(());
        }
        let _ = crate::ui::track_model::sync_tracks_from_engine_allow_empty(tracks, core);
        crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
        Err(ProjectLoadTransactionError::RollbackFailed)
    }
}

/// Loads one backup generation while retaining the pre-load checkpoint until
/// the caller has published the recovered copy. Unlike the synchronous
/// transaction helper, success returns a rollback token for the pending disk
/// commit rather than deleting the checkpoint immediately.
pub(crate) fn begin_recovery_as_transactionally(
    source_path: &str,
    generation: u32,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> Result<PendingRecoveryRollback, ProjectLoadTransactionError> {
    let original_tracks = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .collect::<Vec<_>>();
    let checkpoint = ProjectLoadCheckpoint(project_load_checkpoint_path(Some(source_path)));
    let checkpoint_path = checkpoint.0.to_string_lossy();
    if !save_project_checkpoint_with_ui_state(
        &checkpoint_path,
        "Recover As Checkpoint",
        tracks,
        core,
    ) {
        return Err(ProjectLoadTransactionError::CheckpointFailed);
    }
    let rollback = PendingRecoveryRollback {
        checkpoint,
        original_tracks,
    };
    let staged_tracks = slint::VecModel::from(rollback.original_tracks.clone());
    if core.restore_project_backup(source_path, generation)
        && hydrate_project_backup_models(source_path, generation, &staged_tracks, core)
    {
        tracks.set_vec(staged_tracks.iter().collect::<Vec<_>>());
        return Ok(rollback);
    }

    let restored = rollback.restore(tracks, core).is_ok();
    Err(if !restored {
        ProjectLoadTransactionError::RollbackFailed
    } else {
        ProjectLoadTransactionError::HydrationFailed
    })
}

fn project_load_checkpoint_path(current_project_path: Option<&str>) -> PathBuf {
    static NEXT_CHECKPOINT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let parent = current_project_path
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| {
            current_project_path
                .filter(|path| !path.trim().is_empty())
                .and_then(|_| std::env::current_dir().ok())
        })
        .unwrap_or_else(std::env::temp_dir);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_CHECKPOINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    parent.join(format!(
        ".hirari-load-rollback-{}-{timestamp}-{sequence}.hirari",
        std::process::id()
    ))
}

fn restore_project_load_ui_snapshot(tracks: &slint::VecModel<Z_Track>, snapshot: &[Z_Track]) {
    // The Core checkpoint is authoritative for track structure and runtime
    // IDs. Keep those restored IDs while returning UI-owned state (currently
    // UI-owned input routes, EQ overlays, and transient clip/note selection
    // to their pre-load values.
    for row in 0..tracks.row_count() {
        let Some(mut restored) = tracks.row_data(row) else {
            continue;
        };
        let previous = snapshot
            .iter()
            .find(|track| track.id == restored.id)
            .or_else(|| snapshot.get(row));
        let Some(previous) = previous else {
            continue;
        };
        restored.eq_low_band = previous.eq_low_band;
        restored.eq_low_cut = previous.eq_low_cut;
        restored.eq_high_band = previous.eq_high_band;
        restored.eq_high_cut = previous.eq_high_cut;
        restored.input = previous.input.clone();
        restored.piano_roll_notes = previous.piano_roll_notes.clone();
        restored.clips = previous.clips.clone();
        tracks.set_row_data(row, restored);
    }
}

/// Applies a project load and its UI hydration as one user-visible transaction.
/// A v2 checkpoint preserves the active native graph and Core-owned metadata;
/// the in-memory UI snapshot preserves transient overlays and selections.
pub(crate) fn load_project_transactionally<F>(
    target_path: &str,
    current_project_path: Option<&str>,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
    load_candidate: F,
) -> Result<(), ProjectLoadTransactionError>
where
    F: FnOnce(&HirariCore) -> bool,
{
    let pre_relink_project = resolve_missing_project_media(target_path)
        .map_err(|_| ProjectLoadTransactionError::MediaRelinkFailed)?;
    let result = load_project_transactionally_with_hydrator(
        target_path,
        current_project_path,
        tracks,
        core,
        load_candidate,
        hydrate_project_models,
    );
    if result.is_err() {
        if let Some(original_bytes) = pre_relink_project {
            let mut persistence = hirari_core_bridge::persistence::PersistenceOrchestrator::new(10);
            if persistence
                .atomic_save(target_path, &original_bytes)
                .is_err()
            {
                return Err(ProjectLoadTransactionError::RollbackFailed);
            }
        }
    }
    result
}

pub(crate) fn load_project_transactionally_with_hydrator<F, H>(
    target_path: &str,
    current_project_path: Option<&str>,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
    load_candidate: F,
    hydrate: H,
) -> Result<(), ProjectLoadTransactionError>
where
    F: FnOnce(&HirariCore) -> bool,
    H: Fn(&str, &slint::VecModel<Z_Track>, &HirariCore) -> bool,
{
    load_project_transactionally_with_rollback(
        target_path,
        current_project_path,
        tracks,
        core,
        load_candidate,
        hydrate,
        |checkpoint_path, core| core.load_project_v2(checkpoint_path).is_ok(),
    )
}

/// Tries recovery generations newest-first and publishes only the first one
/// whose Core load and matching UI hydration both succeed. Each failed
/// candidate is rolled back by the transaction helper before the next one is
/// attempted.
pub(crate) fn restore_latest_project_backup_transactionally(
    project_path: &str,
    current_project_path: Option<&str>,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
) -> Result<u32, ProjectLoadTransactionError> {
    let candidates: serde_json::Value =
        serde_json::from_str(&core.recovery_candidates_json(project_path))
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    let mut generations = candidates
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            candidate
                .get("generation")
                .and_then(serde_json::Value::as_u64)
        })
        .filter_map(|generation| u32::try_from(generation).ok())
        .collect::<Vec<_>>();
    generations.sort_unstable();
    generations.dedup();
    if generations.is_empty() {
        return Err(ProjectLoadTransactionError::LoadFailed);
    }

    let mut hydration_failed = false;
    for generation in generations {
        let result = load_project_transactionally_with_hydrator(
            project_path,
            current_project_path,
            tracks,
            core,
            |core| core.restore_project_backup(project_path, generation),
            |hydration_path, tracks, core| {
                if hydration_path == project_path {
                    hydrate_project_backup_models(project_path, generation, tracks, core)
                } else {
                    hydrate_project_models(hydration_path, tracks, core)
                }
            },
        );
        match result {
            Ok(()) => return Ok(generation),
            Err(error @ ProjectLoadTransactionError::CheckpointFailed)
            | Err(error @ ProjectLoadTransactionError::MediaRelinkFailed)
            | Err(error @ ProjectLoadTransactionError::RollbackFailed) => return Err(error),
            Err(ProjectLoadTransactionError::HydrationFailed) => hydration_failed = true,
            Err(ProjectLoadTransactionError::LoadFailed) => {}
        }
    }
    Err(if hydration_failed {
        ProjectLoadTransactionError::HydrationFailed
    } else {
        ProjectLoadTransactionError::LoadFailed
    })
}

fn load_project_transactionally_with_rollback<F, H, R>(
    target_path: &str,
    current_project_path: Option<&str>,
    tracks: &slint::VecModel<Z_Track>,
    core: &HirariCore,
    load_candidate: F,
    hydrate: H,
    restore_checkpoint: R,
) -> Result<(), ProjectLoadTransactionError>
where
    F: FnOnce(&HirariCore) -> bool,
    H: Fn(&str, &slint::VecModel<Z_Track>, &HirariCore) -> bool,
    R: Fn(&str, &HirariCore) -> bool,
{
    let original_tracks = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .collect::<Vec<_>>();
    let checkpoint_path = project_load_checkpoint_path(current_project_path);
    let checkpoint = ProjectLoadCheckpoint(checkpoint_path);
    let checkpoint_text = checkpoint.0.to_string_lossy();
    if !save_project_checkpoint_with_ui_state(
        &checkpoint_text,
        "Project Load Checkpoint",
        tracks,
        core,
    ) {
        return Err(ProjectLoadTransactionError::CheckpointFailed);
    }

    // Hydrate into a private model. Core may switch projects before hydration
    // completes, but the visible arrangement remains unchanged until both
    // halves of the transaction have succeeded.
    let staged_tracks = slint::VecModel::from(original_tracks.clone());
    let loaded = load_candidate(core);
    if loaded && hydrate(target_path, &staged_tracks, core) {
        tracks.set_vec(staged_tracks.iter().collect::<Vec<_>>());
        return Ok(());
    }

    let original_state_restored = restore_checkpoint(&checkpoint_text, core)
        && hydrate(&checkpoint_text, &staged_tracks, core);
    if !original_state_restored {
        // The transaction cannot promise the old session after this point.
        // Best-effort resync makes the visible arrangement reflect whatever
        // Core currently owns instead of leaving a partially hydrated model.
        let _ = crate::ui::track_model::sync_tracks_from_engine_allow_empty(tracks, core);
        crate::ui::track_model::sync_midi_notes_from_core(tracks, core);
        return Err(ProjectLoadTransactionError::RollbackFailed);
    }
    restore_project_load_ui_snapshot(&staged_tracks, &original_tracks);
    tracks.set_vec(staged_tracks.iter().collect::<Vec<_>>());
    Err(if loaded {
        ProjectLoadTransactionError::HydrationFailed
    } else {
        ProjectLoadTransactionError::LoadFailed
    })
}

pub(crate) use crate::ui::track_model::{
    fallback_template_tracks, sync_midi_notes_to_core, sync_tracks_from_engine,
    sync_tracks_from_engine_allow_empty,
};

pub use crate::ui::app::run;

#[cfg(test)]
mod tests {
    use super::{
        atomic_write_midi_sidecar, hydrate_project_models,
        load_project_transactionally_with_hydrator, load_project_transactionally_with_rollback,
        restore_latest_project_backup_transactionally, PersistedNote, ProjectLoadTransactionError,
        UiSettings,
    };
    use slint::Model;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn midi_sidecar_publish_is_atomic_and_leaves_no_temp_file() {
        let root = std::env::temp_dir().join(format!(
            "hirari-ui-sidecar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Song.hirari.midi.json");
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
        let legacy: UiSettings = serde_json::from_str(r#"{"project_path":"/tmp/song.hirari"}"#)
            .expect("legacy settings must remain readable");
        assert_eq!(legacy.project_path, "/tmp/song.hirari");
        assert!(legacy.beginner_mode);
        assert!(!legacy.focus_mode);
        assert_eq!(legacy.workspace_preset, "arrange");
        assert!(legacy.plugin_favorites.is_empty());
        assert!(!legacy.onboarding_completed);
        assert_eq!(legacy.onboarding_step, 0);
    }

    #[test]
    fn ui_settings_round_trip_preserves_mode_and_project_path() {
        let settings = UiSettings {
            project_path: "/tmp/song.hirari".into(),
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
            midi_channel: 0,
            start_beat: 0.0,
            length_beats: 1.0,
            velocity: 100,
            articulation: 0,
            vibrato_amount: 0.7,
            vibrato_rate_millihz: 8500,
            phoneme: "k a".into(),
            pitch_curve_cents: vec![-20, 0, 35],
            portamento_samples: 96,
            probability: 85,
            repeat_count: 2,
            lyric: "a".into(),
        };
        let decoded: PersistedNote =
            serde_json::from_str(&serde_json::to_string(&current).unwrap()).unwrap();
        assert_eq!(decoded.vibrato_rate_millihz, 8500);
        assert!((decoded.vibrato_amount - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn core_note_contract_wins_over_stale_sidecar_rows() {
        use slint::Model;

        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let first_track = core.add_midi_track();
        let second_track = core.add_midi_track();
        assert_ne!(first_track, 0);
        assert_ne!(second_track, 0);
        assert!(core.set_midi_note_lyric(first_track, 60, 100, 0, 480, "native-a"));
        assert!(core.set_midi_note_lyric(second_track, 67, 100, 0, 480, "native-b"));

        let model = slint::VecModel::from(crate::ui::track_model::fallback_template_tracks(&[
            ("MIDI 1", 1),
            ("MIDI 2", 1),
        ]));
        for (row, track_id) in [first_track, second_track].into_iter().enumerate() {
            let mut track = model.row_data(row).expect("template row exists");
            track.id = track_id as i32;
            model.set_row_data(row, track);
        }
        crate::ui::track_model::sync_midi_notes_from_core(&model, &core);

        let project_path = std::env::temp_dir().join(format!(
            "hirari-partial-midi-sidecar-{}-{}.hirari",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sidecar_path =
            std::path::PathBuf::from(format!("{}.midi.json", project_path.display()));
        fs::write(
            &sidecar_path,
            format!(
                r#"[{{"track_id":{},"notes":[{{"pitch":62,"start_beat":0.0,"length_beats":1.0,"velocity":90,"articulation":0,"lyric":"legacy"}}]}}]"#,
                first_track
            ),
        )
        .unwrap();

        assert!(super::load_ui_midi_notes(
            project_path.to_str().unwrap(),
            &model,
            &core
        ));
        let first_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        let second_notes: Vec<_> = model.row_data(1).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(first_notes.len(), 1);
        assert_eq!(first_notes[0].pitch, 60);
        assert_eq!(first_notes[0].lyric.as_str(), "native-a");
        assert_eq!(second_notes.len(), 1);
        assert_eq!(second_notes[0].pitch, 67);
        assert_eq!(second_notes[0].lyric.as_str(), "native-b");

        fs::remove_file(sidecar_path).expect("sidecar cleanup");
    }

    #[test]
    fn recovery_uses_core_notes_when_ui_sidecar_generation_mismatches() {
        let root = std::env::temp_dir().join(format!(
            "hirari-recovery-midi-generation-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let project_path = root.join("Song.hirari");
        let project_text = project_path.to_string_lossy().into_owned();
        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let track_id = core.add_midi_track();
        assert!(core.set_track_name(track_id, "Piano"));
        assert!(core.set_midi_note_lyric(track_id, 60, 100, 0, 24_000, "first generation"));
        let model = slint::VecModel::from(crate::ui::track_model::fallback_template_tracks(&[(
            "Piano", 1,
        )]));
        let mut track = model.row_data(0).expect("template row exists");
        track.id = track_id as i32;
        model.set_row_data(0, track);
        crate::ui::track_model::sync_midi_notes_from_core(&model, &core);
        let first_sidecar = super::serialize_ui_midi_notes(&model).unwrap();
        assert!(core.save_project_with_ui_midi_notes(&project_text, &first_sidecar));

        let mut next_generation = model.row_data(0).unwrap();
        next_generation.piano_roll_notes =
            slint::ModelRc::new(slint::VecModel::from(vec![crate::slint_ui::ZNote {
                region_id: 0,
                midi_channel: 0,
                pitch: 67,
                start_beat: 1.0,
                length_beats: 1.0,
                velocity: 96,
                articulation: 0,
                vibrato_amount: 0.0,
                vibrato_rate_millihz: 5_000,
                phoneme: "".into(),
                pitch_curve_cents: slint::ModelRc::default(),
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
                selected: false,
                lyric: "second generation".into(),
            }]));
        model.set_row_data(0, next_generation);
        assert!(crate::ui::track_model::sync_midi_notes_to_core(
            "", &model, &core
        ));
        let second_sidecar = super::serialize_ui_midi_notes(&model).unwrap();
        assert!(core.save_project_with_ui_midi_notes(&project_text, &second_sidecar));

        let mut third_generation = model.row_data(0).unwrap();
        third_generation.piano_roll_notes =
            slint::ModelRc::new(slint::VecModel::from(vec![crate::slint_ui::ZNote {
                region_id: 0,
                midi_channel: 0,
                pitch: 74,
                start_beat: 2.0,
                length_beats: 1.0,
                velocity: 96,
                articulation: 0,
                vibrato_amount: 0.0,
                vibrato_rate_millihz: 5_000,
                phoneme: "".into(),
                pitch_curve_cents: slint::ModelRc::default(),
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
                selected: false,
                lyric: "third generation".into(),
            }]));
        model.set_row_data(0, third_generation);
        assert!(crate::ui::track_model::sync_midi_notes_to_core(
            "", &model, &core
        ));
        let third_sidecar = super::serialize_ui_midi_notes(&model).unwrap();
        assert!(core.save_project_with_ui_midi_notes(&project_text, &third_sidecar));

        // Simulate an interrupted UI-sidecar rotation by pairing generation 1
        // with the UI sidecar tagged for generation 2.
        let ui_generation_one = PathBuf::from(format!("{project_text}.midi.json.bak.1"));
        let generation_one_ui = fs::read(&ui_generation_one).unwrap();
        let generation_two_ui = fs::read(format!("{project_text}.midi.json.bak.2")).unwrap();
        fs::write(&ui_generation_one, generation_two_ui).unwrap();
        let result = load_project_transactionally_with_hydrator(
            &project_text,
            Some(&project_text),
            &model,
            &core,
            |core| core.restore_project_backup(&project_text, 1),
            |hydration_path, tracks, core| {
                if hydration_path == project_text {
                    super::hydrate_project_backup_models(&project_text, 1, tracks, core)
                } else {
                    hydrate_project_models(hydration_path, tracks, core)
                }
            },
        );
        assert_eq!(result, Ok(()));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes[0].pitch, 67);
        assert_eq!(recovered_notes[0].lyric.as_str(), "second generation");
        fs::write(&ui_generation_one, &generation_one_ui).unwrap();

        let result = load_project_transactionally_with_hydrator(
            &project_text,
            Some(&project_text),
            &model,
            &core,
            |core| core.restore_project_backup(&project_text, 1),
            |hydration_path, tracks, core| {
                if hydration_path == project_text {
                    super::hydrate_project_backup_models(&project_text, 1, tracks, core)
                } else {
                    hydrate_project_models(hydration_path, tracks, core)
                }
            },
        );

        assert_eq!(result, Ok(()));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes.len(), 1);
        assert_eq!(recovered_notes[0].pitch, 67);
        assert_eq!(recovered_notes[0].lyric.as_str(), "second generation");

        let result = load_project_transactionally_with_hydrator(
            &project_text,
            Some(&project_text),
            &model,
            &core,
            |core| core.restore_project_backup(&project_text, 2),
            |hydration_path, tracks, core| {
                if hydration_path == project_text {
                    super::hydrate_project_backup_models(&project_text, 2, tracks, core)
                } else {
                    hydrate_project_models(hydration_path, tracks, core)
                }
            },
        );
        assert_eq!(result, Ok(()));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes.len(), 1);
        assert_eq!(recovered_notes[0].pitch, 60);
        assert_eq!(recovered_notes[0].lyric.as_str(), "first generation");

        fs::remove_file(&ui_generation_one).expect("generation sidecar removal");
        let result = load_project_transactionally_with_hydrator(
            &project_text,
            Some(&project_text),
            &model,
            &core,
            |core| core.restore_project_backup(&project_text, 1),
            |hydration_path, tracks, core| {
                if hydration_path == project_text {
                    super::hydrate_project_backup_models(&project_text, 1, tracks, core)
                } else {
                    hydrate_project_models(hydration_path, tracks, core)
                }
            },
        );
        assert_eq!(result, Ok(()));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes.len(), 1);
        assert_eq!(recovered_notes[0].pitch, 67);
        assert_eq!(recovered_notes[0].lyric.as_str(), "second generation");

        let recovered_generation = restore_latest_project_backup_transactionally(
            &project_text,
            Some(&project_text),
            &model,
            &core,
        );
        assert_eq!(recovered_generation, Ok(2));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes[0].pitch, 60);
        fs::write(&ui_generation_one, generation_one_ui).unwrap();

        fs::write(format!("{project_text}.bak.1"), b"corrupt backup").unwrap();
        let recovered_generation = restore_latest_project_backup_transactionally(
            &project_text,
            Some(&project_text),
            &model,
            &core,
        );
        assert_eq!(recovered_generation, Ok(2));
        let recovered_notes: Vec<_> = model.row_data(0).unwrap().piano_roll_notes.iter().collect();
        assert_eq!(recovered_notes[0].pitch, 60);
        fs::remove_dir_all(root).expect("recovery fixture cleanup");
    }

    #[test]
    fn failed_ui_hydration_restores_the_previous_core_and_track_model() {
        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let old_track = core.add_midi_track();
        assert!(core.set_track_name(old_track, "Original Session"));
        assert!(core.set_midi_note_lyric(old_track, 60, 100, 0, 480, "original"));
        let model = slint::VecModel::<crate::slint_ui::Z_Track>::from(Vec::new());
        assert!(crate::ui::track_model::sync_tracks_from_engine(
            &model, &core
        ));
        crate::ui::track_model::sync_midi_notes_from_core(&model, &core);
        let mut original_row = model.row_data(0).expect("original track row exists");
        original_row.eq_low_band = 1.25;
        model.set_row_data(0, original_row);

        let candidate =
            hirari_core_bridge::HirariCore::new_offline().expect("candidate core initializes");
        let candidate_track = candidate.add_midi_track();
        assert!(candidate.set_track_name(candidate_track, "Candidate Session"));
        assert!(candidate.set_midi_note_lyric(candidate_track, 72, 100, 0, 480, "candidate"));
        let target = std::env::temp_dir().join(format!(
            "hirari-hydration-rollback-{}-{}.hirari",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target_text = target.to_string_lossy().into_owned();
        candidate
            .save_project_v2(&target_text, "Candidate Session", 120.0)
            .expect("candidate project saves");

        let result = load_project_transactionally_with_hydrator(
            &target_text,
            None,
            &model,
            &core,
            |core| core.load_project_v2(&target_text).is_ok(),
            |path, tracks, core| {
                if path == target_text {
                    let _ =
                        crate::ui::track_model::sync_tracks_from_engine_allow_empty(tracks, core);
                    false
                } else {
                    hydrate_project_models(path, tracks, core)
                }
            },
        );

        assert_eq!(result, Err(ProjectLoadTransactionError::HydrationFailed));
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).unwrap();
        assert_eq!(layout[0]["name"], "Original Session");
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["lyric"], "original", "restored notes: {notes}");
        let row = model.row_data(0).expect("previous UI row is restored");
        assert_eq!(row.name.as_str(), "Original Session");
        assert_eq!(row.eq_low_band, 1.25);
        let note = row
            .piano_roll_notes
            .row_data(0)
            .expect("previous note restores");
        assert_eq!(note.lyric.as_str(), "original");

        for suffix in ["", ".control-room.json", ".midi.json"] {
            let _ = fs::remove_file(format!("{target_text}{suffix}"));
        }
    }

    #[test]
    fn checkpoint_write_io_failure_prevents_candidate_load() {
        use std::cell::Cell;

        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let track_id = core.add_midi_track();
        assert!(core.set_track_name(track_id, "Original Session"));
        let tracks = slint::VecModel::<crate::slint_ui::Z_Track>::from(Vec::new());
        assert!(crate::ui::track_model::sync_tracks_from_engine(
            &tracks, &core
        ));
        let blocking_parent = std::env::temp_dir().join(format!(
            "hirari-missing-checkpoint-parent-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&blocking_parent, b"not a directory")
            .expect("fault injection must create a file at the parent path");
        let current_path = blocking_parent.join("Original.hirari");
        let candidate_load_called = Cell::new(false);

        let result = load_project_transactionally_with_hydrator(
            "Candidate.hirari",
            current_path.to_str(),
            &tracks,
            &core,
            |_core| {
                candidate_load_called.set(true);
                true
            },
            |_path, _tracks, _core| true,
        );

        assert_eq!(result, Err(ProjectLoadTransactionError::CheckpointFailed));
        assert!(!candidate_load_called.get());
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).unwrap();
        assert_eq!(layout[0]["name"], "Original Session");
        fs::remove_file(blocking_parent).expect("fault injection file cleanup");
    }

    #[test]
    fn failed_rollback_hydration_is_reported_to_the_caller() {
        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let track_id = core.add_midi_track();
        assert!(core.set_track_name(track_id, "Original Session"));
        let tracks = slint::VecModel::<crate::slint_ui::Z_Track>::from(Vec::new());
        assert!(crate::ui::track_model::sync_tracks_from_engine(
            &tracks, &core
        ));

        let target = std::env::temp_dir().join(format!(
            "hirari-rollback-hydration-failure-{}-{}.hirari",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target_text = target.to_string_lossy().into_owned();
        let result = load_project_transactionally_with_hydrator(
            &target_text,
            None,
            &tracks,
            &core,
            |core| core.set_track_name(track_id, "Candidate Session"),
            |_path, tracks, core| {
                let _ = crate::ui::track_model::sync_tracks_from_engine_allow_empty(tracks, core);
                false
            },
        );

        assert_eq!(result, Err(ProjectLoadTransactionError::RollbackFailed));
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).unwrap();
        assert_eq!(layout[0]["name"], "Original Session");
        assert_eq!(
            tracks.row_data(0).unwrap().name.as_str(),
            "Original Session"
        );
    }

    #[test]
    fn missing_core_checkpoint_read_is_reported_to_the_caller() {
        use std::cell::Cell;

        let core = hirari_core_bridge::HirariCore::new_offline().expect("offline core initializes");
        let track_id = core.add_midi_track();
        assert!(core.set_track_name(track_id, "Original Session"));
        let tracks = slint::VecModel::<crate::slint_ui::Z_Track>::from(Vec::new());
        assert!(crate::ui::track_model::sync_tracks_from_engine(
            &tracks, &core
        ));
        let restore_called = Cell::new(false);

        let result = load_project_transactionally_with_rollback(
            "injected-candidate.hirari",
            None,
            &tracks,
            &core,
            |core| core.set_track_name(track_id, "Candidate Session"),
            |_path, _tracks, _core| false,
            |checkpoint, core| {
                restore_called.set(true);
                fs::remove_file(checkpoint).expect("fault injection must remove checkpoint");
                core.load_project_v2(checkpoint).is_ok()
            },
        );

        assert_eq!(result, Err(ProjectLoadTransactionError::RollbackFailed));
        assert!(
            restore_called.get(),
            "checkpoint restoration must be attempted"
        );
        let layout: serde_json::Value =
            serde_json::from_str(&core.get_project_layout_json()).unwrap();
        assert_eq!(layout[0]["name"], "Candidate Session");
        assert_eq!(
            tracks.row_data(0).unwrap().name.as_str(),
            "Candidate Session",
            "the UI should reflect the surviving Core graph after rollback fails"
        );
    }
}
