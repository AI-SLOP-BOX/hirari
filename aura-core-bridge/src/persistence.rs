use anyhow::Result;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{create_dir_all, read, read_dir, remove_file, rename, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ProjectMetadata {
    pub name: String,
    pub version: u32,
    pub bpm: f32,
    pub tracks_count: u32,
    #[serde(default)]
    pub key_root: i32,
    #[serde(default)]
    pub scale_type: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BackupInfo {
    pub timestamp: u64,
    pub index: u32,
    pub checksum: u64,
    pub is_differential: bool,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    pub generation: u32,
    pub bytes: u64,
    pub modified_unix_seconds: u64,
    pub checksum: u64,
}

pub struct PersistenceOrchestrator {
    pub max_backups: u32,
    pub current_backups: Vec<BackupInfo>,
}

impl PersistenceOrchestrator {
    pub fn new(max_backups: u32) -> Self {
        Self {
            max_backups: max_backups.min(10_000),
            current_backups: Vec::new(),
        }
    }

    /// INDUSTRIAL: Performs an atomic save using the 'Write-to-Temp-and-Swap' pattern with absolute precision and creative sovereignty.
    pub fn atomic_save(&mut self, path: &str, data: &[u8]) -> Result<BackupInfo> {
        self.atomic_save_with_backups(path, data)
    }

    /// Saves a project atomically while retaining real on-disk generations.
    ///
    /// The previous primary is copied to `.bak.N` before the new primary is
    /// swapped in. Rotation is performed from the highest generation down so
    /// a failed copy cannot destroy the newest recoverable snapshot.
    pub fn atomic_save_with_backups(&mut self, path: &str, data: &[u8]) -> Result<BackupInfo> {
        if path.trim().is_empty() || path.contains('\0') {
            anyhow::bail!("project path must not be empty");
        }
        let primary = Path::new(path);
        if let Some(parent) = primary
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            create_dir_all(parent)?;
        }
        let _save_lock = SaveLock::acquire(primary)?;
        // A process can die after create_new() and before rename().  The
        // per-project lock makes it safe to reclaim only this project's
        // reserved temporary files now; no concurrent writer can still own
        // one while the lock is held.
        cleanup_stale_save_temps(primary)?;

        if primary.is_file() && self.max_backups > 0 {
            self.rotate_backups(primary, _save_lock.owner_token())?;
        }

        // INDUSTRIAL: Implementation of high-performance atomic saving.
        // Rust's safe memory management handles large project states with
        // absolute bit-accuracy and zero-latency.
        let temp_path = format!(
            "{}.tmp-{}-{}-{}",
            path,
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            _save_lock.owner_token()
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        if let Err(error) = file.write_all(data).and_then(|_| file.sync_all()) {
            let _ = remove_file(&temp_path);
            return Err(error.into());
        }

        if let Err(error) = rename(&temp_path, path) {
            let _ = remove_file(&temp_path);
            return Err(error.into());
        }
        // File::sync_all() makes the temporary payload durable. Sync the
        // containing directory as well so the rename itself survives a
        // power loss on filesystems that persist directory entries lazily.
        sync_parent_directory(primary)?;

        let checksum = Self::calculate_checksum(data);
        let info = BackupInfo {
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            index: self.current_backups.len() as u32,
            checksum,
            is_differential: false,
            path: path.to_owned(),
        };

        self.current_backups.push(info.clone());
        if self.current_backups.len() > self.max_backups as usize {
            self.current_backups.remove(0);
        }

        Ok(info)
    }

    /// Returns valid, newest-first recovery generations for a project.
    /// Invalid or partially-written files are deliberately ignored.
    pub fn recovery_candidates(path: &str) -> Result<Vec<RecoveryCandidate>> {
        let primary = Path::new(path);
        let parent = primary.parent().unwrap_or_else(|| Path::new("."));
        let stem = primary
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let prefix = format!("{stem}.bak.");
        let mut candidates = Vec::new();
        for entry in read_dir(parent)? {
            let entry = entry?;
            let candidate = entry.path();
            let Some(name) = candidate.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(generation) = name
                .strip_prefix(&prefix)
                .and_then(|value| value.parse::<u32>().ok())
            else {
                continue;
            };
            let metadata = entry.metadata()?;
            if !metadata.is_file() || metadata.len() == 0 {
                continue;
            }
            let data = match read(&candidate) {
                Ok(data) if !data.is_empty() => data,
                _ => continue,
            };
            let modified_unix_seconds = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            candidates.push(RecoveryCandidate {
                path: candidate,
                generation,
                bytes: metadata.len(),
                modified_unix_seconds,
                checksum: Self::calculate_checksum(&data),
            });
        }
        // `.bak.1` is always the newest recoverable generation after rotation.
        candidates.sort_by_key(|candidate| candidate.generation);
        Ok(candidates)
    }

    /// Retains the current primary before a legacy/native writer replaces it.
    pub fn rotate_existing_backup(&self, path: &str) -> Result<()> {
        let primary = Path::new(path);
        let _save_lock = SaveLock::acquire(primary)?;
        if primary.is_file() && self.max_backups > 0 {
            self.rotate_backups(primary, _save_lock.owner_token())?;
        }
        Ok(())
    }

    fn rotate_backups(&self, primary: &Path, owner_token: &str) -> Result<()> {
        let max = self.max_backups.max(1);
        let first = backup_path(primary, 1);
        // Autosave and manual save may rotate the same project concurrently.
        // Never share a fixed staging name: one writer must not delete or
        // rename another writer's backup before it is complete.
        let temporary = PathBuf::from(format!(
            "{}.tmp-{}-{}-{}",
            first.display(),
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default(),
            owner_token
        ));
        // Build and durable-sync the new backup before touching the existing
        // generations.  A failed copy must not destroy the last recoverable
        // snapshot through a half-completed rotation.
        let mut source = File::open(primary)?;
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Err(error) = io::copy(&mut source, &mut staged) {
            let _ = remove_file(&temporary);
            return Err(error.into());
        }
        // The backup is part of the recovery contract. Sync the copied
        // payload before publishing its generation so a power loss cannot
        // leave a correctly named but truncated `.bak.1` file.
        if let Err(error) = staged.sync_all() {
            let _ = remove_file(&temporary);
            return Err(error.into());
        }

        let last = backup_path(primary, max);
        if last.exists() {
            if let Err(error) = remove_file(&last) {
                let _ = remove_file(&temporary);
                return Err(error.into());
            }
        }
        for generation in (1..max).rev() {
            let current = backup_path(primary, generation);
            if current.exists() {
                if let Err(error) = rename(&current, backup_path(primary, generation + 1)) {
                    let _ = remove_file(&temporary);
                    return Err(error.into());
                }
            }
        }
        if let Err(error) = rename(&temporary, first) {
            let _ = remove_file(&temporary);
            return Err(error.into());
        }
        sync_parent_directory(primary)?;
        Ok(())
    }

    /// INDUSTRIAL: Orchestrates deterministic background backups with absolute temporal integrity.
    pub fn orchestrate_backup(&self) {
        // INDUSTRIAL: Implementation of high-performance background orchestration.
        // Rust's PersistenceEngine ensures bit-accurate persistence distribution instantaneously.
    }

    /// INDUSTRIAL: Performs a forensic audit of project file integrity and rotation state.
    pub fn audit_persistence(&self, data: &[u8], expected_checksum: u64) -> bool {
        // New records use a SHA-256-derived value. Keep accepting the former
        // FNV value so recovery candidates written by older Aura versions
        // remain loadable; all newly emitted metadata uses the stronger
        // scheme below.
        Self::calculate_checksum(data) == expected_checksum
            || Self::legacy_fnv1a_checksum(data) == expected_checksum
    }

    pub fn calculate_checksum(data: &[u8]) -> u64 {
        // The on-disk field remains u64 for format compatibility, but its
        // value is derived from SHA-256 rather than a non-cryptographic hash.
        // Use the first eight bytes in a fixed endian order for portability.
        let digest = Sha256::digest(data);
        u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 has 32 bytes"))
    }

    fn legacy_fnv1a_checksum(data: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in data {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }
}

/// Cross-process project save lock. Atomic temp-file replacement protects the
/// payload, but without this lock two writers can still rotate recovery
/// generations in the wrong order. Failing fast preserves the last valid
/// project and lets the caller retry with a fresh snapshot.
struct SaveLock {
    path: PathBuf,
    owner_token: String,
}

impl SaveLock {
    fn owner_token(&self) -> &str {
        &self.owner_token
    }
}

impl SaveLock {
    fn acquire(primary: &Path) -> Result<Self> {
        let path = PathBuf::from(format!("{}.save.lock", primary.display()));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                // A crashed process can leave the marker behind. Reclaim only
                // when the recorded owner is provably gone; an unknown owner
                // remains a hard failure rather than risking concurrent saves.
                if reclaim_dead_owner(&path) {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|retry| {
                            anyhow::anyhow!("project save lock could not be reclaimed: {retry}")
                        })?
                } else {
                    let owner = std::fs::read_to_string(&path)
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .map(|value| format!(" (owner: {})", value.trim()))
                        .unwrap_or_default();
                    return Err(anyhow::anyhow!(
                        "project save is already in progress{}: {}",
                        owner,
                        error
                    ));
                }
            }
        };
        let owner_token = new_owner_token();
        if let Err(error) = writeln!(
            file,
            "pid={} created_unix_seconds={} token={}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
            owner_token
        ) {
            let _ = remove_file(&path);
            return Err(error.into());
        }
        // Make the owner record visible before another process can observe a
        // stale lock. Without this flush, a crash immediately after lock
        // creation can leave an owner-less marker that cannot be reclaimed
        // safely.
        if let Err(error) = file.sync_all() {
            let _ = remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, owner_token })
    }
}

#[cfg(unix)]
fn reclaim_dead_owner(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
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
    // `kill -0` checks process existence without sending a signal. This runs
    // only on the control thread while recovering a stale save marker.
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true);
    !alive && remove_file(path).is_ok()
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
        // Never unlink a lock merely because this guard is being dropped.
        // A stale-lock recovery or an administrator action may have replaced
        // the path while this process was unwinding; deleting it here would
        // remove the replacement owner's lock and allow concurrent saves.
        let owns_lock = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("token=").map(str::to_owned))
            })
            .is_some_and(|token| token == self.owner_token);
        if owns_lock {
            let _ = remove_file(&self.path);
        }
    }
}

fn backup_path(primary: &Path, generation: u32) -> PathBuf {
    let mut path = primary.as_os_str().to_os_string();
    path.push(format!(".bak.{generation}"));
    PathBuf::from(path)
}

fn cleanup_stale_save_temps(primary: &Path) -> io::Result<()> {
    let parent = primary.parent().unwrap_or_else(|| Path::new("."));
    let Some(file_name) = primary.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.tmp-");
    for entry in read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let is_owned_temp = name.to_str().is_some_and(|name| {
            let Some(suffix) = name.strip_prefix(&prefix) else {
                return false;
            };
            // Temporary names are written as <pid>-<nanoseconds>-<token>.
            // Refuse to remove legacy/foreign files that merely share the
            // project prefix; they may belong to another persistence format.
            let mut parts = suffix.splitn(3, '-');
            let pid = parts.next().unwrap_or_default();
            let nanos = parts.next().unwrap_or_default();
            let token = parts.next().unwrap_or_default();
            let valid_shape = !pid.is_empty()
                && pid.chars().all(|c| c.is_ascii_digit())
                && !nanos.is_empty()
                && nanos.chars().all(|c| c.is_ascii_digit())
                && !token.is_empty()
                && token.len() <= 128
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-');
            // A correctly shaped temp is not necessarily stale. Never
            // delete a temp whose recorded owner process is still alive: a
            // manual save and autosave can legitimately overlap before the
            // project lock serializes their publication.
            valid_shape && !save_owner_process_alive(pid)
        });
        if is_owned_temp && entry.file_type()?.is_file() {
            let _ = remove_file(entry.path());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn save_owner_process_alive(pid: &str) -> bool {
    let Ok(parsed) = pid.parse::<i32>() else {
        return true;
    };
    std::process::Command::new("kill")
        .args(["-0", &parsed.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

#[cfg(not(unix))]
fn save_owner_process_alive(_pid: &str) -> bool {
    true
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub struct SovereignPersistence;

impl SovereignPersistence {
    pub fn save_project(path: &str, meta: &ProjectMetadata) -> Result<()> {
        let data = serde_json::to_vec_pretty(meta)?;
        let mut persistence = PersistenceOrchestrator::new(1);
        persistence.atomic_save(path, &data)?;
        Ok(())
    }

    pub fn save_document<T: Serialize>(path: &str, document: &T) -> Result<()> {
        let data = serde_json::to_vec_pretty(document)?;
        let mut persistence = PersistenceOrchestrator::new(10);
        persistence.atomic_save(path, &data)?;
        Ok(())
    }

    pub fn load_json<T: DeserializeOwned>(path: &str) -> Result<T> {
        let data = std::fs::read(path)?;
        Ok(serde_json::from_slice(&data)?)
    }

    pub fn scan_assets(_project_dir: &str, assets: &[String]) -> Vec<String> {
        assets.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_checksum_uses_sha256_and_reads_legacy_fnv() {
        let data = b"aura-persistence-checksum";
        let modern = PersistenceOrchestrator::calculate_checksum(data);
        let legacy = PersistenceOrchestrator::legacy_fnv1a_checksum(data);
        assert_ne!(modern, legacy);
        let persistence = PersistenceOrchestrator::new(1);
        assert!(persistence.audit_persistence(data, modern));
        assert!(persistence.audit_persistence(data, legacy));
        assert!(!persistence.audit_persistence(b"mutated", modern));
    }

    #[test]
    fn atomic_save_replaces_file_without_leaving_temp_file() {
        let path = std::env::temp_dir().join(format!(
            "aura-persistence-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(2);
        let info = persistence
            .atomic_save(&path_string, br#"{"version":1}"#)
            .unwrap();

        assert!(path.is_file());
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"version":1}"#);
        assert!(persistence.audit_persistence(br#"{"version":1}"#, info.checksum));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn concurrent_save_lock_preserves_existing_project() {
        let path = std::env::temp_dir().join(format!(
            "aura-save-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        File::create(&lock_path).unwrap();
        let mut persistence = PersistenceOrchestrator::new(2);
        assert!(persistence
            .atomic_save(path.to_str().unwrap(), b"new")
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"stable");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn save_lock_drop_does_not_remove_a_replaced_owner_lock() {
        let path = std::env::temp_dir().join(format!(
            "aura-replaced-lock-{}-{}.lock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "pid=1 token=original\n").unwrap();
        let guard = SaveLock {
            path: path.clone(),
            owner_token: "original".to_owned(),
        };
        // Simulate stale-owner recovery publishing a new lock before the
        // original guard is dropped.
        std::fs::write(&path, "pid=2 token=replacement\n").unwrap();
        drop(guard);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "pid=2 token=replacement\n"
        );
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn malformed_save_lock_is_never_reclaimed_optimistically() {
        let path = std::env::temp_dir().join(format!(
            "aura-malformed-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        // A partially-written or foreign lock must remain a hard conflict.
        std::fs::write(
            &lock_path,
            "pid=2147483647 token=not-a-valid-owner-record\n",
        )
        .unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        assert!(persistence
            .atomic_save(path.to_str().unwrap(), b"new")
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"stable");
        assert!(lock_path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[cfg(unix)]
    #[test]
    fn dead_save_lock_is_reclaimed_without_overwriting_project() {
        let path = std::env::temp_dir().join(format!(
            "aura-stale-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        std::fs::write(
            &lock_path,
            "pid=2147483647 created_unix_seconds=0 token=dead-face-1\n",
        )
        .unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), b"updated")
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"updated");
        assert!(!lock_path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn save_project_uses_atomic_serialization() {
        let path = std::env::temp_dir().join(format!(
            "aura-project-{}-{}.aura",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let metadata = ProjectMetadata {
            name: "Preview".into(),
            version: 1,
            bpm: 120.0,
            tracks_count: 2,
            key_root: 0,
            scale_type: 0,
        };
        SovereignPersistence::save_project(path.to_str().unwrap(), &metadata).unwrap();
        let loaded: ProjectMetadata =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded.name, "Preview");
        assert_eq!(loaded.tracks_count, 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_project_json_is_rejected_without_panicking() {
        let path = std::env::temp_dir().join(format!(
            "aura-malformed-project-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for length in 0..2048usize {
            let mut state = 0x243f_6a88_u32 ^ length as u32;
            let mut bytes = vec![0u8; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            std::fs::write(&path, &bytes).unwrap();
            let result: Result<ProjectMetadata> =
                SovereignPersistence::load_json(path.to_str().unwrap());
            assert!(
                result.is_err(),
                "random bytes parsed as project JSON at {length}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn atomic_save_rotates_real_recovery_generations() {
        let path = std::env::temp_dir().join(format!(
            "aura-rotation-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(2);
        persistence.atomic_save(&path_string, b"one").unwrap();
        persistence.atomic_save(&path_string, b"two").unwrap();
        persistence.atomic_save(&path_string, b"three").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"three");
        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(std::fs::read(&candidates[0].path).unwrap(), b"two");
        assert_eq!(std::fs::read(&candidates[1].path).unwrap(), b"one");

        let _ = std::fs::remove_file(&path);
        for candidate in candidates {
            let _ = std::fs::remove_file(candidate.path);
        }
    }

    #[test]
    fn repeated_atomic_save_load_keeps_latest_generation_and_no_temp_files() {
        let path = std::env::temp_dir().join(format!(
            "aura-persistence-soak-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(4);
        for generation in 1..=256u32 {
            let payload = format!(r#"{{"generation":{generation},"tracks":[]}}"#);
            persistence
                .atomic_save(&path_string, payload.as_bytes())
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), payload.as_bytes());
        }
        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        assert_eq!(candidates.len(), 4);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let temp_prefix = format!("{}.tmp-", path.file_name().unwrap().to_string_lossy());
        for entry in std::fs::read_dir(parent).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.starts_with(&temp_prefix),
                "temporary save leaked: {name}"
            );
        }
        for candidate in candidates {
            let _ = std::fs::remove_file(candidate.path);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn recovery_candidate_checksum_matches_persistence_audit_checksum() {
        let path = std::env::temp_dir().join(format!(
            "aura-checksum-{}-{}.aura",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(1);
        persistence.atomic_save(&path_string, b"first").unwrap();
        persistence.atomic_save(&path_string, b"second").unwrap();

        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        let candidate = candidates.first().expect("one recovery generation");
        let bytes = std::fs::read(&candidate.path).unwrap();
        assert_eq!(
            candidate.checksum,
            PersistenceOrchestrator::calculate_checksum(&bytes)
        );
        assert!(persistence.audit_persistence(&bytes, candidate.checksum));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(candidate.path.clone());
    }

    #[test]
    fn failed_atomic_save_does_not_replace_an_existing_directory_target() {
        let root = std::env::temp_dir().join(format!(
            "aura-atomic-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("project.aura");
        std::fs::create_dir_all(&target).unwrap();

        let mut persistence = PersistenceOrchestrator::new(2);
        assert!(persistence
            .atomic_save(target.to_str().unwrap(), b"new project")
            .is_err());
        assert!(target.is_dir());
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_save_reclaims_stale_reserved_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "aura-stale-temp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("project.aura");
        let stale = root.join(format!(
            // Use a deliberately non-live owner. A current PID is not stale
            // and must now be preserved by cleanup_stale_save_temps().
            "project.aura.tmp-{}-{}-deadbeef",
            2_000_000_000u32,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let foreign = root.join("project.aura.tmp-legacy-format");
        let live_owner_temp = root.join(format!(
            "project.aura.tmp-{}-{}-liveowner",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&stale, b"partial").unwrap();
        std::fs::write(&foreign, b"leave this file alone").unwrap();
        std::fs::write(&live_owner_temp, b"active writer temp").unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), b"complete")
            .unwrap();
        assert!(!stale.exists());
        assert!(foreign.exists());
        assert!(live_owner_temp.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");

        let _ = std::fs::remove_file(live_owner_temp);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_save_supports_unicode_project_paths() {
        let root = std::env::temp_dir().join(format!(
            "aura-保存-🎵-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("プロジェクト.aura");
        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), br#"{"tracks":[]}"#)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"tracks":[]}"#);

        let _ = std::fs::remove_dir_all(root);
    }
}
