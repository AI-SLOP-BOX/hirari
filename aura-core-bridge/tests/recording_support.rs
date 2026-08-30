use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

#[allow(dead_code)]
pub fn remove_test_file(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if path.exists() {
        std::fs::remove_file(path)
            .unwrap_or_else(|error| panic!("test cleanup failed for {}: {error}", path.display()));
    }
}

pub struct NativeEngineGuard {
    path: PathBuf,
    _process_guard: MutexGuard<'static, ()>,
}

impl Drop for NativeEngineGuard {
    fn drop(&mut self) {
        std::env::remove_var("AURA_NATIVE_TEST_ISOLATION");
        let _ = std::fs::remove_file(&self.path);
    }
}

fn owner_is_alive(path: &Path) -> bool {
    let owner = match std::fs::read_to_string(path) {
        Ok(value) => value.trim().parse::<u32>().ok(),
        Err(_) => None,
    };
    let Some(pid) = owner else { return false };
    #[cfg(unix)]
    {
        return Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub fn native_engine_test_guard() -> NativeEngineGuard {
    // Integration-test binaries run tests concurrently in one process. The
    // filesystem lock alone cannot serialize those threads because they all
    // share the same PID and environment. Pair it with a process-local guard.
    static PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let process_guard = PROCESS_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        // A failing test must not poison the process-wide serialization guard
        // for every subsequent integration test in this binary. The native
        // engine is reset by each guard, so recovering the guard is safe and
        // preserves the original failure without cascading PoisonErrors.
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // A single create_new file makes acquisition and owner publication
    // atomic. The previous directory-then-owner-file protocol had a race
    // where another test process could reclaim the empty directory between
    // those two operations.
    let path = std::env::temp_dir().join("aura-core-native-engine.lock");
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut owner_file) => {
                owner_file
                    .write_all(std::process::id().to_string().as_bytes())
                    .expect("native engine lock owner must be writable");
                owner_file
                    .sync_all()
                    .expect("native engine lock owner must be durable");
                std::env::set_var("AURA_NATIVE_TEST_ISOLATION", "1");
                return NativeEngineGuard {
                    path,
                    _process_guard: process_guard,
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale_without_owner = std::fs::read_to_string(&path).is_err()
                    && std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .map(|age| age > Duration::from_secs(30))
                        .unwrap_or(false);
                if stale_without_owner || !owner_is_alive(&path) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                std::thread::yield_now();
            }
            Err(error) => panic!("cannot acquire native engine test lock: {error}"),
        }
    }
}

#[allow(dead_code)]
pub fn remove_runtime_region_ids(layout: &mut serde_json::Value) {
    if let Some(tracks) = layout.as_array_mut() {
        for track in tracks {
            if let Some(regions) = track
                .get_mut("regions")
                .and_then(serde_json::Value::as_array_mut)
            {
                for region in regions {
                    if let Some(object) = region.as_object_mut() {
                        object.remove("id");
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub fn pcm16_rms(payload: &[u8]) -> f64 {
    let (sum, count) = payload[44..]
        .chunks_exact(2)
        .map(|sample| {
            let value = i16::from_le_bytes([sample[0], sample[1]]) as f64;
            value * value
        })
        .fold((0.0, 0usize), |(sum, count), value| {
            (sum + value, count + 1)
        });
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt()
    }
}

#[allow(dead_code)]
pub fn pcm16_samples(payload: &[u8]) -> Vec<i16> {
    payload[44..]
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect()
}
