//! Tests split out of midi_logical_editor.rs.

    use super::*;

    fn event(
        id: u64,
        kind: LogicalEventKind,
        pitch: i32,
        velocity: i32,
        position: u64,
    ) -> LogicalMidiEvent {
        LogicalMidiEvent {
            id,
            kind,
            channel: 0,
            position,
            length: if kind == LogicalEventKind::Note {
                100
            } else {
                0
            },
            main_value: pitch,
            secondary_value: velocity,
            selected: false,
            muted: false,
            note_expression: vec![],
        }
    }
    fn condition(
        target: FilterTarget,
        operator: FilterOperator,
        value1: i64,
        value2: i64,
    ) -> FilterExpression {
        FilterExpression::Condition(FilterCondition {
            target,
            operator,
            value1,
            value2,
        })
    }

    #[test]
    fn nested_and_or_filter_transforms_only_matching_notes() {
        let filter = FilterExpression::And(vec![
            condition(
                FilterTarget::Kind,
                FilterOperator::Equal,
                LogicalEventKind::Note as i64,
                0,
            ),
            FilterExpression::Or(vec![
                condition(FilterTarget::MainValue, FilterOperator::InsideRange, 60, 64),
                condition(FilterTarget::SecondaryValue, FilterOperator::Less, 30, 0),
            ]),
        ]);
        let preset = LogicalEditorPresetPro {
            name: "Raise".into(),
            filter,
            function: LogicalFunction::Transform,
            actions: vec![LogicalAction {
                target: ActionTarget::MainValue,
                operation: ActionOperation::Add,
                parameter1: 12.0,
                parameter2: 0.0,
            }],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
        let mut events = vec![
            event(1, LogicalEventKind::Note, 60, 90, 0),
            event(2, LogicalEventKind::Note, 72, 20, 100),
            event(3, LogicalEventKind::Controller, 1, 20, 0),
        ];
        let report = preset.apply(&mut events).unwrap();
        assert_eq!((report.matched, report.changed), (2, 2));
        assert_eq!(
            events
                .iter()
                .find(|event| event.id == 1)
                .unwrap()
                .main_value,
            72
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.id == 2)
                .unwrap()
                .main_value,
            84
        );
        assert_eq!(
            events
                .iter()
                .find(|event| event.id == 3)
                .unwrap()
                .main_value,
            1
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[0].position <= pair[1].position));
    }

    #[test]
    fn deterministic_random_and_relative_ramp_are_reproducible() {
        let preset = LogicalEditorPresetPro {
            name: "Humanize".into(),
            filter: condition(
                FilterTarget::Kind,
                FilterOperator::Equal,
                LogicalEventKind::Note as i64,
                0,
            ),
            function: LogicalFunction::Transform,
            actions: vec![
                LogicalAction {
                    target: ActionTarget::SecondaryValue,
                    operation: ActionOperation::SetRelativeRandomBetween,
                    parameter1: -5.0,
                    parameter2: 5.0,
                },
                LogicalAction {
                    target: ActionTarget::SecondaryValue,
                    operation: ActionOperation::RelativeRamp,
                    parameter1: 0.0,
                    parameter2: -20.0,
                },
            ],
            cursor_position: 0,
            loop_range: Some((0, 100)),
            random_seed: 99,
        };
        let source = vec![
            event(1, LogicalEventKind::Note, 60, 100, 0),
            event(2, LogicalEventKind::Note, 64, 100, 100),
        ];
        let mut first = source.clone();
        let mut second = source;
        preset.apply(&mut first).unwrap();
        preset.apply(&mut second).unwrap();
        assert_eq!(first, second);
        assert!(first[1].secondary_value < first[0].secondary_value);
    }

    #[test]
    fn extract_and_copy_keep_unique_ids_and_atomic_state() {
        let base = LogicalEditorPresetPro {
            name: "Selected".into(),
            filter: condition(FilterTarget::Selected, FilterOperator::Equal, 1, 0),
            function: LogicalFunction::Extract,
            actions: vec![],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
        let mut events = vec![
            event(1, LogicalEventKind::Note, 60, 90, 0),
            event(2, LogicalEventKind::Note, 64, 90, 100),
        ];
        events[0].selected = true;
        let report = base.apply(&mut events).unwrap();
        assert_eq!(report.extracted[0].id, 1);
        assert_eq!(events.len(), 1);
        let mut copy = base.clone();
        copy.function = LogicalFunction::Copy;
        copy.filter = condition(
            FilterTarget::Kind,
            FilterOperator::Equal,
            LogicalEventKind::Note as i64,
            0,
        );
        let copied = copy.apply(&mut events).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(copied.extracted.len(), 1);
        assert_ne!(events[0].id, copied.extracted[0].id);
    }

    #[test]
    fn invalid_transform_leaves_all_events_unchanged() {
        let preset = LogicalEditorPresetPro {
            name: "Invalid".into(),
            filter: condition(
                FilterTarget::Kind,
                FilterOperator::Equal,
                LogicalEventKind::Note as i64,
                0,
            ),
            function: LogicalFunction::Transform,
            actions: vec![LogicalAction {
                target: ActionTarget::MainValue,
                operation: ActionOperation::Add,
                parameter1: 100.0,
                parameter2: 0.0,
            }],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
        let mut events = vec![
            event(1, LogicalEventKind::Note, 40, 90, 0),
            event(2, LogicalEventKind::Note, 100, 90, 100),
        ];
        let original = events.clone();
        assert!(preset.apply(&mut events).is_err());
        assert_eq!(events, original);
    }

    #[test]
    fn insert_transforms_copies_while_insert_exclusive_drops_nonmatches() {
        let action = LogicalAction {
            target: ActionTarget::MainValue,
            operation: ActionOperation::Add,
            parameter1: 12.0,
            parameter2: 0.0,
        };
        let mut insert = LogicalEditorPresetPro {
            name: "Insert".into(),
            filter: condition(FilterTarget::MainValue, FilterOperator::Less, 64, 0),
            function: LogicalFunction::Insert,
            actions: vec![action.clone()],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
        let source = vec![
            event(1, LogicalEventKind::Note, 60, 90, 0),
            event(2, LogicalEventKind::Note, 67, 90, 100),
        ];
        let mut events = source.clone();
        assert_eq!(insert.apply(&mut events).unwrap().changed, 1);
        assert_eq!(
            events
                .iter()
                .map(|event| event.main_value)
                .collect::<Vec<_>>(),
            vec![60, 72, 67]
        );
        insert.function = LogicalFunction::InsertExclusive;
        let mut exclusive = source;
        assert_eq!(insert.apply(&mut exclusive).unwrap().matched, 1);
        assert_eq!(exclusive.len(), 1);
        assert_eq!(exclusive[0].main_value, 72);
    }

    #[test]
    fn copy_and_extract_apply_actions_to_new_track_output() {
        let mut preset = LogicalEditorPresetPro {
            name: "Copy".into(),
            filter: condition(
                FilterTarget::Kind,
                FilterOperator::Equal,
                LogicalEventKind::Note as i64,
                0,
            ),
            function: LogicalFunction::Copy,
            actions: vec![LogicalAction {
                target: ActionTarget::SecondaryValue,
                operation: ActionOperation::SetFixed,
                parameter1: 64.0,
                parameter2: 0.0,
            }],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
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
        let preset = LogicalEditorPresetPro {
            name: "Scale".into(),
            filter: condition(
                FilterTarget::Kind,
                FilterOperator::Equal,
                LogicalEventKind::Note as i64,
                0,
            ),
            function: LogicalFunction::Transform,
            actions: vec![
                LogicalAction {
                    target: ActionTarget::MainValue,
                    operation: ActionOperation::TransposeToScale,
                    parameter1: 0.0,
                    parameter2: 0.0,
                },
                LogicalAction {
                    target: ActionTarget::MainValue,
                    operation: ActionOperation::RemoveNoteExpression,
                    parameter1: 0.0,
                    parameter2: 0.0,
                },
            ],
            cursor_position: 0,
            loop_range: None,
            random_seed: 1,
        };
        let mut events = vec![event(1, LogicalEventKind::Note, 61, 90, 0)];
        events[0].note_expression.push((74, 100));
        preset.apply(&mut events).unwrap();
        assert_eq!(events[0].main_value, 60);
        assert!(events[0].note_expression.is_empty());
    }
