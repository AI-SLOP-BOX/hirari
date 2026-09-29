use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CueStatus {
    Pending,
    Recorded,
    Approved,
    Rejected,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdrCue {
    pub id: u32,
    pub start_frame: u64,
    pub end_frame: u64,
    pub speaker: String,
    pub text: String,
    pub status: CueStatus,
    pub notes: String,
}
impl AdrCue {
    pub fn validate(&self) -> bool {
        self.id != 0
            && self.start_frame < self.end_frame
            && self.speaker.len() <= 128
            && !self.speaker.trim().is_empty()
            && !self.speaker.contains('\0')
            && self.text.len() <= 4096
            && !self.text.contains('\0')
            && self.notes.len() <= 4096
            && !self.notes.contains('\0')
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdrCueSheet {
    pub cues: Vec<AdrCue>,
}
impl AdrCueSheet {
    pub fn validate(&self) -> bool {
        self.cues.len() <= 100_000
            && self.cues.iter().all(AdrCue::validate)
            && self
                .cues
                .iter()
                .enumerate()
                .all(|(i, c)| self.cues[..i].iter().all(|p| p.id != c.id))
            && self
                .cues
                .windows(2)
                .all(|w| w[0].start_frame < w[1].start_frame)
    }
    pub fn upsert(&mut self, cue: AdrCue) -> bool {
        if !cue.validate() {
            return false;
        }
        if let Some(existing) = self.cues.iter_mut().find(|existing| existing.id == cue.id) {
            *existing = cue;
        } else {
            self.cues.push(cue);
        }
        self.cues.sort_by_key(|cue| cue.start_frame);
        self.validate()
    }
    pub fn set_status(&mut self, id: u32, status: CueStatus) -> bool {
        self.cues
            .iter_mut()
            .find(|c| c.id == id)
            .map(|c| {
                c.status = status;
                true
            })
            .unwrap_or(false)
    }
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.cues.len();
        self.cues.retain(|cue| cue.id != id);
        before != self.cues.len()
    }
    pub fn range(&self, start: u64, end: u64) -> Vec<AdrCue> {
        if end <= start {
            return Vec::new();
        }
        self.cues
            .iter()
            .filter(|c| c.start_frame < end && c.end_frame > start)
            .cloned()
            .collect()
    }
    pub fn search(&self, query: &str) -> Vec<AdrCue> {
        let q = query.trim().to_ascii_lowercase();
        self.cues
            .iter()
            .filter(|cue| {
                q.is_empty()
                    || cue.speaker.to_ascii_lowercase().contains(&q)
                    || cue.text.to_ascii_lowercase().contains(&q)
                    || cue.notes.to_ascii_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manages_adr_cues() {
        let mut sheet = AdrCueSheet::default();
        assert!(sheet.upsert(AdrCue {
            id: 1,
            start_frame: 10,
            end_frame: 20,
            speaker: "A".into(),
            text: "Line".into(),
            status: CueStatus::Pending,
            notes: String::new()
        }));
        assert!(sheet.set_status(1, CueStatus::Approved));
        assert_eq!(sheet.range(15, 16).len(), 1);
        assert!(sheet.validate());
    }
}
