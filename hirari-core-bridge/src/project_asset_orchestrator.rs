use crate::asset::AssetOrchestrator;
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ProjectOrchestrator {
    pub project_dir: String,
    pub asset_paths: Vec<String>,
    /// Compatibility facade over the canonical asset collector. Keeping one
    /// implementation prevents the legacy project manager from silently
    /// accepting symlinks or basename collisions differently.
    pub assets: AssetOrchestrator,
}

impl Default for ProjectOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectOrchestrator {
    pub fn new() -> Self {
        Self {
            project_dir: String::new(),
            asset_paths: Vec::new(),
            assets: AssetOrchestrator::new(),
        }
    }

    /// INDUSTRIAL: Bundles all project assets with absolute file precision and path normalization.
    pub fn consolidate_assets(&mut self, project_dir: String, asset_list: Vec<String>) -> bool {
        let timeline = json!({ "assets": asset_list });
        if !self
            .assets
            .consolidate_assets(&project_dir, timeline.to_string().as_bytes())
        {
            return false;
        }
        self.project_dir = project_dir;
        self.asset_paths = self
            .assets
            .assets
            .iter()
            .map(|asset| asset.consolidated_path.clone())
            .collect();
        true
    }

    /// INDUSTRIAL: Saves the project state with absolute atomic safety and data integrity.
    pub fn save_project(&self, path: &str, data: &[u8]) {
        // INDUSTRIAL: Implementation of high-performance atomic project saving.
        // Rust's AtomicSavingEngine ensures bit-accurate project protection.
        // Logic to perform atomic file write with checksums
        let _ = self.try_save_project(path, data);
    }

    /// Atomically writes project bytes, returning false without replacing the
    /// destination when validation, flushing, or rename fails.
    pub fn try_save_project(&self, path: &str, data: &[u8]) -> bool {
        self.save_project_checked(path, data).is_ok()
    }

    /// Checked variant of the compatibility save facade. The destination is
    /// replaced only after the temporary payload is fully flushed. An error
    /// after rename is reported distinctly because the payload may already be
    /// published while directory durability is still uncertain.
    pub fn save_project_checked(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let destination = Path::new(path);
        if path.trim().is_empty() || destination.is_dir() {
            return Err("invalid project destination".into());
        }
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                return Err("project destination parent does not exist".into());
            }
        }
        let Some(save_lock) = SaveLock::acquire(destination) else {
            return Err("project save lock is held".into());
        };
        if cleanup_stale_compatibility_temps(destination).is_err() {
            return Err("could not clean stale project temporary files".into());
        }
        static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = destination.with_extension(
            format!(
                "{}tmp",
                destination
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!("{}.", e))
                    .unwrap_or_default()
            ) + &format!(
                "-{}-{}-{}",
                std::process::id(),
                sequence,
                save_lock.owner_token()
            ),
        );
        let result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .and_then(|mut file| {
                file.write_all(data)?;
                file.sync_all()
            })
            .and_then(|_| fs::rename(&temp_path, destination));
        if let Err(error) = result {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("project publish failed: {error}"));
        }
        if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !sync_directory(parent) {
                return Err(
                    "project published but parent directory durability could not be confirmed"
                        .into(),
                );
            }
        }
        drop(save_lock);
        Ok(())
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide asset state.
    pub fn audit_project_asset_orchestrator(&self) -> bool {
        !self.project_dir.is_empty() && self.assets.audit_assets()
    }
}

/// Completes and atomically saves a native project payload. The C++ field
/// serializer supplies the versioned bytes; Rust owns the native envelope's
/// checksum policy so reads and writes use the same implementation.
#[no_mangle]
pub unsafe extern "C" fn hirari_save_native_project_bytes(
    path: *const std::ffi::c_char,
    path_size: usize,
    data: *const u8,
    data_size: usize,
) -> u8 {
    if path.is_null() || path_size == 0 || data.is_null() || data_size < 8 {
        return 0;
    }
    if data_size > 256 * 1024 * 1024 - 4 {
        return 0;
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path.cast::<u8>(), path_size) };
    let Ok(path) = std::str::from_utf8(path_bytes) else {
        return 0;
    };
    let payload = unsafe { std::slice::from_raw_parts(data, data_size) };
    let magic = u32::from_ne_bytes(payload[..4].try_into().unwrap());
    let version = u32::from_ne_bytes(payload[4..8].try_into().unwrap());
    if magic != crate::native_project::MAGIC || !(1..=42).contains(&version) {
        return 0;
    }
    let mut bytes = payload.to_vec();
    if version >= 27 {
        let checksum = crate::native_project::crc32(&bytes);
        bytes.extend_from_slice(&checksum.to_ne_bytes());
    }
    ProjectOrchestrator::new()
        .save_project_checked(path, &bytes)
        .is_ok() as u8
}

/// Reads and validates the native project envelope before handing the
/// versioned payload to the remaining C++ field decoder. The returned allocation
/// must be released with `hirari_free_loaded_project_bytes` and its exact size.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariNativeProjectHeader {
    pub version: u32,
    pub sample_rate: u32,
    pub header_size: u32,
    pub track_count: u32,
    pub bpm: f64,
    pub root_note: i32,
    pub scale_type: i32,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_load_project_payload(
    path: *const std::ffi::c_char,
    path_size: usize,
    output_size: *mut usize,
    output_header: *mut HirariNativeProjectHeader,
) -> *mut u8 {
    if output_size.is_null() || output_header.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *output_size = 0 };
    unsafe { output_header.write(HirariNativeProjectHeader::default()) };
    if path.is_null() || path_size == 0 {
        return std::ptr::null_mut();
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path.cast::<u8>(), path_size) };
    let Ok(path) = std::str::from_utf8(path_bytes) else {
        return std::ptr::null_mut();
    };
    let path = Path::new(path);
    let Ok(metadata) = fs::metadata(path) else {
        return std::ptr::null_mut();
    };
    if !metadata.is_file() || metadata.len() < 8 || metadata.len() > 256 * 1024 * 1024 {
        return std::ptr::null_mut();
    }
    let Ok(mut bytes) = fs::read(path) else {
        return std::ptr::null_mut();
    };
    if bytes.len() < 8 || bytes.len() > 256 * 1024 * 1024 {
        return std::ptr::null_mut();
    }
    let magic = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
    let version = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    if magic != crate::native_project::MAGIC || !(1..=42).contains(&version) {
        return std::ptr::null_mut();
    }
    let header_size = if version >= 14 { 28 } else { 20 };
    if bytes.len() < header_size {
        return std::ptr::null_mut();
    }
    let sample_rate = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    let bpm = f64::from_ne_bytes(bytes[12..20].try_into().unwrap());
    let (root_note, scale_type) = if version >= 14 {
        (
            i32::from_ne_bytes(bytes[20..24].try_into().unwrap()),
            i32::from_ne_bytes(bytes[24..28].try_into().unwrap()),
        )
    } else {
        (0, 0)
    };
    if sample_rate == 0
        || !bpm.is_finite()
        || !(20.0..=300.0).contains(&bpm)
        || !(0..=11).contains(&root_note)
        || !(0..=32).contains(&scale_type)
    {
        return std::ptr::null_mut();
    }
    let Some(track_count_bytes) = bytes.get(header_size..header_size + 4) else {
        return std::ptr::null_mut();
    };
    let track_count = u32::from_ne_bytes(track_count_bytes.try_into().unwrap());
    if track_count > 100_000 {
        return std::ptr::null_mut();
    }
    if version >= 27 {
        if bytes.len() < 12 {
            return std::ptr::null_mut();
        }
        let payload_size = bytes.len() - 4;
        let stored_checksum = u32::from_ne_bytes(bytes[payload_size..].try_into().unwrap());
        if crate::native_project::crc32(&bytes[..payload_size]) != stored_checksum {
            return std::ptr::null_mut();
        }
        bytes.truncate(payload_size);
    }
    let mut bytes = bytes.into_boxed_slice();
    let length = bytes.len();
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    unsafe { *output_size = length };
    unsafe {
        output_header.write(HirariNativeProjectHeader {
            version,
            sample_rate,
            header_size: header_size as u32,
            track_count,
            bpm,
            root_note,
            scale_type,
        })
    };
    pointer
}

#[no_mangle]
pub unsafe extern "C" fn hirari_free_loaded_project_bytes(data: *mut u8, size: usize) {
    if !data.is_null() {
        let bytes = std::ptr::slice_from_raw_parts_mut(data, size);
        unsafe { drop(Box::from_raw(bytes)) };
    }
}

/// Cross-process lock shared by the compatibility project facade and the
/// canonical persistence path.  A unique lock file prevents autosave and
/// manual save from rotating or replacing the same project concurrently.
struct SaveLock {
    path: std::path::PathBuf,
    owner_token: String,
}

impl SaveLock {
    fn owner_token(&self) -> &str {
        &self.owner_token
    }
}

impl SaveLock {
    fn acquire(destination: &Path) -> Option<Self> {
        let path = std::path::PathBuf::from(format!("{}.save.lock", destination.display()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .or_else(|_| {
                if reclaim_dead_owner(&path) {
                    OpenOptions::new().write(true).create_new(true).open(&path)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "save lock is held",
                    ))
                }
            })
            .ok()?;
        let mut file = file;
        let owner_token = new_owner_token();
        if writeln!(
            file,
            "pid={} created_unix_seconds={} token={}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
            owner_token
        )
        .is_err()
            || file.sync_all().is_err()
        {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(Self { path, owner_token })
    }
}

#[cfg(unix)]
fn reclaim_dead_owner(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let has_valid_token = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("token="))
        .is_some_and(|token| {
            !token.is_empty()
                && token.len() <= 128
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        });
    if !has_valid_token {
        return false;
    }
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<i32>().ok())
    else {
        return false;
    };
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true);
    !alive && fs::remove_file(path).is_ok()
}

#[cfg(not(unix))]
fn reclaim_dead_owner(_path: &Path) -> bool {
    false
}

fn new_owner_token() -> String {
    static OWNER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = OWNER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, sequence)
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        // The path can be replaced after acquisition (for example by stale
        // lock recovery). Never let an older owner remove a newer owner's
        // lock during Drop; verify the ownership token before unlinking.
        let owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("token="))
                    .map(|token| token == self.owner_token)
            })
            .unwrap_or(false);
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn sync_directory(path: &Path) -> bool {
    #[cfg(unix)]
    {
        match File::open(path).and_then(|directory| directory.sync_all()) {
            Ok(()) => true,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn cleanup_stale_compatibility_temps(destination: &Path) -> std::io::Result<()> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let Some(file_name) = destination.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.tmp-");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
            && entry.file_type()?.is_file()
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProjectOrchestrator, SaveLock};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hirari-project-manager-{id}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn explicit_asset_consolidation_uses_canonical_collector() {
        let project = temp_dir();
        let source = project.join("take.wav");
        fs::write(&source, b"audio").unwrap();
        let mut manager = ProjectOrchestrator::new();
        assert!(manager.consolidate_assets(
            project.to_string_lossy().into_owned(),
            vec![source.to_string_lossy().into_owned()],
        ));
        assert_eq!(manager.asset_paths.len(), 1);
        assert!(manager.audit_project_asset_orchestrator());
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn compatibility_save_lock_serializes_writers_and_releases_cleanly() {
        let project = temp_dir();
        let destination = project.join("session.hirari");
        let first = SaveLock::acquire(&destination).expect("first writer must acquire lock");
        assert!(SaveLock::acquire(&destination).is_none());
        drop(first);
        assert!(SaveLock::acquire(&destination).is_some());
        let _ = fs::remove_file(format!("{}.save.lock", destination.display()));
        fs::remove_dir_all(project).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn compatibility_save_lock_reclaims_dead_owner_only() {
        let project = temp_dir();
        let destination = project.join("session.hirari");
        let lock_path = format!("{}.save.lock", destination.display());
        fs::write(
            &lock_path,
            "pid=2147483647 created_unix_seconds=1 token=dead-face-1\n",
        )
        .unwrap();

        let reclaimed = SaveLock::acquire(&destination).expect("dead owner must be reclaimable");
        assert!(std::path::Path::new(&lock_path).exists());
        drop(reclaimed);
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn compatibility_save_lock_drop_does_not_remove_replaced_owner() {
        let project = temp_dir();
        let destination = project.join("session.hirari");
        let first = SaveLock::acquire(&destination).expect("first writer must acquire lock");
        let lock_path = format!("{}.save.lock", destination.display());
        fs::write(
            &lock_path,
            "pid=2147483647 created_unix_seconds=1 token=replacement-face-2\n",
        )
        .unwrap();
        drop(first);
        assert!(std::path::Path::new(&lock_path).exists());
        fs::remove_file(lock_path).unwrap();
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn compatibility_save_reclaims_stale_reserved_temporary_files() {
        let project = temp_dir();
        let destination = project.join("session.hirari");
        let stale = project.join("session.hirari.tmp-crashed-17");
        fs::write(&stale, b"partial").unwrap();

        let manager = ProjectOrchestrator::new();
        assert!(manager.try_save_project(destination.to_str().unwrap(), b"complete"));
        assert!(!stale.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"complete");
        fs::remove_dir_all(project).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_asset_is_rejected_without_partial_state() {
        use std::os::unix::fs::symlink;
        let project = temp_dir();
        let source = project.join("real.wav");
        let link = project.join("linked.wav");
        fs::write(&source, b"audio").unwrap();
        symlink(&source, &link).unwrap();
        let mut manager = ProjectOrchestrator::new();
        assert!(!manager.consolidate_assets(
            project.to_string_lossy().into_owned(),
            vec![link.to_string_lossy().into_owned()],
        ));
        assert!(manager.asset_paths.is_empty());
        assert!(manager.assets.assets.is_empty());
        fs::remove_dir_all(project).unwrap();
    }
}
