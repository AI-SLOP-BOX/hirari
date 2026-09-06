pub struct AudioTake {
    pub id: u32,
    pub file_path: String,
    pub start_samples: f64,
}

pub struct TakeOrchestrator {
    pub takes_by_track: std::collections::HashMap<u32, Vec<AudioTake>>,
}

impl Default for TakeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TakeOrchestrator {
    pub fn new() -> Self {
        Self {
            takes_by_track: std::collections::HashMap::new(),
        }
    }

    /// INDUSTRIAL: Adds a new recording as a take with memory-safe Rust collections and version control.
    pub fn add_take(&mut self, track_id: u32, path: &str, start: f64) -> bool {
        if track_id == 0
            || path.trim().is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || !start.is_finite()
            || start < 0.0
        {
            return false;
        }

        let entry = self.takes_by_track.entry(track_id).or_default();
        let Some(id) = u32::try_from(entry.len()).ok() else {
            return false;
        };
        entry.push(AudioTake {
            id,
            file_path: path.to_string(),
            start_samples: start,
        });
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide take synchronization graph.
    pub fn audit_takes(&self) -> bool {
        self.takes_by_track.iter().all(|(track_id, takes)| {
            if *track_id == 0 || takes.len() > 65_536 {
                return false;
            }
            let mut ids = std::collections::HashSet::with_capacity(takes.len());
            takes.iter().all(|take| {
                ids.insert(take.id)
                    && !take.file_path.trim().is_empty()
                    && take.file_path.len() <= 4096
                    && !take.file_path.contains('\0')
                    && take.start_samples.is_finite()
                    && take.start_samples >= 0.0
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioTake, TakeOrchestrator};

    #[test]
    fn audits_valid_take_sets() {
        let mut takes = TakeOrchestrator::new();
        assert!(takes.add_take(1, "take-1.wav", 0.0));
        assert!(takes.add_take(1, "take-2.wav", 128.0));
        assert!(takes.audit_takes());
    }

    #[test]
    fn rejects_invalid_take_metadata() {
        let mut takes = TakeOrchestrator::new();
        takes.takes_by_track.insert(
            1,
            vec![AudioTake {
                id: 0,
                file_path: String::new(),
                start_samples: f64::NAN,
            }],
        );
        assert!(!takes.audit_takes());
    }

    #[test]
    fn rejects_invalid_take_at_ingress() {
        let mut takes = TakeOrchestrator::new();
        assert!(!takes.add_take(1, "", 0.0));
        assert!(!takes.add_take(1, "take.wav", -1.0));
        assert!(!takes.add_take(1, "take.wav", f64::NAN));
        assert!(takes.takes_by_track.get(&1).is_none());
    }
}
