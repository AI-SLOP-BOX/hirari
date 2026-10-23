//! Versioned, checksummed plugin preset files.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAGIC: u32 = 0x4155_5053;
const VERSION: u32 = 3;
const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct PluginPresetFile {
    plugin_type: u32,
    state: Vec<u8>,
}

fn preset_path(path: *const u8, length: usize) -> Option<PathBuf> {
    if path.is_null() || length == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, length) };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(std::str::from_utf8(bytes).ok()?))
    }
}

fn legacy_checksum(data: &[u8]) -> u64 {
    data.iter().fold(1_469_598_103_934_665_603_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
    })
}

fn temporary_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".tmp-");
    #[cfg(not(windows))]
    {
        name.push(std::process::id().to_string());
        name.push("-");
    }
    #[cfg(windows)]
    name.push("process-");
    name.push(TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed).to_string());
    PathBuf::from(name)
}

/// Writes the current version-3 native preset format, retaining its atomic
/// replacement and durability behavior.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_save(
    path: *const u8,
    path_length: usize,
    plugin_type: u32,
    state: *const u8,
    state_length: usize,
) -> bool {
    if state.is_null() || state_length == 0 || state_length > MAX_STATE_BYTES {
        return false;
    }
    let Some(target) = preset_path(path, path_length) else {
        return false;
    };
    let state = unsafe { std::slice::from_raw_parts(state, state_length) };
    let temporary = temporary_path(&target);
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&MAGIC.to_ne_bytes())?;
        file.write_all(&VERSION.to_ne_bytes())?;
        file.write_all(&plugin_type.to_ne_bytes())?;
        file.write_all(&(state_length as u32).to_ne_bytes())?;
        file.write_all(&legacy_checksum(state).to_ne_bytes())?;
        file.write_all(state)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &target)?;
        #[cfg(unix)]
        {
            let parent = target
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            if let Ok(directory) = File::open(parent) {
                directory.sync_all()?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.is_ok()
}

/// Loads and validates a preset. Extra trailing bytes remain ignored, as in
/// the former stream reader.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_load(
    path: *const u8,
    path_length: usize,
) -> *mut PluginPresetFile {
    let Some(path) = preset_path(path, path_length) else {
        return std::ptr::null_mut();
    };
    let Ok(mut file) = File::open(path) else {
        return std::ptr::null_mut();
    };
    let mut header = [0u8; 24];
    if file.read_exact(&mut header).is_err() {
        return std::ptr::null_mut();
    }
    let magic = u32::from_ne_bytes(header[0..4].try_into().unwrap());
    let version = u32::from_ne_bytes(header[4..8].try_into().unwrap());
    let plugin_type = u32::from_ne_bytes(header[8..12].try_into().unwrap());
    let size = u32::from_ne_bytes(header[12..16].try_into().unwrap()) as usize;
    let expected_checksum = u64::from_ne_bytes(header[16..24].try_into().unwrap());
    if magic != MAGIC || version != VERSION || size == 0 || size > MAX_STATE_BYTES {
        return std::ptr::null_mut();
    }
    let mut state = vec![0; size];
    if file.read_exact(&mut state).is_err() || legacy_checksum(&state) != expected_checksum {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(PluginPresetFile { plugin_type, state }))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_type(
    preset: *const PluginPresetFile,
) -> u32 {
    unsafe { preset.as_ref() }.map_or(0, |preset| preset.plugin_type)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_data(
    preset: *const PluginPresetFile,
) -> *const u8 {
    unsafe { preset.as_ref() }.map_or(std::ptr::null(), |preset| preset.state.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_size(
    preset: *const PluginPresetFile,
) -> usize {
    unsafe { preset.as_ref() }.map_or(0, |preset| preset.state.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_preset_file_free(preset: *mut PluginPresetFile) {
    if !preset.is_null() {
        drop(unsafe { Box::from_raw(preset) });
    }
}
