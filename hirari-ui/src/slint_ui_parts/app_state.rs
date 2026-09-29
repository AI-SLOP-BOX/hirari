// --- HIRARI DAW UI CORE ---
// HARDENING: Zero-Leak, Zero-Duplicate Event Architecture

slint::include_modules!();
#[cfg(test)]
pub(crate) use crate::ui::render_inspector::inspect_rendered_wav;
pub(crate) use crate::ui::render_inspector::{
    default_render_output_path, inspect_rendered_wav_result, unix_time_millis,
};
#[allow(unused_imports)]
pub(crate) use crate::ui::telemetry::{
    bounce_state_label, clamp_selection_index, format_bytes, render_progress_status,
    ui_error_message, ui_error_with_action, BounceState, UiErrorKind,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(serde::Deserialize, serde::Serialize)]
struct UiSettings {
    project_path: String,
    #[serde(default)]
    recent_project_paths: Vec<String>,
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
            recent_project_paths: Vec::new(),
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

fn ui_config_dir() -> Option<PathBuf> {
    std::env::var_os("HIRARI_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
        })
}

#[derive(serde::Deserialize, serde::Serialize)]
struct MoufuConnectionSettings {
    address: String,
}

fn moufu_settings_path() -> Option<PathBuf> {
    Some(
        ui_config_dir()?
            .join("hirari-ui")
            .join("moufu-connection.json"),
    )
}

pub(crate) fn load_moufu_address() -> Option<String> {
    let contents = fs::read_to_string(moufu_settings_path()?).ok()?;
    let settings = serde_json::from_str::<MoufuConnectionSettings>(&contents).ok()?;
    let address = settings.address.trim();
    (!address.is_empty()).then(|| address.to_owned())
}

pub(crate) fn save_moufu_address(address: &str) -> std::io::Result<()> {
    let path = moufu_settings_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Hirari configuration directory is unavailable",
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Moufu settings path",
        )
    })?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        "moufu-connection-{}-{nonce}.tmp",
        std::process::id()
    ));
    let serialized = serde_json::to_vec(&MoufuConnectionSettings {
        address: address.trim().to_owned(),
    })
    .map_err(std::io::Error::other)?;
    fs::write(&temporary, serialized)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path)?;
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn session_recovery_dir() -> Option<PathBuf> {
    Some(ui_config_dir()?.join("hirari-ui").join("recovery-sessions"))
}

pub(crate) fn new_session_recovery_path() -> Option<PathBuf> {
    let directory = session_recovery_dir()?;
    fs::create_dir_all(&directory).ok()?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(directory.join(format!(
        "unsaved-session-{}-{nonce}.hirari",
        std::process::id()
    )))
}

/// Finds the newest valid auto-recovery snapshot without mutating the active
/// engine. The UI still asks before opening or discarding the candidate.
pub(crate) fn latest_session_recovery_path() -> Option<PathBuf> {
    let directory = session_recovery_dir()?;
    let mut candidates = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "hirari")
        })
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            hirari_core_bridge::project::ProjectDocument::load(&path.to_string_lossy()).ok()?;
            Some((path, modified))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.cmp(&left.1));
    candidates.into_iter().next().map(|(path, _)| path)
}

/// Deletes only a recovery file created in Hirari's dedicated recovery
/// directory, together with its known generations and editor sidecar.
pub(crate) fn discard_session_recovery(path: &Path) -> bool {
    let Some(directory) = session_recovery_dir() else {
        return false;
    };
    if path.parent() != Some(directory.as_path())
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("unsaved-session-") && name.ends_with(".hirari"))
    {
        return false;
    }

    let path_text = path.to_string_lossy();
    let mut files = vec![
        path.to_path_buf(),
        PathBuf::from(format!("{path_text}.midi.json")),
        PathBuf::from(format!("{path_text}.control-room.json")),
    ];
    for generation in 1..=10 {
        files.push(PathBuf::from(format!("{path_text}.bak.{generation}")));
        files.push(PathBuf::from(format!(
            "{path_text}.midi.json.bak.{generation}"
        )));
    }
    files.into_iter().all(|file| match fs::remove_file(&file) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    })
}

fn ui_settings_path() -> Option<PathBuf> {
    Some(ui_config_dir()?.join("hirari-ui").join("project-path.json"))
}

pub(crate) fn load_saved_project_path() -> Option<String> {
    let path = ui_settings_path()?;
    let settings = load_ui_settings_from_path(&path)?;
    (!settings.project_path.trim().is_empty()).then_some(settings.project_path)
}

pub(crate) fn load_recent_projects() -> Vec<Z_Recent_Project> {
    let Some(settings_path) = ui_settings_path() else {
        return Vec::new();
    };
    let Some(settings) = load_ui_settings_from_path(&settings_path) else {
        return Vec::new();
    };
    let mut seen_paths = Vec::new();
    settings
        .recent_project_paths
        .into_iter()
        .filter_map(|saved_path| {
            let path = PathBuf::from(saved_path);
            if !path.is_file() {
                return None;
            }
            let path = path.canonicalize().unwrap_or(path);
            let path_text = path.to_string_lossy().into_owned();
            if seen_paths.contains(&path_text) || path_text.len() > 4096 {
                return None;
            }
            seen_paths.push(path_text.clone());
            let name = path
                .file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Hirari Project")
                .to_owned();
            let folder = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_owned();
            Some(Z_Recent_Project {
                name: name.into(),
                path: path_text.into(),
                folder: folder.into(),
            })
        })
        .take(12)
        .collect()
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
    let project_path = project_path.trim();
    if project_path.is_empty() || project_path.len() > 4096 || project_path.contains('\0') {
        return;
    }
    let Some(settings_path) = ui_settings_path() else {
        return;
    };
    let mut settings = load_ui_settings_from_path(&settings_path).unwrap_or_default();
    settings.project_path = project_path.to_owned();
    let normalized_path = Path::new(project_path)
        .canonicalize()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| project_path.to_owned());
    settings.recent_project_paths.retain(|recent_path| {
        Path::new(recent_path)
            .canonicalize()
            .map(|path| path.to_string_lossy().as_ref() != normalized_path.as_str())
            .unwrap_or_else(|_| recent_path != &normalized_path)
    });
    settings.recent_project_paths.insert(0, normalized_path);
    settings.recent_project_paths.truncate(12);
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
        .add_filter("Hirari Project", &["hirari", "json"])
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
        .add_filter("Hirari Project", &["hirari"])
        .set_file_name("untitled.hirari")
        .save_file()
        .and_then(|mut path| {
            if path.as_os_str().is_empty() {
                return None;
            }

            if path.extension().is_none() {
                path = PathBuf::from(format!("{}.hirari", path.to_string_lossy()));
            }

            let path = path.to_string_lossy().into_owned();
            (!path.trim().is_empty()).then_some(path)
        })
}

pub(crate) fn choose_dawproject_export_file(default_name: &str) -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("DAWproject", &["dawproject"])
        .set_file_name(default_name)
        .save_file()
        .and_then(|mut path| {
            if path.as_os_str().is_empty() {
                return None;
            }
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("dawproject"))
            {
                path.set_extension("dawproject");
            }
            let path = path.to_string_lossy().into_owned();
            (!path.trim().is_empty()).then_some(path)
        })
}

pub(crate) fn choose_dawproject_import_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("DAWproject", &["dawproject"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
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
pub(super) struct PersistedNote {
    #[serde(default)]
    pub(super) region_id: i32,
    pub(super) pitch: i32,
    #[serde(default)]
    pub(super) midi_channel: u8,
    pub(super) start_beat: f32,
    pub(super) length_beats: f32,
    pub(super) velocity: i32,
    pub(super) articulation: i32,
    #[serde(default)]
    pub(super) vibrato_amount: f32,
    #[serde(default = "default_ui_vibrato_rate")]
    pub(super) vibrato_rate_millihz: i32,
    #[serde(default)]
    pub(super) phoneme: String,
    #[serde(default)]
    pub(super) pitch_curve_cents: Vec<i32>,
    #[serde(default)]
    pub(super) portamento_samples: u32,
    #[serde(default = "default_note_probability")]
    pub(super) probability: i32,
    #[serde(default = "default_note_repeat_count")]
    pub(super) repeat_count: i32,
    #[serde(default)]
    pub(super) lyric: String,
}

fn default_ui_vibrato_rate() -> i32 {
    5000
}

fn default_note_probability() -> i32 {
    100
}

fn default_note_repeat_count() -> i32 {
    1
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct PersistedTrackNotes {
    pub(super) track_id: i32,
    pub(super) notes: Vec<PersistedNote>,
}

pub(crate) fn midi_notes_path(project_path: &str) -> PathBuf {
    PathBuf::from(format!("{}.midi.json", project_path))
}
