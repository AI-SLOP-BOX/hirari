// --- AURA STUDIO PRO v17.8 | INDUSTRIALIZED UI CORE ---
// HARDENING: Zero-Leak, Zero-Duplicate Event Architecture

slint::include_modules!();
pub(crate) use crate::ui::render_inspector::{
    default_render_output_path, inspect_rendered_wav, unix_time_millis,
};
use crate::ui::sync::replace_track;
#[allow(unused_imports)]
pub(crate) use crate::ui::telemetry::{
    bounce_state_label, clamp_selection_index, format_bytes, render_progress_status,
    ui_error_message, ui_error_with_action, BounceState, UiErrorKind,
};
use slint::Model;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(serde::Deserialize, serde::Serialize)]
struct UiSettings {
    project_path: String,
    #[serde(default)]
    audio_library_path: String,
    #[serde(default = "default_beginner_mode")]
    beginner_mode: bool,
    #[serde(default)]
    focus_mode: bool,
    #[serde(default = "default_workspace_preset")]
    workspace_preset: String,
    #[serde(default)]
    plugin_favorites: Vec<String>,
    #[serde(default)]
    onboarding_completed: bool,
    #[serde(default)]
    onboarding_step: u8,
}

fn default_beginner_mode() -> bool {
    true
}

fn default_workspace_preset() -> String {
    "arrange".to_owned()
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            project_path: String::new(),
            audio_library_path: String::new(),
            beginner_mode: true,
            focus_mode: false,
            workspace_preset: default_workspace_preset(),
            plugin_favorites: Vec::new(),
            onboarding_completed: false,
            onboarding_step: 0,
        }
    }
}

fn ui_settings_path() -> Option<PathBuf> {
    let config_dir = std::env::var_os("AURA_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
        })?;
    Some(config_dir.join("aura-ui").join("project-path.json"))
}

pub(crate) fn load_saved_project_path() -> Option<String> {
    let path = ui_settings_path()?;
    let settings = load_ui_settings_from_path(&path)?;
    (!settings.project_path.trim().is_empty()).then_some(settings.project_path)
}

pub(crate) fn load_audio_library_path() -> Option<String> {
    let path = ui_settings_path()?;
    let settings = load_ui_settings_from_path(&path)?;
    (!settings.audio_library_path.trim().is_empty()).then_some(settings.audio_library_path)
}

pub(crate) fn store_audio_library_path(library_path: &str) {
    let library_path = library_path.trim();
    if library_path.is_empty() || library_path.len() > 4096 || library_path.contains('\0') {
        return;
    }
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.audio_library_path = library_path.to_owned();
    store_ui_settings(&settings_path, &settings);
}

fn load_ui_settings_from_path(path: &Path) -> Option<UiSettings> {
    serde_json::from_str::<UiSettings>(&fs::read_to_string(path).ok()?).ok()
}

pub(crate) fn load_beginner_mode() -> bool {
    ui_settings_path()
        .and_then(|path| load_ui_settings_from_path(&path))
        .map(|settings| settings.beginner_mode)
        .unwrap_or(true)
}

pub(crate) fn store_beginner_mode(beginner_mode: bool) {
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.beginner_mode = beginner_mode;
    store_ui_settings(&settings_path, &settings);
}

pub(crate) fn load_onboarding_progress() -> (bool, u8) {
    ui_settings_path()
        .and_then(|path| load_ui_settings_from_path(&path))
        .map(|settings| {
            (
                settings.onboarding_completed,
                settings.onboarding_step.min(8),
            )
        })
        .unwrap_or((false, 0))
}

pub(crate) fn store_onboarding_progress(completed: bool, step: u8) {
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.onboarding_completed = completed;
    settings.onboarding_step = step.min(8);
    store_ui_settings(&settings_path, &settings);
}

pub(crate) fn load_focus_mode() -> bool {
    ui_settings_path()
        .and_then(|path| load_ui_settings_from_path(&path))
        .map(|settings| settings.focus_mode)
        .unwrap_or(false)
}

pub(crate) fn store_focus_mode(focus_mode: bool) {
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.focus_mode = focus_mode;
    store_ui_settings(&settings_path, &settings);
}

pub(crate) fn load_workspace_preset() -> String {
    ui_settings_path()
        .and_then(|path| load_ui_settings_from_path(&path))
        .map(|settings| settings.workspace_preset)
        .filter(|preset| {
            matches!(
                preset.as_str(),
                "arrange" | "mix" | "vocal" | "sound_design"
            )
        })
        .unwrap_or_else(default_workspace_preset)
}

pub(crate) fn store_workspace_preset(preset: &str) {
    if !matches!(preset, "arrange" | "mix" | "vocal" | "sound_design") {
        return;
    }
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.workspace_preset = preset.to_owned();
    store_ui_settings(&settings_path, &settings);
}

pub(crate) fn load_plugin_favorites() -> std::collections::HashSet<String> {
    ui_settings_path()
        .and_then(|path| load_ui_settings_from_path(&path))
        .map(|settings| settings.plugin_favorites.into_iter().collect())
        .unwrap_or_default()
}

pub(crate) fn store_plugin_favorite(plugin_id: &str, favorite: bool) {
    let plugin_id = plugin_id.trim();
    if plugin_id.is_empty() || plugin_id.len() > 256 || plugin_id.contains('\0') {
        return;
    }
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.plugin_favorites.retain(|id| id != plugin_id);
    if favorite {
        settings.plugin_favorites.push(plugin_id.to_owned());
        settings.plugin_favorites.sort();
        settings.plugin_favorites.dedup();
    }
    store_ui_settings(&settings_path, &settings);
}

pub(crate) fn store_project_path(project_path: &str) {
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.project_path = project_path.to_owned();
    store_ui_settings(&settings_path, &settings);
}

fn store_ui_settings(settings_path: &Path, settings: &UiSettings) {
    let Some(parent) = settings_path.parent() else {
        return;
    };
    let Ok(contents) = serde_json::to_vec(settings) else {
        return;
    };
    // This is deliberately best-effort: a settings failure must not affect the UI.
    let _ = fs::create_dir_all(parent).and_then(|()| fs::write(settings_path, contents));
}

pub(crate) fn choose_project_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Aura Project", &["aura", "json"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

pub(crate) fn choose_audio_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Audio", &["wav", "aiff", "aif", "flac"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

pub(crate) fn choose_video_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Video", &["mov", "mp4", "m4v", "mkv", "avi"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Convert filesystem paths to UI-safe labels without exposing the OS account
/// name or the machine's home directory. Full paths are still retained
/// internally for file I/O; only user-facing status text uses this formatter.
pub(crate) fn display_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(relative) = path.strip_prefix(&home) {
            let relative = relative.to_string_lossy();
            return if relative.is_empty() {
                "<home>".to_owned()
            } else {
                format!("<home>/{relative}")
            };
        }
    }
    if let Ok(project_root) = std::env::current_dir() {
        if let Ok(relative) = path.strip_prefix(&project_root) {
            return format!("<project>/{}", relative.to_string_lossy());
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "<path>".to_owned())
}

pub(crate) fn resolve_audio_path(requested: &str) -> Option<String> {
    let requested = requested.trim();
    if !requested.is_empty() && PathBuf::from(requested).is_file() {
        return Some(requested.to_owned());
    }
    choose_audio_file()
}

pub(crate) fn choose_audio_library_directory() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

pub(crate) fn choose_project_save_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Aura Project", &["aura"])
        .set_file_name("untitled.aura")
        .save_file()
        .and_then(|mut path| {
            if path.as_os_str().is_empty() {
                return None;
            }

            if path.extension().is_none() {
                path = PathBuf::from(format!("{}.aura", path.to_string_lossy()));
            }

            let path = path.to_string_lossy().into_owned();
            (!path.trim().is_empty()).then_some(path)
        })
}

pub(crate) fn scan_installed_plugin_count() -> (usize, usize) {
    let mut paths = vec![
        PathBuf::from("/Library/Audio/Plug-Ins/Components"),
        PathBuf::from("/Library/Audio/Plug-Ins/VST3"),
        PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.extend([
            home.join("Library/Audio/Plug-Ins/Components"),
            home.join("Library/Audio/Plug-Ins/VST3"),
            home.join("Library/Audio/Plug-Ins/CLAP"),
        ]);
    }

    let mut discovered = std::collections::HashSet::new();
    let blacklist = load_plugin_blacklist();
    let mut rejected = 0usize;
    for directory in paths {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase());
            let is_plugin = matches!(extension.as_deref(), Some("component" | "vst3" | "clap"));
            if !is_plugin {
                continue;
            }
            if blacklist.contains(&path.to_string_lossy().into_owned()) {
                rejected += 1;
                continue;
            }
            if path.is_dir() || path.is_file() {
                discovered.insert(path);
            } else {
                rejected += 1;
            }
        }
    }
    (discovered.len(), rejected)
}

fn plugin_blacklist_path() -> Option<PathBuf> {
    ui_settings_path().map(|path| path.with_file_name("plugin-blacklist.txt"))
}

pub(crate) fn load_plugin_blacklist() -> std::collections::HashSet<String> {
    let Some(path) = plugin_blacklist_path() else {
        return std::collections::HashSet::new();
    };
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn clear_plugin_blacklist() -> bool {
    let Some(file) = plugin_blacklist_path() else {
        return false;
    };
    let Some(parent) = file.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    fs::write(file, b"").is_ok()
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PersistedNote {
    pitch: i32,
    start_beat: f32,
    length_beats: f32,
    velocity: i32,
    articulation: i32,
    #[serde(default)]
    lyric: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PersistedTrackNotes {
    track_id: i32,
    notes: Vec<PersistedNote>,
}

pub(crate) fn midi_notes_path(project_path: &str) -> PathBuf {
    PathBuf::from(format!("{}.midi.json", project_path))
}

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

pub(crate) fn load_ui_midi_notes(project_path: &str, tracks: &slint::VecModel<Z_Track>) -> bool {
    let Ok(data) = fs::read(midi_notes_path(project_path)) else {
        return false;
    };
    let Ok(persisted) = serde_json::from_slice::<Vec<PersistedTrackNotes>>(&data) else {
        return false;
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
    native_ready && load_ui_midi_notes(project_path, tracks) && {
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
    use super::{atomic_write_midi_sidecar, UiSettings};
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
}
