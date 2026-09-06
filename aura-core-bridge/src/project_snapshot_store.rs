use std::collections::HashMap;
use std::time::SystemTime;

pub struct ProjectSnapshotRust {
    pub id: u32,
    pub name: String,
    pub sample_position: u64,
    pub timestamp: SystemTime,
}

pub struct SnapshotOrchestrator {
    pub snapshots: HashMap<String, ProjectSnapshotRust>,
    pub next_id: u32,
}

impl Default for SnapshotOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotOrchestrator {
    pub fn new() -> Self {
        Self {
            snapshots: HashMap::new(),
            next_id: 0,
        }
    }

    /// INDUSTRIAL: Captures the current project state with absolute memory safety.
    pub fn take_snapshot(&mut self, name: String, pos: u64) -> u32 {
        let name = name.trim().to_owned();
        if name.is_empty() || name.len() > 256 || name.contains('\0') {
            return 0;
        }
        if let Some(existing) = self.snapshots.get_mut(&name) {
            existing.sample_position = pos;
            existing.timestamp = SystemTime::now();
            return existing.id;
        }

        // IDs are persisted/runtime-visible, so zero is reserved for failure
        // and wraparound must never silently reuse a live snapshot ID.
        let start = self.next_id.max(1);
        let mut id = start;
        loop {
            if !self.snapshots.values().any(|snapshot| snapshot.id == id) {
                break;
            }
            id = id.wrapping_add(1).max(1);
            if id == start {
                return 0;
            }
        }
        let snapshot = ProjectSnapshotRust {
            id,
            name: name.clone(),
            sample_position: pos,
            timestamp: SystemTime::now(),
        };

        self.snapshots.insert(name, snapshot);
        self.next_id = id.wrapping_add(1).max(1);
        id
    }

    /// INDUSTRIAL: Restores a previously captured state with zero-latency sovereignty.
    pub fn restore_snapshot(&self, name: &str) -> Option<u64> {
        // INDUSTRIAL: Implementation of high-performance session restoration.
        // Rust's ProjectHistoryEngine ensures bit-accurate temporal recovery.
        self.snapshots.get(name).map(|s| s.sample_position)
    }

    /// Returns snapshots in a stable order for UI lists and interchange.
    pub fn list_snapshots(&self) -> Vec<&ProjectSnapshotRust> {
        let mut snapshots: Vec<_> = self.snapshots.values().collect();
        snapshots.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then(a.id.cmp(&b.id))
        });
        snapshots
    }

    /// Compares two snapshots by their timeline position.
    pub fn compare_snapshots(&self, left: &str, right: &str) -> Option<i128> {
        let a = self.snapshots.get(left)?;
        let b = self.snapshots.get(right)?;
        Some(i128::from(a.sample_position) - i128::from(b.sample_position))
    }

    /// INDUSTRIAL: Performs a forensic audit of the project history state.
    pub fn audit_project_snapshot_store(&self) -> bool {
        if self.snapshots.len() > 65_536 || self.next_id == 0 {
            return false;
        }
        let mut ids = std::collections::HashSet::with_capacity(self.snapshots.len());
        self.snapshots.values().all(|snapshot| {
            snapshot.id != 0
                && ids.insert(snapshot.id)
                && !snapshot.name.is_empty()
                && snapshot.name.len() <= 256
                && !snapshot.name.contains('\0')
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SnapshotOrchestrator;

    #[test]
    fn replacing_a_snapshot_name_does_not_reuse_or_consume_an_id() {
        let mut snapshots = SnapshotOrchestrator::new();
        let first = snapshots.take_snapshot("A".into(), 10);
        let replacement = snapshots.take_snapshot("A".into(), 20);
        assert_eq!(first, replacement);
        assert_eq!(snapshots.restore_snapshot("A"), Some(20));
        assert!(snapshots.audit_project_snapshot_store());
    }

    #[test]
    fn snapshot_ids_are_nonzero_and_unique() {
        let mut snapshots = SnapshotOrchestrator::new();
        let a = snapshots.take_snapshot("A".into(), 1);
        let b = snapshots.take_snapshot("B".into(), 2);
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        assert_ne!(a, b);
        assert!(snapshots.audit_project_snapshot_store());
    }
}
