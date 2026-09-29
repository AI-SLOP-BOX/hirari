//! Versioned binary codec for the native plug-in metadata cache.
//!
//! The host still owns plug-in discovery and admission. Cache framing and
//! endian-stable serialization are implemented here so cache bytes no longer
//! depend on C++ stream layout.

use std::ffi::c_char;
use std::io::Read;
use std::path::PathBuf;

const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

#[repr(C)]
pub struct PluginCacheRecordView {
    pub name: *const c_char,
    pub name_size: usize,
    pub path: *const c_char,
    pub path_size: usize,
    pub plugin_type: u32,
    pub subtype: u32,
    pub fingerprint: u64,
}

#[repr(C)]
pub struct PluginCacheRecordOwned {
    pub name: *mut u8,
    pub name_size: usize,
    pub path: *mut u8,
    pub path_size: usize,
    pub plugin_type: u32,
    pub subtype: u32,
    pub fingerprint: u64,
}

/// Encodes the established cache record layout using explicit little-endian
/// fields. The returned boxed byte slice belongs to Rust until freed by the
/// paired release function.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_cache_encode(
    magic: u32,
    version: u32,
    records: *const PluginCacheRecordView,
    record_count: usize,
    output: *mut *mut u8,
    output_size: *mut usize,
) -> u8 {
    if (record_count != 0 && records.is_null())
        || output.is_null()
        || output_size.is_null()
        || record_count > 10_000
    {
        return 0;
    }
    *output = std::ptr::null_mut();
    *output_size = 0;
    let records = if record_count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(records, record_count)
    };
    let mut bytes = Vec::with_capacity(12 + record_count.saturating_mul(48));
    bytes.extend_from_slice(&magic.to_le_bytes());
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&(record_count as u32).to_le_bytes());
    for record in records {
        if record.name_size > 1024 * 1024
            || record.path_size > 1024 * 1024
            || (record.name_size != 0 && record.name.is_null())
            || (record.path_size != 0 && record.path.is_null())
        {
            return 0;
        }
        let name = if record.name_size == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(record.name.cast::<u8>(), record.name_size)
        };
        let path = if record.path_size == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(record.path.cast::<u8>(), record.path_size)
        };
        bytes.extend_from_slice(&(record.name_size as u32).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&(record.path_size as u32).to_le_bytes());
        bytes.extend_from_slice(path);
        bytes.extend_from_slice(&record.plugin_type.to_le_bytes());
        bytes.extend_from_slice(&record.subtype.to_le_bytes());
        bytes.extend_from_slice(&record.fingerprint.to_le_bytes());
    }
    let mut bytes = bytes.into_boxed_slice();
    *output_size = bytes.len();
    *output = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    1
}

/// Reads and decodes a cache file into Rust-owned records. The C++ host still
/// applies plug-in admission and current-binary fingerprint checks afterward.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_cache_load(
    path: *const std::ffi::c_char,
    path_size: usize,
    expected_magic: u32,
    expected_version: u32,
    output: *mut *mut PluginCacheRecordOwned,
    output_count: *mut usize,
) -> u8 {
    if path.is_null()
        || path_size == 0
        || path_size > 1024 * 1024
        || output.is_null()
        || output_count.is_null()
    {
        return 0;
    }
    *output = std::ptr::null_mut();
    *output_count = 0;
    let path_bytes = std::slice::from_raw_parts(path.cast::<u8>(), path_size);
    let Some(path) = path_from_bytes(path_bytes) else {
        return 0;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    if file
        .metadata()
        .map(|metadata| metadata.len() > MAX_CACHE_BYTES)
        .unwrap_or(true)
    {
        return 0;
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_CACHE_BYTES
    {
        return 0;
    }
    let mut cursor = CacheCursor {
        bytes: &bytes,
        offset: 0,
    };
    if cursor.u32() != Some(expected_magic) || cursor.u32() != Some(expected_version) {
        return 0;
    }
    let Some(count) = cursor.u32().map(|count| count as usize) else {
        return 0;
    };
    if count > 10_000 {
        return 0;
    }
    let mut decoded = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(name) = cursor.blob() else { return 0 };
        let Some(path) = cursor.blob() else { return 0 };
        let (Some(plugin_type), Some(subtype), Some(fingerprint)) =
            (cursor.u32(), cursor.u32(), cursor.u64())
        else {
            return 0;
        };
        decoded.push((name, path, plugin_type, subtype, fingerprint));
    }
    if cursor.offset != bytes.len() {
        return 0;
    }

    let mut records = Vec::with_capacity(decoded.len());
    for (name, path, plugin_type, subtype, fingerprint) in decoded {
        let (name, name_size) = boxed_bytes(name);
        let (path, path_size) = boxed_bytes(path);
        records.push(PluginCacheRecordOwned {
            name,
            name_size,
            path,
            path_size,
            plugin_type,
            subtype,
            fingerprint,
        });
    }
    if !records.is_empty() {
        let records = records.into_boxed_slice();
        *output_count = records.len();
        *output = Box::into_raw(records) as *mut PluginCacheRecordOwned;
    }
    1
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(std::str::from_utf8(bytes).ok()?))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_cache_records_free(
    records: *mut PluginCacheRecordOwned,
    count: usize,
) {
    if records.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(records, count);
    for record in &mut *slice {
        free_bytes(record.name, record.name_size);
        free_bytes(record.path, record.path_size);
    }
    drop(Box::from_raw(slice));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_cache_free(bytes: *mut u8, size: usize) {
    if !bytes.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            bytes, size,
        )));
    }
}

fn boxed_bytes(bytes: Vec<u8>) -> (*mut u8, usize) {
    if bytes.is_empty() {
        return (std::ptr::null_mut(), 0);
    }
    let bytes = bytes.into_boxed_slice();
    let size = bytes.len();
    (Box::into_raw(bytes) as *mut u8, size)
}

unsafe fn free_bytes(bytes: *mut u8, size: usize) {
    if !bytes.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            bytes, size,
        )));
    }
}

struct CacheCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl CacheCursor<'_> {
    fn read(&mut self, size: usize) -> Option<&[u8]> {
        let end = self.offset.checked_add(size)?;
        let value = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(value)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.read(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.read(8)?.try_into().ok()?))
    }

    fn blob(&mut self) -> Option<Vec<u8>> {
        let size = self.u32()? as usize;
        (size <= 1024 * 1024).then_some(())?;
        Some(self.read(size)?.to_vec())
    }
}
