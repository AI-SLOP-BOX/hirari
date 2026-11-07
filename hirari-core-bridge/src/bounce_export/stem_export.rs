//! One file-naming policy for every stem export.
//!
//! The GUI's `EXPORT STEMS` and the CLI's `hirari render stems` used to keep
//! their own sanitizers.  Only the GUI understood Windows device names, and
//! only the GUI detected collisions the way the destination filesystem does,
//! so one project could publish different files depending on which surface
//! exported it: a track named `CON` reached the CLI as the unusable
//! `CON.wav`, and `Drums`/`DRUMS` both reached the CLI as two paths that
//! collapse onto a single inode on a case-insensitive volume (default macOS
//! APFS included) — silently dropping one stem.
//!
//! Both surfaces derive their names here instead.

use std::collections::HashSet;

/// Windows reserves these device names even when they carry an extension, so
/// a stem matching one is prefixed before it can reach the filesystem.
const WINDOWS_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Derives the file stem (no extension) one exported track writes to.
///
/// `used` accumulates the stems this batch has already claimed **as lowercase
/// strings**, so names differing only in case are treated as the collision a
/// case-insensitive volume turns them into.  The returned stem keeps the
/// track's original casing.
pub fn safe_stem_name(name: &str, track_id: u32, used: &mut HashSet<String>) -> String {
    let base: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let base = if base.is_empty() {
        format!("track-{track_id}")
    } else {
        base
    };

    let mut suffix = 1u32;
    loop {
        let stem = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let candidate = if WINDOWS_DEVICE_NAMES.contains(&stem.to_ascii_uppercase().as_str()) {
            format!("track-{stem}")
        } else {
            stem
        };
        if used.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

/// Core-owned queue for one stems export.
///
/// Both surfaces step this instead of keeping a loop of their own: `hirari
/// render stems` steps it around the engine's blocking bounce, and the GUI's
/// `EXPORT STEMS` steps it from its telemetry timer around the engine's own
/// bounce thread.  Selection, naming, output validation and the rollback of a
/// partly written batch therefore exist once, in core, instead of once per
/// surface.
///
/// The queue only advances when a stem has been *published*, so "which track
/// is next" and "which files to delete if this fails" are the same fact and
/// cannot drift apart.
#[derive(Debug, Default)]
pub struct StemBatchState {
    /// `(track id, destination)` in project order.
    tracks: Vec<(u32, std::path::PathBuf)>,
    /// Files this batch published; removed together if any stem fails.
    published: Vec<std::path::PathBuf>,
    /// Container the surface should render each stem into. Kept here so the
    /// completion document can report the batch as it actually ran, instead
    /// of a surface re-reading its own UI selection after the fact.
    format: u32,
}

impl StemBatchState {
    pub fn new(tracks: Vec<(u32, std::path::PathBuf)>, format: u32) -> Self {
        Self {
            tracks,
            published: Vec::new(),
            format,
        }
    }

    pub fn format(&self) -> u32 {
        self.format
    }

    /// The next stem to arm, or `None` once every track has been rendered.
    pub fn next_target(&self) -> Option<(u32, std::path::PathBuf)> {
        self.tracks
            .get(self.published.len())
            .map(|(track_id, path)| (*track_id, path.clone()))
    }

    /// Records one published stem so the queue moves to the next track.
    pub fn advance(&mut self, rendered: std::path::PathBuf) {
        self.published.push(rendered);
    }

    /// `(rendered, total)` for progress reporting.
    pub fn progress(&self) -> (u32, u32) {
        (self.published.len() as u32, self.tracks.len() as u32)
    }

    pub fn is_complete(&self) -> bool {
        self.next_target().is_none()
    }

    /// Destinations published so far, in render order.
    pub fn outputs(&self) -> Vec<String> {
        self.published
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    /// Removes everything this batch published, so a failing batch never
    /// leaves a partial set of stems on disk.
    pub fn rollback(&mut self) {
        for path in self.published.drain(..) {
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Could not remove partial stem {}: {error}", path.display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::safe_stem_name;
    use std::collections::HashSet;

    #[test]
    fn keeps_track_characters_and_replaces_the_rest() {
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("Kick-01_Lead", 7, &mut used), "Kick-01_Lead");

        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("a/b\\c:d", 7, &mut used), "a_b_c_d");
    }

    #[test]
    fn falls_back_to_the_track_id_only_when_nothing_survives() {
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("", 7, &mut used), "track-7");

        // Sanitized to underscores, which is still a usable (if opaque) name.
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("///", 7, &mut used), "___");
    }

    #[test]
    fn prefixes_windows_device_names_in_any_case() {
        for reserved in ["CON", "con", "PRN", "aux", "NUL", "COM1", "com9", "LPT9"] {
            let mut used = HashSet::new();
            assert_eq!(
                safe_stem_name(reserved, 3, &mut used),
                format!("track-{reserved}"),
                "{reserved} must not be exported under its reserved name"
            );
        }
    }

    #[test]
    fn suffixes_a_second_claim_on_the_same_name() {
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("Drums", 2, &mut used), "Drums");
        assert_eq!(safe_stem_name("Drums", 3, &mut used), "Drums-2");
        assert_eq!(safe_stem_name("Drums", 4, &mut used), "Drums-3");
    }

    #[test]
    fn treats_case_only_differences_as_one_file() {
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("Drums", 2, &mut used), "Drums");
        // A case-sensitive set would hand the renderer `DRUMS.wav` as well,
        // and the volume would resolve both paths onto `Drums.wav`.
        assert_eq!(safe_stem_name("DRUMS", 3, &mut used), "DRUMS-2");
    }

    #[test]
    fn suffixes_a_second_device_name_that_is_not_itself_reserved() {
        let mut used = HashSet::new();
        assert_eq!(safe_stem_name("CON", 2, &mut used), "track-CON");
        assert_eq!(safe_stem_name("CON", 3, &mut used), "CON-2");
    }
}
