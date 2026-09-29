//! Rust-owned, fingerprint-aware blacklist for plug-in scan failures.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::io::Write;
use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[repr(C)]
pub struct PluginBlacklistRecord {
    pub path: *mut u8,
    pub path_size: usize,
    pub reason: u32,
}

#[derive(Clone, Copy)]
struct Entry {
    reason: u32,
    fingerprint: Option<u64>,
}

struct Blacklist {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl Blacklist {
    fn open(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter_map(parse_record)
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    fn persist(&self, entries: &BTreeMap<String, Entry>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let temporary = self.path.with_extension(format!(
            "blacklist-{}-{nonce}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = Vec::new();
        for (path, entry) in entries {
            bytes.push(b'"');
            for byte in path.bytes() {
                if byte == b'"' || byte == b'\\' {
                    bytes.push(b'\\');
                }
                bytes.push(byte);
            }
            bytes.extend_from_slice(format!("\" {}", entry.reason).as_bytes());
            if let Some(fingerprint) = entry.fingerprint {
                bytes.extend_from_slice(format!(" {fingerprint}").as_bytes());
            }
            bytes.push(b'\n');
        }
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            if let Some(parent) = self.path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
            Ok::<(), std::io::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

fn parse_record(line: &str) -> Option<(String, Entry)> {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    let mut path = Vec::new();
    let mut closed = false;
    while let Some(&byte) = bytes.get(cursor) {
        cursor += 1;
        match byte {
            b'"' => {
                closed = true;
                break;
            }
            b'\\' => path.push(*bytes.get(cursor)?),
            other => path.push(other),
        }
        if byte == b'\\' {
            cursor += 1;
        }
    }
    if !closed {
        return None;
    }
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let reason_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let reason = std::str::from_utf8(bytes.get(reason_start..cursor)?)
        .ok()?
        .parse::<u32>()
        .ok()?;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let fingerprint_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let fingerprint = (cursor > fingerprint_start)
        .then(|| {
            std::str::from_utf8(&bytes[fingerprint_start..cursor])
                .ok()?
                .parse()
                .ok()
        })
        .flatten();
    let path = String::from_utf8(path).ok()?;
    if path.len() > 4096 || path.contains('\0') {
        return None;
    }
    Some((
        cache_key(&path),
        Entry {
            reason,
            fingerprint,
        },
    ))
}

fn cache_key(value: &str) -> String {
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or(path.clone(), |cwd| cwd.join(path))
    };
    let mut existing = absolute.clone();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        suffix.push(name.to_os_string());
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

/// Produces the canonical slash-separated identity used by blacklist records
/// and the plugin cache facade. A null output with zero capacity queries the
/// required byte count; output is not NUL terminated.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_cache_key(
    path: *const u8,
    path_size: usize,
    output: *mut u8,
    output_capacity: usize,
    output_size: *mut usize,
) -> bool {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 || output_size.is_null() {
        return false;
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    let Ok(path) = std::str::from_utf8(path_bytes) else {
        return false;
    };
    let key = cache_key(path);
    unsafe { *output_size = key.len() };
    if key.is_empty() {
        return true;
    }
    if output.is_null() && output_capacity == 0 {
        return true;
    }
    if output.is_null() || output_capacity < key.len() {
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(key.as_ptr(), output, key.len()) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_create(
    path: *const u8,
    path_size: usize,
) -> *mut c_void {
    if path.is_null() || path_size == 0 || path_size > 4096 {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    };
    #[cfg(not(unix))]
    let path = match std::str::from_utf8(bytes) {
        Ok(path) => PathBuf::from(path),
        Err(_) => return std::ptr::null_mut(),
    };
    Box::into_raw(Box::new(Blacklist::open(path))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<Blacklist>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_is_blocked(
    state: *const c_void,
    path: *const c_char,
    path_size: usize,
    fingerprint: u64,
    has_fingerprint: bool,
) -> bool {
    let Some(state) = (unsafe { state.cast::<Blacklist>().as_ref() }) else {
        return false;
    };
    let Some(path) = (unsafe { read_string(path, path_size) }) else {
        return false;
    };
    let Ok(entries) = state.entries.lock() else {
        return false;
    };
    let Some(entry) = entries.get(&path) else {
        return false;
    };
    match entry.fingerprint {
        None => true,
        Some(saved) => has_fingerprint && saved == fingerprint,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_reason(
    state: *const c_void,
    path: *const c_char,
    path_size: usize,
) -> u32 {
    let Some(state) = (unsafe { state.cast::<Blacklist>().as_ref() }) else {
        return 0;
    };
    let Some(path) = (unsafe { read_string(path, path_size) }) else {
        return 0;
    };
    state
        .entries
        .lock()
        .ok()
        .and_then(|entries| entries.get(&path).map(|entry| entry.reason))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_set(
    state: *mut c_void,
    path: *const c_char,
    path_size: usize,
    reason: u32,
    fingerprint: u64,
    has_fingerprint: bool,
) -> bool {
    let Some(state) = (unsafe { state.cast::<Blacklist>().as_mut() }) else {
        return false;
    };
    let Some(path) = (unsafe { read_string(path, path_size) }) else {
        return false;
    };
    let Ok(mut entries) = state.entries.lock() else {
        return false;
    };
    entries.insert(
        path,
        Entry {
            reason,
            fingerprint: has_fingerprint.then_some(fingerprint),
        },
    );
    state.persist(&entries).is_ok()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_remove(
    state: *mut c_void,
    path: *const c_char,
    path_size: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<Blacklist>().as_mut() }) else {
        return false;
    };
    let Some(path) = (unsafe { read_string(path, path_size) }) else {
        return false;
    };
    let Ok(mut entries) = state.entries.lock() else {
        return false;
    };
    entries.remove(&path);
    state.persist(&entries).is_ok()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_snapshot(
    state: *const c_void,
    output: *mut *mut PluginBlacklistRecord,
    output_count: *mut usize,
) -> bool {
    if state.is_null() || output.is_null() || output_count.is_null() {
        return false;
    }
    unsafe {
        output.write(std::ptr::null_mut());
        output_count.write(0);
    }
    let state = unsafe { &*state.cast::<Blacklist>() };
    let Ok(entries) = state.entries.lock() else {
        return false;
    };
    let mut records = Vec::with_capacity(entries.len());
    for (path, entry) in entries.iter() {
        let bytes = path.as_bytes().to_vec().into_boxed_slice();
        let size = bytes.len();
        records.push(PluginBlacklistRecord {
            path: Box::into_raw(bytes) as *mut u8,
            path_size: size,
            reason: entry.reason,
        });
    }
    if !records.is_empty() {
        let records = records.into_boxed_slice();
        unsafe {
            output_count.write(records.len());
            output.write(Box::into_raw(records) as *mut PluginBlacklistRecord);
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_blacklist_snapshot_free(
    records: *mut PluginBlacklistRecord,
    count: usize,
) {
    if records.is_null() {
        return;
    }
    let records = std::ptr::slice_from_raw_parts_mut(records, count);
    for record in unsafe { &mut *records } {
        if !record.path.is_null() {
            let bytes = std::ptr::slice_from_raw_parts_mut(record.path, record.path_size);
            unsafe { drop(Box::from_raw(bytes)) };
        }
    }
    unsafe { drop(Box::from_raw(records)) };
}

unsafe fn read_string(path: *const c_char, path_size: usize) -> Option<String> {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path.cast::<u8>(), path_size) };
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.contains('\0')).then(|| text.to_owned())
}
