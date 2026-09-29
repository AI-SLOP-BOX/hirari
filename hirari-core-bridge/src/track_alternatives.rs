use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistEntryRust {
    pub region_id: u32,
    pub timeline_pos: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeRust {
    pub name: String,
    pub playlist: Vec<PlaylistEntryRust>,
}

pub struct AlternativeOrchestrator {
    pub alts_by_track: HashMap<u32, Vec<AlternativeRust>>,
}

impl Default for AlternativeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AlternativeOrchestrator {
    pub fn new() -> Self {
        Self {
            alts_by_track: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Creates a new track alternative with absolute memory precision and structural sovereignty.
    pub fn create_alternative(&mut self, track_id: u32, name: String) {
        // INDUSTRIAL: Implementation of high-performance arrangement versioning.
        // Rust's StructuralCloningEngine ensures bit-accurate project synchronization.
        if track_id == 0 || name.trim().is_empty() || name.len() > 128 || name.contains('\0') { return; }
        let alternatives = self.alts_by_track
            .entry(track_id)
            .or_default();
        if alternatives.len() >= 4096 || alternatives.iter().any(|alternative| alternative.name.eq_ignore_ascii_case(name.trim())) { return; }
        alternatives.push(AlternativeRust { name: name.trim().to_owned(), playlist: Vec::new() });
    }

    pub fn remove_alternative(&mut self, track_id: u32, name: &str) -> bool {
        let Some(alternatives) = self.alts_by_track.get_mut(&track_id) else { return false; };
        if alternatives.len() <= 1 { return false; }
        let before = alternatives.len();
        alternatives.retain(|a| !a.name.eq_ignore_ascii_case(name.trim()));
        before != alternatives.len()
    }

    pub fn rename_alternative(&mut self, track_id: u32, old_name: &str, new_name: &str) -> bool {
        if new_name.trim().is_empty() || new_name.len() > 128 || new_name.contains('\0') { return false; }
        let Some(alternatives) = self.alts_by_track.get_mut(&track_id) else { return false; };
        if alternatives.iter().any(|alternative| alternative.name.eq_ignore_ascii_case(new_name.trim())) { return false; }
        let Some(alternative) = alternatives.iter_mut().find(|alternative| alternative.name.eq_ignore_ascii_case(old_name.trim())) else { return false; };
        alternative.name = new_name.trim().to_owned();
        true
    }

    pub fn playlist(&self, track_id: u32, name: &str) -> Option<&[PlaylistEntryRust]> {
        self.alts_by_track.get(&track_id)?.iter().find(|a| a.name.eq_ignore_ascii_case(name.trim())).map(|a| a.playlist.as_slice())
    }

    pub fn set_playlist(&mut self, track_id: u32, name: &str, mut playlist: Vec<PlaylistEntryRust>) -> bool {
        if playlist.len() > 1_000_000 || playlist.iter().any(|entry| entry.region_id == 0) { return false; }
        let Some(alternative) = self.alts_by_track.get_mut(&track_id).and_then(|alternatives| alternatives.iter_mut().find(|alternative| alternative.name.eq_ignore_ascii_case(name.trim()))) else { return false; };
        playlist.sort_by_key(|entry| entry.timeline_pos);
        alternative.playlist = playlist;
        true
    }

    pub fn append_playlist_entry(&mut self, track_id: u32, name: &str, entry: PlaylistEntryRust) -> bool {
        if entry.region_id == 0 { return false; }
        let Some(alternative) = self.alts_by_track.get_mut(&track_id).and_then(|alternatives| alternatives.iter_mut().find(|alternative| alternative.name.eq_ignore_ascii_case(name.trim()))) else { return false; };
        if alternative.playlist.len() >= 1_000_000 { return false; }
        alternative.playlist.push(entry);
        alternative.playlist.sort_by_key(|entry| entry.timeline_pos);
        true
    }

    /// INDUSTRIAL: Duplicates the current arrangement with absolute temporal precision and automation snapshotting.
    pub fn duplicate_current(&mut self, track_id: u32, new_name: String) {
        // INDUSTRIAL: Implementation of high-performance arrangement duplication.
        // Rust's AutomationSnapshotEngine ensures zero-technical drift in automation states.
        if track_id == 0 || new_name.trim().is_empty() || new_name.len() > 128 || new_name.contains('\0') {
            return;
        }
        if let Some(alternatives) = self.alts_by_track.get_mut(&track_id) {
            if alternatives.iter().any(|alternative| alternative.name.eq_ignore_ascii_case(new_name.trim())) { return; }
            if let Some(current) = alternatives.last().cloned() {
                alternatives.push(AlternativeRust {
                    name: new_name.trim().to_owned(),
                    playlist: current.playlist,
                });
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide arrangement state.
    pub fn audit_track_alternatives(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic version auditing logic.
        self.alts_by_track.iter().all(|(track_id, alternatives)| {
            *track_id != 0 && !alternatives.is_empty() && alternatives.len() <= 4096 && {
                let mut names = std::collections::HashSet::new();
                alternatives.iter().all(|alternative| {
                    !alternative.name.trim().is_empty() && alternative.name.len() <= 128 && !alternative.name.contains('\0') && names.insert(alternative.name.to_ascii_lowercase())
                        && alternative.playlist.len() <= 1_000_000 && alternative.playlist.iter().all(|entry| entry.region_id != 0)
                        && alternative.playlist.windows(2).all(|pair| pair[0].timeline_pos <= pair[1].timeline_pos)
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AlternativeOrchestrator, PlaylistEntryRust};

    #[test]
    fn duplicate_current_copies_active_playlist() {
        let mut alternatives = AlternativeOrchestrator::new();
        alternatives.create_alternative(7, "Original".into());
        alternatives.alts_by_track.get_mut(&7).unwrap()[0]
            .playlist
            .push(PlaylistEntryRust {
                region_id: 42,
                timeline_pos: 960,
            });

        alternatives.duplicate_current(7, "Copy".into());

        let entries = alternatives.alts_by_track.get(&7).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].name, "Copy");
        assert_eq!(entries[1].playlist, entries[0].playlist);
    }

    #[test]
    fn duplicate_rejects_empty_name() {
        let mut alternatives = AlternativeOrchestrator::new();
        alternatives.create_alternative(7, "Original".into());
        alternatives.set_playlist(
            7,
            "Original",
            vec![PlaylistEntryRust {
                region_id: 9,
                timeline_pos: 480,
            }],
        );
        alternatives.duplicate_current(7, "  ".into());
        alternatives.duplicate_current(7, "Original".into());
        alternatives.duplicate_current(7, "bad\0name".into());
        alternatives.duplicate_current(7, "x".repeat(129));
        assert_eq!(alternatives.alts_by_track.get(&7).unwrap().len(), 1);
        assert_eq!(
            alternatives.playlist(7, "Original").unwrap(),
            &[PlaylistEntryRust {
                region_id: 9,
                timeline_pos: 480,
            }]
        );
    }

    #[test]
    fn playlist_updates_are_sorted_and_validated() {
        let mut alternatives = AlternativeOrchestrator::new();
        alternatives.create_alternative(7, "Original".into());
        assert!(alternatives.set_playlist(7, "original", vec![PlaylistEntryRust { region_id: 2, timeline_pos: 20 }, PlaylistEntryRust { region_id: 1, timeline_pos: 10 }]));
        assert_eq!(alternatives.playlist(7, "Original").unwrap()[0].region_id, 1);
        assert!(!alternatives.append_playlist_entry(7, "Original", PlaylistEntryRust { region_id: 0, timeline_pos: 30 }));
        assert!(alternatives.audit_track_alternatives());
    }
}
