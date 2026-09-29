use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArrangerSection {
    pub id: u32,
    pub name: String,
    pub start_beat: u64,
    pub length_beats: u64,
    pub repeats: u16,
    pub enabled: bool,
}
impl ArrangerSection {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && self.length_beats > 0
            && self.length_beats <= 1_000_000
            && self.repeats > 0
            && self.repeats <= 1024
    }
}

pub fn expand_arrangement(sections: &[ArrangerSection]) -> Option<Vec<ArrangerSection>> {
    if sections.len() > 1024
        || sections
            .iter()
            .any(|section| section.id == 0 || !section.validate())
    {
        return None;
    }
    let mut out = Vec::new();
    let mut cursor = 0u64;
    let mut used_ids = std::collections::HashSet::new();
    let mut next_id = sections
        .iter()
        .map(|section| section.id)
        .max()
        .unwrap_or(0)
        .max(1);
    for s in sections.iter().filter(|s| s.enabled) {
        for _ in 0..s.repeats {
            let mut x = s.clone();
            x.id = loop {
                next_id = next_id.checked_add(1)?;
                if used_ids.insert(next_id) {
                    break next_id;
                }
            };
            x.start_beat = cursor;
            x.repeats = 1;
            out.push(x);
            cursor = cursor.checked_add(s.length_beats)?;
        }
    }
    Some(out)
}

pub fn insert_section(sections: &mut Vec<ArrangerSection>, section: ArrangerSection) -> bool {
    if sections.len() >= 1024
        || section.id == 0
        || !section.validate()
        || sections.iter().any(|existing| existing.id == section.id)
    {
        return false;
    }
    let backup = sections.clone();
    sections.push(section);
    if validate_arrangement(sections) {
        true
    } else {
        *sections = backup;
        false
    }
}

pub fn remove_section(sections: &mut Vec<ArrangerSection>, id: u32) -> bool {
    let before = sections.len();
    sections.retain(|section| section.id != id);
    before != sections.len()
}

pub fn duplicate_section(sections: &mut Vec<ArrangerSection>, id: u32, new_id: u32) -> bool {
    if new_id == 0 || sections.len() >= 1024 || sections.iter().any(|section| section.id == new_id)
    {
        return false;
    }
    let Some(source) = sections.iter().find(|section| section.id == id).cloned() else {
        return false;
    };
    let mut copy = source;
    copy.id = new_id;
    let Some(next_start) = copy.start_beat.checked_add(copy.length_beats) else {
        return false;
    };
    copy.start_beat = next_start;
    let backup = sections.clone();
    sections.push(copy);
    if validate_arrangement(sections) {
        true
    } else {
        *sections = backup;
        false
    }
}

pub fn validate_arrangement(sections: &[ArrangerSection]) -> bool {
    sections.len() <= 1024
        && sections
            .iter()
            .all(|section| section.id != 0 && section.validate())
        && sections.iter().enumerate().all(|(index, section)| {
            sections[..index]
                .iter()
                .all(|previous| previous.id != section.id)
        })
        && !arrangement_has_overlaps(sections)
}
pub fn normalize_arrangement(sections: &mut [ArrangerSection]) -> bool {
    if sections.iter().any(|s| !s.validate() || s.id == 0)
        || sections
            .iter()
            .enumerate()
            .any(|(i, s)| sections[..i].iter().any(|p| p.id == s.id))
    {
        return false;
    }
    sections.sort_by_key(|s| s.start_beat);
    true
}
pub fn arrangement_has_overlaps(sections: &[ArrangerSection]) -> bool {
    let mut ordered: Vec<ArrangerSection> = sections
        .iter()
        .filter(|section| section.enabled)
        .cloned()
        .collect();
    if !normalize_arrangement(&mut ordered) {
        return true;
    }
    ordered
        .windows(2)
        .any(|w| w[0].start_beat.saturating_add(w[0].length_beats) > w[1].start_beat)
}
pub fn reorder_arrangement(sections: &mut Vec<ArrangerSection>, order: &[u32]) -> bool {
    if order.is_empty()
        || order.len() != sections.len()
        || order
            .iter()
            .enumerate()
            .any(|(i, id)| order[..i].contains(id))
        || order.iter().any(|id| !sections.iter().any(|s| s.id == *id))
    {
        return false;
    }
    let mut result = Vec::with_capacity(sections.len());
    for id in order {
        let Some(section) = sections.iter().find(|s| s.id == *id).cloned() else {
            return false;
        };
        result.push(section);
    }
    let mut cursor = 0u64;
    for section in &mut result {
        section.start_beat = cursor;
        let Some(next) = cursor.checked_add(section.length_beats) else {
            return false;
        };
        cursor = next;
    }
    *sections = result;
    true
}
pub fn set_section_enabled(sections: &mut [ArrangerSection], id: u32, enabled: bool) -> bool {
    sections
        .iter_mut()
        .find(|s| s.id == id)
        .map(|s| {
            s.enabled = enabled;
            true
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expands_repeated_sections() {
        let s = ArrangerSection {
            id: 1,
            name: "Verse".into(),
            start_beat: 0,
            length_beats: 16,
            repeats: 2,
            enabled: true,
        };
        let out = expand_arrangement(&[s]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].start_beat, 16);
        assert_ne!(out[0].id, out[1].id);
    }
    #[test]
    fn detects_overlapping_sections() {
        let s = [
            ArrangerSection {
                id: 1,
                name: "A".into(),
                start_beat: 0,
                length_beats: 16,
                repeats: 1,
                enabled: true,
            },
            ArrangerSection {
                id: 2,
                name: "B".into(),
                start_beat: 8,
                length_beats: 8,
                repeats: 1,
                enabled: true,
            },
        ];
        assert!(arrangement_has_overlaps(&s));
    }
    #[test]
    fn edits_sections_with_unique_ids() {
        let mut sections = Vec::new();
        assert!(insert_section(
            &mut sections,
            ArrangerSection {
                id: 1,
                name: "Intro".into(),
                start_beat: 0,
                length_beats: 8,
                repeats: 1,
                enabled: true
            }
        ));
        assert!(duplicate_section(&mut sections, 1, 2));
        assert!(!duplicate_section(&mut sections, 1, 2));
        assert!(validate_arrangement(&sections));
        assert!(remove_section(&mut sections, 2));
        assert!(validate_arrangement(&sections));
    }
}
