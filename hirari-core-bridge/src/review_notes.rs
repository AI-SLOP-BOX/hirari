use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewStatus {
    Open,
    Resolved,
    Rejected,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewNote {
    pub id: u64,
    pub author: String,
    pub text: String,
    pub timestamp_ms: u64,
    pub status: ReviewStatus,
    pub history: Vec<ReviewStatus>,
}

#[derive(Default)]
pub struct ReviewNoteStore {
    notes: Vec<ReviewNote>,
    next_id: u64,
}
impl ReviewNoteStore {
    pub fn new() -> Self {
        Self {
            notes: Vec::new(),
            next_id: 1,
        }
    }
    pub fn add(&mut self, author: &str, text: &str, timestamp_ms: u64) -> Option<u64> {
        if author.trim().is_empty()
            || text.trim().is_empty()
            || author.len() > 128
            || text.len() > 16_384
            || author.contains('\0')
            || text.contains('\0')
            || self.notes.len() >= 65_536
        {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        self.notes.push(ReviewNote {
            id,
            author: author.trim().to_owned(),
            text: text.trim().to_owned(),
            timestamp_ms,
            status: ReviewStatus::Open,
            history: vec![ReviewStatus::Open],
        });
        Some(id)
    }
    pub fn set_status(&mut self, id: u64, status: ReviewStatus) -> bool {
        let Some(note) = self.notes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        if note.status == status {
            return false;
        }
        note.status = status.clone();
        note.history.push(status);
        true
    }
    pub fn snapshot(&self) -> &[ReviewNote] {
        &self.notes
    }
    pub fn by_status(&self, status: ReviewStatus) -> Vec<&ReviewNote> {
        let mut out: Vec<_> = self
            .notes
            .iter()
            .filter(|note| note.status == status)
            .collect();
        out.sort_by_key(|note| (note.timestamp_ms, note.id));
        out
    }
    pub fn search(&self, query: &str) -> Vec<&ReviewNote> {
        let q = query.trim().to_ascii_lowercase();
        let mut out: Vec<_> = self
            .notes
            .iter()
            .filter(|note| {
                q.is_empty()
                    || note.author.to_ascii_lowercase().contains(&q)
                    || note.text.to_ascii_lowercase().contains(&q)
            })
            .collect();
        out.sort_by_key(|note| (note.timestamp_ms, note.id));
        out
    }
    pub fn snapshot_json(&self) -> Result<String, String> {
        if !self.audit() {
            return Err("invalid review state".into());
        }
        serde_json::to_string(&self.notes).map_err(|e| e.to_string())
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        let notes: Vec<ReviewNote> =
            serde_json::from_str(json).map_err(|error| format!("invalid review notes: {error}"))?;
        let next_id = notes
            .iter()
            .map(|note| note.id)
            .max()
            .and_then(|id| id.checked_add(1))
            .unwrap_or(1);
        let store = Self { notes, next_id };
        store
            .audit()
            .then_some(store)
            .ok_or_else(|| "review notes failed validation".into())
    }
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.notes.len();
        self.notes.retain(|note| note.id != id);
        before != self.notes.len()
    }
    pub fn clear(&mut self) {
        self.notes.clear();
        self.next_id = 1;
    }
    pub fn audit(&self) -> bool {
        self.next_id > 0
            && self.notes.len() <= 65_536
            && self.notes.iter().all(|n| {
                n.id > 0
                    && n.id < self.next_id
                    && !n.author.trim().is_empty()
                    && !n.author.contains('\0')
                    && !n.text.trim().is_empty()
                    && !n.text.contains('\0')
                    && n.author.len() <= 128
                    && n.text.len() <= 16_384
                    && !n.history.is_empty()
                    && n.history.last() == Some(&n.status)
            })
            && self
                .notes
                .iter()
                .enumerate()
                .all(|(i, n)| self.notes[..i].iter().all(|previous| previous.id != n.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_status_is_auditable() {
        let mut s = ReviewNoteStore::new();
        let id = s.add("a", "fix timing", 1).unwrap();
        assert!(s.set_status(id, ReviewStatus::Resolved));
        assert_eq!(s.snapshot()[0].history.len(), 2);
        assert!(!s.set_status(99, ReviewStatus::Rejected));
    }
    #[test]
    fn review_notes_round_trip_and_remove() {
        let mut s = ReviewNoteStore::new();
        let id = s.add("editor", "check vocal", 10).unwrap();
        let json = s.snapshot_json().unwrap();
        let mut restored = ReviewNoteStore::from_json(&json).unwrap();
        assert_eq!(restored.snapshot()[0].id, id);
        assert!(restored.remove(id));
        assert!(restored.snapshot().is_empty());
        assert!(restored.audit());
    }
}
