//! Output-path policy for offline renders. Shared by the Slint surface and
//! the AmeUI export card so both surfaces produce the same naming scheme
//! (`hirari_master-<pid>-<millis>-<seq>.wav` in temp) and keep the same
//! uniqueness guarantees.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RENDER_PATH: AtomicU64 = AtomicU64::new(1);

pub(crate) fn default_render_output_path() -> PathBuf {
    let sequence = NEXT_RENDER_PATH.fetch_add(1, Ordering::Relaxed);
    let timestamp = unix_time_millis();
    std::env::temp_dir().join(format!(
        "hirari_master-{}-{timestamp}-{sequence}.wav",
        std::process::id()
    ))
}

pub(crate) fn unix_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}
