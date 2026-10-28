use std::ffi::{c_char, CString};
use std::sync::OnceLock;

// Resolved once per process: the plug-in cache and its blacklist share this
// path for the lifetime of the session.
static CACHE_PATH: OnceLock<Option<CString>> = OnceLock::new();

fn prepare_cache_path() -> Option<CString> {
    let directory = match std::env::var_os("HOME") {
        Some(home) => std::path::PathBuf::from(home).join(".hirari"),
        None => std::path::PathBuf::from("/tmp/.hirari"),
    };
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("plugin_cache_v8.bin");
    CString::new(path.to_string_lossy().as_bytes()).ok()
}

/// Returns the plug-in cache file path, or null when the cache directory
/// could not be created. The pointer is owned by Rust and valid for the
/// process lifetime.
#[no_mangle]
pub extern "C" fn hirari_plugin_cache_path() -> *const c_char {
    CACHE_PATH
        .get_or_init(prepare_cache_path)
        .as_ref()
        .map_or(std::ptr::null(), |path| path.as_ptr())
}
