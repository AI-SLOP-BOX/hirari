//! Deterministic MIDI Logical Editor primitives.
//!
//! Rules are deliberately data-only so the same operation can be invoked by
//! the desktop UI, command API, project history, or a future preset browser.

use crate::project_contracts::MidiNoteContract;
use serde::{Deserialize, Serialize};

include!("midi_logical_rules.rs");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogicalEventKind { Note, Controller, ProgramChange, PitchBend, ChannelPressure, PolyPressure }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogicalMidiEvent {
    pub id: u64,
    pub kind: LogicalEventKind,
    pub channel: u8,
    pub position: u64,
    pub length: u64,
    pub main_value: i32,
    pub secondary_value: i32,
    pub selected: bool,
    pub muted: bool,
    pub note_expression: Vec<(u32, i32)>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterTarget { Kind, Channel, Position, Length, MainValue, SecondaryValue, Selected, Muted }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterOperator { Equal, NotEqual, Less, LessOrEqual, Greater, GreaterOrEqual, InsideRange, OutsideRange }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterCondition { pub target: FilterTarget, pub operator: FilterOperator, pub value1: i64, pub value2: i64 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterExpression {
    Condition(FilterCondition),
    And(Vec<FilterExpression>),
    Or(Vec<FilterExpression>),
    Not(Box<FilterExpression>),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionTarget { Channel, Position, Length, MainValue, SecondaryValue }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionOperation {
    Add, Subtract, Multiply, Divide, RoundBy, SetRandomBetween, SetRelativeRandomBetween,
    SetFixed, Mirror, AddLength, MoveToCursor, LinearRamp, RelativeRamp,
    TransposeToScale, RemoveNoteExpression,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogicalAction {
    pub target: ActionTarget,
    pub operation: ActionOperation,
    pub parameter1: f64,
    pub parameter2: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogicalFunction { Transform, Insert, InsertExclusive, Delete, Select, Deselect, Extract, Copy }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogicalEditorPresetPro {
    pub name: String,
    pub filter: FilterExpression,
    pub function: LogicalFunction,
    pub actions: Vec<LogicalAction>,
    pub cursor_position: u64,
    pub loop_range: Option<(u64, u64)>,
    pub random_seed: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogicalEditorReport {
    pub matched: usize,
    pub changed: usize,
    pub extracted: Vec<LogicalMidiEvent>,
}

impl LogicalEditorPresetPro {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 256 || self.name.contains('\0') {
            return Err("logical editor preset name is invalid".into());
        }
        self.filter.validate(0)?;
        if self.actions.len() > 128 { return Err("logical editor has too many actions".into()); }
        if self.function == LogicalFunction::Transform && self.actions.is_empty() {
            return Err("transform function requires actions".into());
        }
        if matches!(self.function, LogicalFunction::Delete | LogicalFunction::Select | LogicalFunction::Deselect)
            && !self.actions.is_empty() {
            return Err("actions are not valid for this logical function".into());
        }
        if self.loop_range.is_some_and(|(start, end)| start >= end) {
            return Err("logical editor loop range is invalid".into());
        }
        for action in &self.actions { action.validate(self.loop_range)?; }
        Ok(())
    }

    /// Applies the complete preset atomically. Random operations use a local,
    /// deterministic generator so preset results can be reproduced and undone.
    pub fn apply(&self, events: &mut Vec<LogicalMidiEvent>) -> Result<LogicalEditorReport, String> {
        self.validate()?;
        if events.len() > 1_000_000 || !events.iter().all(LogicalMidiEvent::validate)
            || events.iter().enumerate().any(|(index, event)| events[..index].iter().any(|previous| previous.id == event.id)) {
            return Err("logical editor input events are invalid".into());
        }
        let matched_indices: Vec<usize> = events.iter().enumerate().filter(|(_, event)| self.filter.matches(event))
            .map(|(index, _)| index).collect();
        let mut report = LogicalEditorReport { matched: matched_indices.len(), ..Default::default() };
        let mut candidate = events.clone();
        match self.function {
            LogicalFunction::Transform => {
                let mut random = DeterministicRandom::new(self.random_seed);
                for &index in &matched_indices {
                    let before = candidate[index].clone();
                    for action in &self.actions {
                        action.apply(&mut candidate[index], self.cursor_position, self.loop_range, &mut random)?;
                    }
                    if !candidate[index].validate() { return Err(format!("logical action made event {} invalid", candidate[index].id)); }
                    report.changed += usize::from(candidate[index] != before);
                }
            }
            LogicalFunction::Insert => {
                let mut random = DeterministicRandom::new(self.random_seed);
                let mut next_id = candidate.iter().map(|event| event.id).max().unwrap_or(0).checked_add(1)
                    .ok_or_else(|| "logical insert id overflow".to_owned())?;
                let mut inserts = Vec::with_capacity(matched_indices.len());
                for index in matched_indices {
                    let mut copy = candidate[index].clone(); copy.id = next_id;
                    next_id = next_id.checked_add(1).ok_or_else(|| "logical insert id overflow".to_owned())?;
                    self.apply_actions(&mut copy, &mut random)?; inserts.push(copy);
                }
                report.changed = inserts.len(); candidate.extend(inserts);
            }
            LogicalFunction::InsertExclusive => {
                let mut random = DeterministicRandom::new(self.random_seed);
                let mut retained = Vec::with_capacity(matched_indices.len());
                for index in matched_indices {
                    let mut event = candidate[index].clone(); self.apply_actions(&mut event, &mut random)?; retained.push(event);
                }
                report.changed = candidate.len(); candidate = retained;
            }
            LogicalFunction::Delete => {
                let ids: std::collections::BTreeSet<_> = matched_indices.iter().map(|index| candidate[*index].id).collect();
                candidate.retain(|event| !ids.contains(&event.id)); report.changed = ids.len();
            }
            LogicalFunction::Select | LogicalFunction::Deselect => {
                let selected = self.function == LogicalFunction::Select;
                for index in matched_indices { if candidate[index].selected != selected { candidate[index].selected = selected; report.changed += 1; } }
            }
            LogicalFunction::Extract => {
                let mut random = DeterministicRandom::new(self.random_seed);
                let ids: std::collections::BTreeSet<_> = matched_indices.iter().map(|index| candidate[*index].id).collect();
                for index in matched_indices { let mut event = candidate[index].clone(); self.apply_actions(&mut event, &mut random)?; report.extracted.push(event); }
                candidate.retain(|event| !ids.contains(&event.id)); report.changed = report.extracted.len();
            }
            LogicalFunction::Copy => {
                let mut random = DeterministicRandom::new(self.random_seed);
                let mut next_id = candidate.iter().map(|event| event.id).max().unwrap_or(0).checked_add(1)
                    .ok_or_else(|| "logical copy id overflow".to_owned())?;
                for index in matched_indices {
                    let mut copy = candidate[index].clone(); copy.id = next_id;
                    next_id = next_id.checked_add(1).ok_or_else(|| "logical copy id overflow".to_owned())?;
                    self.apply_actions(&mut copy, &mut random)?; report.extracted.push(copy);
                }
                report.changed = report.extracted.len();
            }
        }
        candidate.sort_by_key(|event| (event.position, event.id));
        *events = candidate;
        Ok(report)
    }


    fn apply_actions(&self, event: &mut LogicalMidiEvent, random: &mut DeterministicRandom) -> Result<(), String> {
        for action in &self.actions { action.apply(event, self.cursor_position, self.loop_range, random)?; }
        if event.validate() { Ok(()) } else { Err(format!("logical action made event {} invalid", event.id)) }
    }
}

impl FilterExpression {
    fn validate(&self, depth: usize) -> Result<(), String> {
        if depth > 32 { return Err("logical filter nesting is too deep".into()); }
        match self {
            Self::Condition(condition) => condition.validate(),
            Self::And(children) | Self::Or(children) => {
                if children.is_empty() || children.len() > 256 { return Err("logical boolean group is empty or too large".into()); }
                for child in children { child.validate(depth + 1)?; }
                Ok(())
            }
            Self::Not(child) => child.validate(depth + 1),
        }
    }

    fn matches(&self, event: &LogicalMidiEvent) -> bool {
        match self {
            Self::Condition(condition) => condition.matches(event),
            Self::And(children) => children.iter().all(|child| child.matches(event)),
            Self::Or(children) => children.iter().any(|child| child.matches(event)),
            Self::Not(child) => !child.matches(event),
        }
    }
}

impl FilterCondition {
    fn validate(&self) -> Result<(), String> {
        if matches!(self.operator, FilterOperator::InsideRange | FilterOperator::OutsideRange) && self.value1 > self.value2 {
            return Err("logical filter range is reversed".into());
        }
        let boolean_target = matches!(self.target, FilterTarget::Selected | FilterTarget::Muted);
        if boolean_target && !(0..=1).contains(&self.value1) { return Err("logical boolean filter value is invalid".into()); }
        let kind_target = self.target == FilterTarget::Kind;
        if kind_target && !(0..=5).contains(&self.value1) { return Err("logical event type is invalid".into()); }
        Ok(())
    }

    fn matches(&self, event: &LogicalMidiEvent) -> bool {
        let value = match self.target {
            FilterTarget::Kind => event.kind as i64,
            FilterTarget::Channel => i64::from(event.channel),
            FilterTarget::Position => event.position.min(i64::MAX as u64) as i64,
            FilterTarget::Length => event.length.min(i64::MAX as u64) as i64,
            FilterTarget::MainValue => i64::from(event.main_value),
            FilterTarget::SecondaryValue => i64::from(event.secondary_value),
            FilterTarget::Selected => i64::from(event.selected),
            FilterTarget::Muted => i64::from(event.muted),
        };
        match self.operator {
            FilterOperator::Equal => value == self.value1,
            FilterOperator::NotEqual => value != self.value1,
            FilterOperator::Less => value < self.value1,
            FilterOperator::LessOrEqual => value <= self.value1,
            FilterOperator::Greater => value > self.value1,
            FilterOperator::GreaterOrEqual => value >= self.value1,
            FilterOperator::InsideRange => (self.value1..=self.value2).contains(&value),
            FilterOperator::OutsideRange => !(self.value1..=self.value2).contains(&value),
        }
    }
}

impl LogicalAction {
    fn validate(&self, loop_range: Option<(u64, u64)>) -> Result<(), String> {
        if !self.parameter1.is_finite() || !self.parameter2.is_finite() { return Err("logical action parameter is not finite".into()); }
        if matches!(self.operation, ActionOperation::Divide | ActionOperation::RoundBy) && self.parameter1 == 0.0 {
            return Err("logical action divisor cannot be zero".into());
        }
        if matches!(self.operation, ActionOperation::SetRandomBetween | ActionOperation::SetRelativeRandomBetween)
            && self.parameter1 > self.parameter2 { return Err("logical random range is reversed".into()); }
        if matches!(self.operation, ActionOperation::LinearRamp | ActionOperation::RelativeRamp) && loop_range.is_none() {
            return Err("logical ramp requires a loop range".into());
        }
        if matches!(self.operation, ActionOperation::AddLength) && self.target != ActionTarget::Position {
            return Err("add length only applies to position".into());
        }
        if self.operation == ActionOperation::TransposeToScale
            && (self.target != ActionTarget::MainValue || !(0.0..=11.0).contains(&self.parameter1)
                || !matches!(self.parameter2 as i32, 0..=2) || self.parameter2.fract() != 0.0) {
            return Err("transpose-to-scale parameters are invalid".into());
        }
        Ok(())
    }

    fn apply(&self, event: &mut LogicalMidiEvent, cursor: u64, loop_range: Option<(u64, u64)>, random: &mut DeterministicRandom)
        -> Result<(), String> {
        if self.operation == ActionOperation::RemoveNoteExpression {
            if event.kind != LogicalEventKind::Note { return Err("note expression can only be removed from notes".into()); }
            event.note_expression.clear(); return Ok(());
        }
        let current = event.target_value(self.target) as f64;
        let value = match self.operation {
            ActionOperation::Add => current + self.parameter1,
            ActionOperation::Subtract => current - self.parameter1,
            ActionOperation::Multiply => current * self.parameter1,
            ActionOperation::Divide => current / self.parameter1,
            ActionOperation::RoundBy => (current / self.parameter1).round() * self.parameter1,
            ActionOperation::SetRandomBetween => random.between(self.parameter1, self.parameter2),
            ActionOperation::SetRelativeRandomBetween => current + random.between(self.parameter1, self.parameter2),
            ActionOperation::SetFixed => self.parameter1,
            ActionOperation::Mirror => self.parameter1 * 2.0 - current,
            ActionOperation::AddLength => event.position.saturating_add(event.length) as f64,
            ActionOperation::MoveToCursor => cursor as f64,
            ActionOperation::LinearRamp | ActionOperation::RelativeRamp => {
                let (start, end) = loop_range.ok_or_else(|| "logical ramp requires loop range".to_owned())?;
                let ratio = if event.position <= start { 0.0 } else if event.position >= end { 1.0 }
                    else { (event.position - start) as f64 / (end - start) as f64 };
                let ramp = self.parameter1 + (self.parameter2 - self.parameter1) * ratio;
                if self.operation == ActionOperation::RelativeRamp { current + ramp } else { ramp }
            }
            ActionOperation::TransposeToScale => nearest_scale_pitch(current, self.parameter1 as u8, self.parameter2 as u8)? as f64,
            ActionOperation::RemoveNoteExpression => unreachable!(),
        };
        event.set_target_value(self.target, value)
    }
}

fn nearest_scale_pitch(value: f64, root: u8, scale_type: u8) -> Result<u8, String> {
    let note = value.round();
    if !(0.0..=127.0).contains(&note) { return Err("transpose-to-scale pitch is invalid".into()); }
    let intervals: &[u8] = match scale_type { 0 => &[0, 2, 4, 5, 7, 9, 11],
        1 => &[0, 2, 3, 5, 7, 9, 11], 2 => &[0, 2, 3, 5, 7, 8, 11], _ => return Err("unknown logical scale".into()) };
    let note = note as u8;
    for distance in 0..=127i16 {
        let lower = i16::from(note) - distance;
        if lower >= 0 && intervals.contains(&(((lower as u8) + 12 - root) % 12)) { return Ok(lower as u8); }
        let upper = i16::from(note) + distance;
        if upper <= 127 && intervals.contains(&(((upper as u8) + 12 - root) % 12)) { return Ok(upper as u8); }
    }
    Err("scale has no reachable pitch".into())
}

impl LogicalMidiEvent {
    fn validate(&self) -> bool {
        self.id != 0 && self.channel < 16 && self.length <= u64::MAX - self.position
            && match self.kind {
                LogicalEventKind::Note => (0..=127).contains(&self.main_value) && (1..=127).contains(&self.secondary_value) && self.length > 0,
                LogicalEventKind::Controller | LogicalEventKind::PolyPressure => (0..=127).contains(&self.main_value) && (0..=127).contains(&self.secondary_value),
                LogicalEventKind::ProgramChange | LogicalEventKind::ChannelPressure => (0..=127).contains(&self.main_value),
                LogicalEventKind::PitchBend => (-8192..=8191).contains(&self.main_value),
            } && self.note_expression.len() <= 65_536
    }

    fn target_value(&self, target: ActionTarget) -> i64 {
        match target { ActionTarget::Channel => i64::from(self.channel), ActionTarget::Position => self.position.min(i64::MAX as u64) as i64,
            ActionTarget::Length => self.length.min(i64::MAX as u64) as i64, ActionTarget::MainValue => i64::from(self.main_value),
            ActionTarget::SecondaryValue => i64::from(self.secondary_value) }
    }

    fn set_target_value(&mut self, target: ActionTarget, value: f64) -> Result<(), String> {
        if !value.is_finite() { return Err("logical action result is not finite".into()); }
        let rounded = value.round();
        match target {
            ActionTarget::Channel if (0.0..=15.0).contains(&rounded) => self.channel = rounded as u8,
            ActionTarget::Position if (0.0..=u64::MAX as f64).contains(&rounded) => self.position = rounded as u64,
            ActionTarget::Length if (0.0..=u64::MAX as f64).contains(&rounded) => self.length = rounded as u64,
            ActionTarget::MainValue if (i32::MIN as f64..=i32::MAX as f64).contains(&rounded) => self.main_value = rounded as i32,
            ActionTarget::SecondaryValue if (i32::MIN as f64..=i32::MAX as f64).contains(&rounded) => self.secondary_value = rounded as i32,
            _ => return Err("logical action result is outside target range".into()),
        }
        Ok(())
    }
}

struct DeterministicRandom(u64);
impl DeterministicRandom {
    fn new(seed: u64) -> Self { Self(if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed }) }
    fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x }
    fn between(&mut self, minimum: f64, maximum: f64) -> f64 {
        let unit = self.next() as f64 / u64::MAX as f64; minimum + (maximum - minimum) * unit
    }
}

#[cfg(test)]
mod pro_tests {
    use super::*;

    fn event(id: u64, kind: LogicalEventKind, pitch: i32, velocity: i32, position: u64) -> LogicalMidiEvent {
        LogicalMidiEvent { id, kind, channel: 0, position, length: if kind == LogicalEventKind::Note { 100 } else { 0 },
            main_value: pitch, secondary_value: velocity, selected: false, muted: false, note_expression: vec![] }
    }
    fn condition(target: FilterTarget, operator: FilterOperator, value1: i64, value2: i64) -> FilterExpression {
        FilterExpression::Condition(FilterCondition { target, operator, value1, value2 })
    }

    #[test]
    fn nested_and_or_filter_transforms_only_matching_notes() {
        let filter = FilterExpression::And(vec![
            condition(FilterTarget::Kind, FilterOperator::Equal, LogicalEventKind::Note as i64, 0),
            FilterExpression::Or(vec![condition(FilterTarget::MainValue, FilterOperator::InsideRange, 60, 64),
                condition(FilterTarget::SecondaryValue, FilterOperator::Less, 30, 0)]),
        ]);
        let preset = LogicalEditorPresetPro { name: "Raise".into(), filter, function: LogicalFunction::Transform,
            actions: vec![LogicalAction { target: ActionTarget::MainValue, operation: ActionOperation::Add,
                parameter1: 12.0, parameter2: 0.0 }], cursor_position: 0, loop_range: None, random_seed: 1 };
        let mut events = vec![event(1, LogicalEventKind::Note, 60, 90, 0), event(2, LogicalEventKind::Note, 72, 20, 100),
            event(3, LogicalEventKind::Controller, 1, 20, 0)];
        let report = preset.apply(&mut events).unwrap();
        assert_eq!((report.matched, report.changed), (2, 2));
        assert_eq!(events.iter().find(|event| event.id == 1).unwrap().main_value, 72);
        assert_eq!(events.iter().find(|event| event.id == 2).unwrap().main_value, 84);
        assert_eq!(events.iter().find(|event| event.id == 3).unwrap().main_value, 1);
        assert!(events.windows(2).all(|pair| pair[0].position <= pair[1].position));
    }

    #[test]
    fn deterministic_random_and_relative_ramp_are_reproducible() {
        let preset = LogicalEditorPresetPro { name: "Humanize".into(), filter: condition(FilterTarget::Kind,
            FilterOperator::Equal, LogicalEventKind::Note as i64, 0), function: LogicalFunction::Transform,
            actions: vec![LogicalAction { target: ActionTarget::SecondaryValue,
                operation: ActionOperation::SetRelativeRandomBetween, parameter1: -5.0, parameter2: 5.0 },
                LogicalAction { target: ActionTarget::SecondaryValue, operation: ActionOperation::RelativeRamp,
                    parameter1: 0.0, parameter2: -20.0 }], cursor_position: 0, loop_range: Some((0, 100)), random_seed: 99 };
        let source = vec![event(1, LogicalEventKind::Note, 60, 100, 0), event(2, LogicalEventKind::Note, 64, 100, 100)];
        let mut first = source.clone(); let mut second = source;
        preset.apply(&mut first).unwrap(); preset.apply(&mut second).unwrap();
        assert_eq!(first, second); assert!(first[1].secondary_value < first[0].secondary_value);
    }

    #[test]
    fn extract_and_copy_keep_unique_ids_and_atomic_state() {
        let base = LogicalEditorPresetPro { name: "Selected".into(), filter: condition(FilterTarget::Selected,
            FilterOperator::Equal, 1, 0), function: LogicalFunction::Extract, actions: vec![], cursor_position: 0,
            loop_range: None, random_seed: 1 };
        let mut events = vec![event(1, LogicalEventKind::Note, 60, 90, 0), event(2, LogicalEventKind::Note, 64, 90, 100)];
        events[0].selected = true;
        let report = base.apply(&mut events).unwrap();
        assert_eq!(report.extracted[0].id, 1); assert_eq!(events.len(), 1);
        let mut copy = base.clone(); copy.function = LogicalFunction::Copy; copy.filter = condition(FilterTarget::Kind,
            FilterOperator::Equal, LogicalEventKind::Note as i64, 0);
        let copied = copy.apply(&mut events).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(copied.extracted.len(), 1);
        assert_ne!(events[0].id, copied.extracted[0].id);
    }

    #[test]
    fn invalid_transform_leaves_all_events_unchanged() {
        let preset = LogicalEditorPresetPro { name: "Invalid".into(), filter: condition(FilterTarget::Kind,
            FilterOperator::Equal, LogicalEventKind::Note as i64, 0), function: LogicalFunction::Transform,
            actions: vec![LogicalAction { target: ActionTarget::MainValue, operation: ActionOperation::Add,
                parameter1: 100.0, parameter2: 0.0 }], cursor_position: 0, loop_range: None, random_seed: 1 };
        let mut events = vec![event(1, LogicalEventKind::Note, 40, 90, 0), event(2, LogicalEventKind::Note, 100, 90, 100)];
        let original = events.clone();
        assert!(preset.apply(&mut events).is_err()); assert_eq!(events, original);
    }

    #[test]
    fn insert_transforms_copies_while_insert_exclusive_drops_nonmatches() {
        let action = LogicalAction { target: ActionTarget::MainValue, operation: ActionOperation::Add,
            parameter1: 12.0, parameter2: 0.0 };
        let mut insert = LogicalEditorPresetPro { name: "Insert".into(), filter: condition(FilterTarget::MainValue,
            FilterOperator::Less, 64, 0), function: LogicalFunction::Insert, actions: vec![action.clone()],
            cursor_position: 0, loop_range: None, random_seed: 1 };
        let source = vec![event(1, LogicalEventKind::Note, 60, 90, 0), event(2, LogicalEventKind::Note, 67, 90, 100)];
        let mut events = source.clone();
        assert_eq!(insert.apply(&mut events).unwrap().changed, 1);
        assert_eq!(events.iter().map(|event| event.main_value).collect::<Vec<_>>(), vec![60, 72, 67]);
        insert.function = LogicalFunction::InsertExclusive;
        let mut exclusive = source;
        assert_eq!(insert.apply(&mut exclusive).unwrap().matched, 1);
        assert_eq!(exclusive.len(), 1);
        assert_eq!(exclusive[0].main_value, 72);
    }

    #[test]
    fn copy_and_extract_apply_actions_to_new_track_output() {
        let mut preset = LogicalEditorPresetPro { name: "Copy".into(), filter: condition(FilterTarget::Kind,
            FilterOperator::Equal, LogicalEventKind::Note as i64, 0), function: LogicalFunction::Copy,
            actions: vec![LogicalAction { target: ActionTarget::SecondaryValue, operation: ActionOperation::SetFixed,
                parameter1: 64.0, parameter2: 0.0 }], cursor_position: 0, loop_range: None, random_seed: 1 };
        let mut events = vec![event(1, LogicalEventKind::Note, 60, 100, 0)];
        let copied = preset.apply(&mut events).unwrap();
        assert_eq!(events[0].secondary_value, 100);
        assert_eq!(copied.extracted[0].secondary_value, 64);
        preset.function = LogicalFunction::Extract;
        let extracted = preset.apply(&mut events).unwrap();
        assert!(events.is_empty());
        assert_eq!(extracted.extracted[0].secondary_value, 64);
    }

    #[test]
    fn transposes_to_scale_and_removes_note_expression() {
        let preset = LogicalEditorPresetPro { name: "Scale".into(), filter: condition(FilterTarget::Kind,
            FilterOperator::Equal, LogicalEventKind::Note as i64, 0), function: LogicalFunction::Transform,
            actions: vec![LogicalAction { target: ActionTarget::MainValue, operation: ActionOperation::TransposeToScale,
                parameter1: 0.0, parameter2: 0.0 }, LogicalAction { target: ActionTarget::MainValue,
                operation: ActionOperation::RemoveNoteExpression, parameter1: 0.0, parameter2: 0.0 }],
            cursor_position: 0, loop_range: None, random_seed: 1 };
        let mut events = vec![event(1, LogicalEventKind::Note, 61, 90, 0)];
        events[0].note_expression.push((74, 100));
        preset.apply(&mut events).unwrap();
        assert_eq!(events[0].main_value, 60);
        assert!(events[0].note_expression.is_empty());
    }
}
