//! Neutral shared listing of recent projects. The AmeUI home page asks this; it must not depend on a windowing backend.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub folder: String,
}

fn config_dir() -> Option<PathBuf> {
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
struct Settings {
    #[serde(default)]
    recent_project_paths: Vec<String>,
}

pub fn load_recent_projects() -> Vec<RecentProject> {
    let Some(config_dir) = config_dir() else {
        return Vec::new();
    };
    let settings_path = config_dir.join("hirari-ui").join("project-path.json");
    let Ok(text) = fs::read_to_string(&settings_path) else {
        return Vec::new();
    };
    let Ok(settings) = serde_json::from_str::<Settings>(&text) else {
        return Vec::new();
    };
    let mut seen = Vec::<String>::new();
    settings
        .recent_project_paths
        .into_iter()
        .filter_map(|saved| {
            let path = PathBuf::from(saved);
            if !path.is_file() {
                return None;
            }
            let path = path.canonicalize().unwrap_or(path);
            let path_text = path.to_string_lossy().into_owned();
            if seen.contains(&path_text) || path_text.len() > 4096 {
                return None;
            }
            seen.push(path_text.clone());
            let name = path
                .file_stem()
                .and_then(|v| v.to_str())
                .filter(|v| !v.trim().is_empty())
                .unwrap_or("Hirari Project")
                .to_owned();
            let folder = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|v| v.to_str())
                .unwrap_or_default()
                .to_owned();
            Some(RecentProject {
                name,
                path: path_text,
                folder,
            })
        })
        .take(12)
        .collect()
}
