//! Minimal user preference toggles shared by the Slint dock and the AmeUI
//! surface. Reads/writes the same `hirari-ui/project-path.json` settings
//! file the rest of Hirari uses, so both surfaces stay in step.

use std::fs;
use std::path::PathBuf;

#[derive(serde::Deserialize, serde::Serialize)]
struct Preferences {
    #[serde(default)]
    focus_mode: bool,
}

fn settings_path() -> Option<PathBuf> {
    let config = std::env::var_os("HIRARI_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
        })?;
    Some(config.join("hirari-ui").join("project-path.json"))
}

fn load() -> Preferences {
    let Some(path) = settings_path() else {
        return Preferences { focus_mode: false };
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Preferences>(&text).ok())
        .unwrap_or(Preferences { focus_mode: false })
}

fn store(preferences: &Preferences) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Hirari configuration directory is unavailable",
        ));
    };
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid settings path",
        ));
    };
    fs::create_dir_all(parent)?;
    // Read-modify-write preserves every other key in the file (project)
    // paths, recents, audio library path) for the legacy surface.
    let mut merged =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).unwrap_or_default())
            .unwrap_or_else(|_| serde_json::json!({}));
    let object = merged.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "project-path.json is not an object",
        )
    })?;
    object.insert(
        "focus_mode".to_owned(),
        serde_json::Value::Bool(preferences.focus_mode),
    );
    fs::write(&path, serde_json::to_vec(&merged)?)?;
    Ok(())
}

pub(crate) fn load_focus_mode() -> bool {
    load().focus_mode
}

pub(crate) fn store_focus_mode(focus_mode: bool) -> std::io::Result<()> {
    let preferences = Preferences { focus_mode };
    store(&preferences)
}
