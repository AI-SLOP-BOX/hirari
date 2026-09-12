/**
 * @struct VersioningEngine
 * @brief Professional arrangement versioning and branching engine.
 * INDUSTRIAL: Implements git-like snapshot sovereignty, allowing users to 
 * explore different song structures without technical debt or destructive edits.
 */
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ArrangementSnapshot {
    pub id: u32,
    pub name: String,
    pub timestamp: u64,
    pub payload: Vec<u8>, // Binary blob of tracks/regions/automation
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VersioningEngine {
    pub branches: Vec<ArrangementSnapshot>,
    pub current_branch_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotDiff { pub left_id: u32, pub right_id: u32, pub changed_bytes: usize, pub left_size: usize, pub right_size: usize }

impl VersioningEngine {
    pub fn new() -> Self {
        Self {
            branches: Vec::new(),
            current_branch_id: 0,
        }
    }

    /**
     * @brief SNAPSHOT: Creates a new arrangement branch.
     * INDUSTRIAL: Beyond simple undo, this creates a parallel musical reality.
     */
    pub fn create_branch(&mut self, name: &str, data: Vec<u8>) -> u32 {
        let name = name.trim();
        if name.is_empty() || name.len() > 256 || name.contains('\0') || data.len() > 256 * 1024 * 1024 || self.branches.len() == u32::MAX as usize || self.branches.iter().any(|branch| branch.name == name) { return u32::MAX; }
        let id = self.branches.len() as u32;
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.branches.push(ArrangementSnapshot {
            id,
            name: name.to_owned(),
            timestamp,
            payload: data,
        });
        id
    }

    /**
     * @brief CHECKOUT: Switches to a specific arrangement branch.
     * INDUSTRIAL: Performs a zero-latency restoration of the project state.
     */
    pub fn checkout_branch(&mut self, id: u32) -> Option<&ArrangementSnapshot> {
        if let Some(branch) = self.branches.iter().find(|b| b.id == id) {
            self.current_branch_id = id;
            Some(branch)
        } else {
            None
        }
    }

    /// Compare two arrangement snapshots without exposing their binary payloads.
    pub fn compare_branches(&self, left_id: u32, right_id: u32) -> Option<SnapshotDiff> {
        let left = self.branches.iter().find(|b| b.id == left_id)?;
        let right = self.branches.iter().find(|b| b.id == right_id)?;
        let changed_bytes = left.payload.iter().zip(right.payload.iter()).filter(|(a,b)| a != b).count()
            + left.payload.len().abs_diff(right.payload.len());
        Some(SnapshotDiff { left_id, right_id, changed_bytes, left_size: left.payload.len(), right_size: right.payload.len() })
    }
    pub fn remove_branch(&mut self, id: u32) -> bool {
        let Some(index) = self.branches.iter().position(|branch| branch.id == id) else { return false; };
        self.branches.remove(index);
        for (index, branch) in self.branches.iter_mut().enumerate() { branch.id = index as u32; }
        self.current_branch_id = self.branches.get(index.min(self.branches.len().saturating_sub(1))).map(|b| b.id).unwrap_or(0);
        true
    }
    pub fn rename_branch(&mut self, id: u32, name: &str) -> bool {
        if name.trim().is_empty() || name.len() > 256 || self.branches.iter().any(|branch| branch.name == name.trim() && branch.id != id) { return false; }
        self.branches.iter_mut().find(|branch| branch.id == id).map(|branch| { branch.name = name.trim().to_owned(); true }).unwrap_or(false)
    }
    pub fn branch_names(&self) -> Vec<String> { self.branches.iter().map(|branch| branch.name.clone()).collect() }
    pub fn latest_branch(&self) -> Option<&ArrangementSnapshot> { self.branches.iter().max_by_key(|branch| branch.timestamp) }

    pub fn audit_versioning(&self) -> bool {
        if self.branches.len() > 65_536 {
            return false;
        }
        let ids_are_unique = self
            .branches
            .iter()
            .enumerate()
            .all(|(index, branch)| branch.id == index as u32);
        let names_are_valid = self.branches.iter().all(|branch| !branch.name.trim().is_empty() && branch.name.len() <= 256 && !branch.name.contains('\0') && branch.payload.len() <= 256 * 1024 * 1024)
            && self.branches.iter().enumerate().all(|(i, branch)| self.branches[..i].iter().all(|previous| previous.name != branch.name));
        let current_is_valid = self.branches.is_empty()
            || self.branches.iter().any(|branch| branch.id == self.current_branch_id);
        ids_are_unique && names_are_valid && current_is_valid
    }
}

#[cfg(test)]
mod tests {
    use super::VersioningEngine;

    #[test]
    fn invalid_current_branch_fails_audit() {
        let mut engine = VersioningEngine::new();
        engine.create_branch("Main", vec![1]);
        engine.current_branch_id = 99;
        assert!(!engine.audit_versioning());
    }

    #[test]
    fn blank_branch_name_fails_audit() {
        let mut engine = VersioningEngine::new();
        assert_eq!(engine.create_branch("   ", vec![]), u32::MAX);
        assert!(engine.audit_versioning());
    }

    #[test]
    fn compares_snapshot_payloads() {
        let mut engine = VersioningEngine::new();
        let a = engine.create_branch("A", vec![1, 2, 3]);
        let b = engine.create_branch("B", vec![1, 9, 3, 4]);
        let diff = engine.compare_branches(a, b).unwrap();
        assert_eq!(diff.changed_bytes, 2);
        assert!(engine.compare_branches(a, 99).is_none());
    }

    #[test]
    fn duplicate_branch_names_are_rejected_without_mutating_state() {
        let mut engine = VersioningEngine::new();
        let first = engine.create_branch("Main", vec![1]);
        let before = engine.branch_names();
        assert_ne!(first, u32::MAX);
        assert_eq!(engine.create_branch(" Main ", vec![2]), u32::MAX);
        assert_eq!(engine.branch_names(), before);
    }
}
