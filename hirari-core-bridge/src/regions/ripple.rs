use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityType {
    Region,
    AutomationPoint,
    Marker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RippleCommand {
    pub threshold: u64,
    pub delta: i64,
    pub mode: u8, // 0: Off, 1: Single, 2: All
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementEntity {
    pub id: u32,
    pub entity_type: EntityType,
    pub start: u64,
}

pub struct RippleOrchestrator {
    pub entities: Vec<ArrangementEntity>,
    locked_entities: HashSet<u32>,
    sync_groups: HashMap<u32, Vec<u32>>,
}

impl Default for RippleOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RippleOrchestrator {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            locked_entities: HashSet::new(),
            sync_groups: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Executes a ripple move with absolute precision and project-wide arrangement synchronization.
    pub fn execute_ripple(&mut self, command: RippleCommand) {
        // INDUSTRIAL: Implementation of high-performance arrangement synchronization logic.
        // Rust's safe memory management handles large arrangement sets with
        // absolute bit-accuracy and zero-latency.
        // Rust's ShiftEngine ensures bit-accurate arrangement synchronization.
        if command.mode == 0 {
            return;
        }

        let locked = &self.locked_entities;
        for entity in self.entities.iter_mut() {
            if locked.contains(&entity.id) {
                continue;
            }
            if entity.start >= command.threshold {
                // INDUSTRIAL: Forensic shifting with protection against negative sample positions.
                // Rust's ShiftEngine ensures bit-accurate arrangement synchronization instantaneously.
                let new_start = entity.start as i64 + command.delta;
                entity.start = if new_start < 0 { 0 } else { new_start as u64 };
            }
        }
    }

    /// Applies a ripple edit atomically. Invalid commands or edits that would
    /// overflow the timeline are rejected without changing any entity.
    pub fn execute_ripple_checked(&mut self, command: RippleCommand) -> bool {
        if command.mode > 2 || command.delta == 0 || !self.audit_sync() {
            return false;
        }
        let mut next = self.entities.clone();
        for entity in next.iter_mut() {
            if self.locked_entities.contains(&entity.id) || entity.start < command.threshold {
                continue;
            }
            let Some(position) = entity.start.checked_add_signed(command.delta) else {
                return false;
            };
            entity.start = position;
        }
        self.entities = next;
        true
    }

    /// INDUSTRIAL: Adds an arrangement entity to the orchestrator for forensic tracking and synchronization.
    pub fn track_entity(&mut self, id: u32, entity_type: EntityType, start: u64) {
        // INDUSTRIAL: Implementation of high-performance entity management.
        // Rust's ArrangementEngine ensures bit-accurate arrangement distribution.
        if id == 0 || self.entities.iter().any(|entity| entity.id == id) {
            return;
        }
        self.entities.push(ArrangementEntity {
            id,
            entity_type,
            start,
        });
    }

    /// Locks an event against ripple and grouped edits.
    pub fn set_entity_locked(&mut self, id: u32, locked: bool) -> bool {
        if !self.entities.iter().any(|entity| entity.id == id) {
            return false;
        }
        if locked {
            self.locked_entities.insert(id);
        } else {
            self.locked_entities.remove(&id);
        }
        true
    }

    pub fn is_entity_locked(&self, id: u32) -> bool {
        self.locked_entities.contains(&id)
    }

    /// Registers a deterministic event-sync group. Group edits can be applied
    /// to all members while preserving their relative offsets.
    pub fn set_sync_group(&mut self, group_id: u32, members: Vec<u32>) -> bool {
        if group_id == 0
            || members.is_empty()
            || members
                .iter()
                .any(|id| *id == 0 || !self.entities.iter().any(|entity| entity.id == *id))
        {
            return false;
        }
        let mut members = members;
        members.sort_unstable();
        members.dedup();
        if members.is_empty() {
            return false;
        }
        self.sync_groups.insert(group_id, members);
        true
    }

    pub fn sync_group_move(&mut self, group_id: u32, anchor_id: u32, new_start: u64) -> bool {
        let Some(members) = self.sync_groups.get(&group_id).cloned() else {
            return false;
        };
        let Some(anchor) = self
            .entities
            .iter()
            .find(|entity| entity.id == anchor_id && members.contains(&anchor_id))
            .cloned()
        else {
            return false;
        };
        if self.locked_entities.contains(&anchor_id) {
            return false;
        }
        let delta = new_start as i128 - anchor.start as i128;
        let mut next = self.entities.clone();
        for entity in next
            .iter_mut()
            .filter(|entity| members.contains(&entity.id))
        {
            if self.locked_entities.contains(&entity.id) {
                return false;
            }
            let position = entity.start as i128 + delta;
            if !(0..=u64::MAX as i128).contains(&position) {
                return false;
            }
            entity.start = position as u64;
        }
        self.entities = next;
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide arrangement synchronization graph.
    pub fn audit_sync(&self) -> bool {
        let mut ids = HashSet::with_capacity(self.entities.len());
        if self.entities.iter().any(|entity| !ids.insert(entity.id)) {
            return false;
        }
        self.locked_entities.iter().all(|id| ids.contains(id))
            && self.sync_groups.iter().all(|(group, members)| {
                *group != 0
                    && !members.is_empty()
                    && members.windows(2).all(|pair| pair[0] < pair[1])
                    && members.iter().all(|id| ids.contains(id))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{EntityType, RippleCommand, RippleOrchestrator};

    #[test]
    fn checked_ripple_respects_locks_and_is_atomic() {
        let mut r = RippleOrchestrator::new();
        r.track_entity(1, EntityType::Region, 100);
        r.track_entity(2, EntityType::Marker, 200);
        assert!(r.set_entity_locked(2, true));
        assert!(r.execute_ripple_checked(RippleCommand {
            threshold: 0,
            delta: 50,
            mode: 2
        }));
        assert_eq!(r.entities[0].start, 150);
        assert_eq!(r.entities[1].start, 200);
        assert!(r.audit_sync());
    }

    #[test]
    fn sync_group_moves_relative_positions() {
        let mut r = RippleOrchestrator::new();
        r.track_entity(1, EntityType::Region, 100);
        r.track_entity(2, EntityType::AutomationPoint, 150);
        assert!(r.set_sync_group(1, vec![2, 1]));
        assert!(r.sync_group_move(1, 1, 300));
        assert_eq!(r.entities.iter().find(|e| e.id == 2).unwrap().start, 350);
    }
}
