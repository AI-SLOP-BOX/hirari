use std::path::PathBuf;
use std::sync::OnceLock;

// Resolved once per process: the plug-in cache and its blacklist share this
// path for the lifetime of the session.
static CACHE_FILE: OnceLock<Option<PathBuf>> = OnceLock::new();

fn prepare_cache_file() -> Option<PathBuf> {
    let directory = match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".hirari"),
        None => PathBuf::from("/tmp/.hirari"),
    };
    std::fs::create_dir_all(&directory).ok()?;
    Some(directory.join("plugin_cache_v8.bin"))
}

/// Cache file path for the Rust-side cache identity and blacklist persistence.
pub(crate) fn cache_file_path() -> Option<&'static PathBuf> {
    CACHE_FILE.get_or_init(prepare_cache_file).as_ref()
}
