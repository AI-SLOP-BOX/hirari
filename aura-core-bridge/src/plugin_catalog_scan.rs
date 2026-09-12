fn quarantine_file() -> PathBuf {
    if let Some(path) = std::env::var_os("AURA_PLUGIN_QUARANTINE_FILE") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/Aura/quarantined-plugins.txt")
}

fn quarantine_set() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| {
        let entries = std::fs::read_to_string(quarantine_file())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(str::to_owned)
            .collect();
        Mutex::new(entries)
    })
}

fn persist_quarantine(set: &HashSet<String>) -> std::io::Result<()> {
    persist_sorted_plugin_ids(&quarantine_file(), set)
}

fn favorites_file() -> PathBuf {
    if let Some(path) = std::env::var_os("AURA_PLUGIN_FAVORITES_FILE") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/Aura/favorite-plugins.txt")
}

fn favorite_set() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| {
        let entries = std::fs::read_to_string(favorites_file())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && line.len() <= 256 && !line.contains('\0'))
            .map(str::to_owned)
            .collect();
        Mutex::new(entries)
    })
}

fn persist_favorites(set: &HashSet<String>) -> std::io::Result<()> {
    persist_sorted_plugin_ids(&favorites_file(), set)
}

/// Persist plugin IDs through a collision-resistant, fsync-backed rename.
/// The create-new step prevents concurrent writers from ever sharing a temp
/// path, while cleanup keeps failed writes from accumulating beside the
/// canonical file.
fn persist_sorted_plugin_ids(path: &std::path::Path, set: &HashSet<String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut values: Vec<&String> = set.iter().collect();
    values.sort();
    let contents = values
        .into_iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Persist a stable catalog ID as a user favorite. Binary identity remains
/// separate: replacing a plugin invalidates its cached state/quarantine
/// identity without unexpectedly removing the user's browser favorite.
pub fn set_favorite(id: &str, favorite: bool) -> std::io::Result<bool> {
    if id.trim().is_empty() || id.len() > 256 || id.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid plugin id",
        ));
    }
    let mut set = favorite_set()
        .lock()
        .map_err(|_| std::io::Error::other("favorite state unavailable"))?;
    let changed = if favorite {
        set.insert(id.to_owned())
    } else {
        set.remove(id)
    };
    if changed {
        if let Err(error) = persist_favorites(&set) {
            // Do not leave the process with a state that was never committed
            // to disk; callers may immediately issue another catalog query.
            if favorite {
                set.remove(id);
            } else {
                set.insert(id.to_owned());
            }
            return Err(error);
        }
    }
    Ok(changed)
}

pub fn is_quarantined_hash(binary_hash: &str) -> bool {
    quarantine_set()
        .lock()
        .map(|set| set.contains(binary_hash))
        .unwrap_or(false)
}

/// Check quarantine by concrete plugin path without trusting the display
/// name or extension.  This is deliberately separate from `is_admitted_path`:
/// project hydration may encounter a plugin that is not in the current scan
/// roots, but a known-bad binary must still never be re-launched.
pub fn is_quarantined_path(path: &str) -> bool {
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    let (hash, bytes, _) = binary_identity(&canonical);
    bytes != 0 && !hash.is_empty() && is_quarantined_hash(&hash)
}

pub fn quarantine_plugin(plugin: &InstalledPlugin) -> Result<(), String> {
    if plugin.binary_hash.len() != 64 {
        return Err("plugin has no valid binary identity".into());
    }
    let mut set = quarantine_set()
        .lock()
        .map_err(|_| "quarantine lock poisoned")?;
    set.insert(plugin.binary_hash.clone());
    persist_quarantine(&set).map_err(|error| error.to_string())
}

pub fn clear_plugin_quarantine(plugin: &InstalledPlugin) -> Result<bool, String> {
    let mut set = quarantine_set()
        .lock()
        .map_err(|_| "quarantine lock poisoned")?;
    let removed = set.remove(&plugin.binary_hash);
    if removed {
        persist_quarantine(&set).map_err(|error| error.to_string())?;
    }
    Ok(removed)
}

pub fn quarantine_path(path: &str) -> Result<InstalledPlugin, String> {
    let requested = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let plugin = scan()
        .into_iter()
        .find(|plugin| std::fs::canonicalize(&plugin.path).ok().as_ref() == Some(&requested))
        .ok_or_else(|| "plugin path is not in the installed catalog".to_owned())?;
    quarantine_plugin(&plugin)?;
    Ok(plugin)
}

pub fn clear_quarantine_path(path: &str) -> Result<InstalledPlugin, String> {
    let requested = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let plugin = scan()
        .into_iter()
        .find(|plugin| std::fs::canonicalize(&plugin.path).ok().as_ref() == Some(&requested))
        .ok_or_else(|| "plugin path is not in the installed catalog".to_owned())?;
    clear_plugin_quarantine(&plugin)?;
    Ok(plugin)
}

include!("plugin_catalog_discovery.rs");
