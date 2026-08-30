// --- AURA ORCHESTRATOR | (c) 2026 ---
// DECOUPLING LAYER: UI -> ORCHESTRATOR -> CORE

use aura_core_bridge::AuraCore;
use std::fmt;
use std::rc::Rc;

pub enum UIAction {
    SetVolume(u32, f32),
    SetPan(u32, f32),
    SetRegionGain(u32, u32, f32),
    SetMidiEvents(String),
    AddTrack(String),
    TogglePlay,
    Undo,
    Redo,
    Save(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionResult {
    Applied,
    NoChange,
    Created(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    InvalidValue(&'static str),
    CoreRejected {
        operation: &'static str,
        track_id: Option<u32>,
        region_id: Option<u32>,
    },
    SaveFailed(String),
    AudioUnavailable(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue(message) => formatter.write_str(message),
            Self::CoreRejected {
                operation,
                track_id,
                region_id,
            } => {
                write!(formatter, "{operation}")?;
                if let Some(track_id) = track_id {
                    write!(formatter, " (track {track_id}")?;
                    if let Some(region_id) = region_id {
                        write!(formatter, ", region {region_id}")?;
                    }
                    write!(formatter, ")")?;
                }
                Ok(())
            }
            Self::SaveFailed(path) => write!(formatter, "failed to save project: {path}"),
            Self::AudioUnavailable(status) => write!(formatter, "audio unavailable: {status}"),
        }
    }
}

impl std::error::Error for ActionError {}

pub struct CoreOrchestrator {
    core: Rc<AuraCore>,
}

impl CoreOrchestrator {
    pub fn new(core: Rc<AuraCore>) -> Self {
        Self { core }
    }

    pub fn dispatch(&self, action: UIAction) {
        let _ = self.dispatch_result(action);
    }

    pub fn dispatch_result(&self, action: UIAction) -> Result<ActionResult, ActionError> {
        match action {
            UIAction::SetVolume(tid, val) if val.is_finite() => self
                .core
                .set_volume(tid, val)
                .then_some(ActionResult::Applied)
                .ok_or(ActionError::CoreRejected {
                    operation: "volume rejected by Core",
                    track_id: Some(tid),
                    region_id: None,
                }),
            UIAction::SetVolume(_, _) => Err(ActionError::InvalidValue("volume must be finite")),
            UIAction::SetPan(tid, val) if val.is_finite() => self
                .core
                .set_pan(tid, val)
                .then_some(ActionResult::Applied)
                .ok_or(ActionError::CoreRejected {
                    operation: "pan rejected by Core",
                    track_id: Some(tid),
                    region_id: None,
                }),
            UIAction::SetPan(_, _) => Err(ActionError::InvalidValue("pan must be finite")),
            UIAction::SetRegionGain(tid, rid, gain) => self
                .core
                .set_region_gain(tid, rid, gain)
                .then_some(ActionResult::Applied)
                .ok_or(ActionError::CoreRejected {
                    operation: "region gain rejected by Core",
                    track_id: Some(tid),
                    region_id: Some(rid),
                }),
            UIAction::SetMidiEvents(events) => self
                .core
                .set_midi_events_json(&events)
                .then_some(ActionResult::Applied)
                .ok_or(ActionError::CoreRejected {
                    operation: "MIDI event snapshot rejected by Core",
                    track_id: None,
                    region_id: None,
                }),
            UIAction::TogglePlay => {
                let playing = self.core.is_playing();
                let target = !playing;
                if self.core.try_set_playing(target) {
                    Ok(ActionResult::Applied)
                } else {
                    Err(ActionError::AudioUnavailable(
                        self.core.audio_driver_status(),
                    ))
                }
            }
            UIAction::Undo => {
                if self.core.undo_depth() == 0 {
                    return Ok(ActionResult::NoChange);
                }
                self.core.undo();
                Ok(ActionResult::Applied)
            }
            UIAction::Redo => {
                if self.core.redo_depth() == 0 {
                    return Ok(ActionResult::NoChange);
                }
                self.core.redo();
                Ok(ActionResult::Applied)
            }
            UIAction::AddTrack(t_type) => {
                let id = if t_type == "AUDIO" {
                    0
                } else if t_type == "FOLD" {
                    1
                } else {
                    2
                };
                Ok(ActionResult::Created(self.core.add_track(id)))
            }
            UIAction::Save(path) => self
                .core
                .save_project(&path)
                .then_some(ActionResult::Applied)
                .ok_or(ActionError::SaveFailed(path)),
        }
    }

    pub fn core(&self) -> Rc<AuraCore> {
        self.core.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionError, ActionResult, CoreOrchestrator, UIAction};
    use aura_core_bridge::AuraCore;
    use std::rc::Rc;
    use std::sync::{Mutex, OnceLock};

    fn native_engine_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct OfflineNativeEngineTestGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<String>,
    }

    impl Drop for OfflineNativeEngineTestGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("AURA_NATIVE_TEST_ISOLATION", previous);
            } else {
                std::env::remove_var("AURA_NATIVE_TEST_ISOLATION");
            }
        }
    }

    fn offline_native_engine_test_guard() -> OfflineNativeEngineTestGuard {
        let guard = native_engine_test_guard();
        let previous = std::env::var("AURA_NATIVE_TEST_ISOLATION").ok();
        std::env::set_var("AURA_NATIVE_TEST_ISOLATION", "1");
        OfflineNativeEngineTestGuard {
            _guard: guard,
            previous,
        }
    }

    fn region_gain(layout: &serde_json::Value, track_id: u32, region_id: u32) -> f64 {
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| {
                regions.iter().find(|region| {
                    region.get("id").and_then(serde_json::Value::as_u64) == Some(region_id as u64)
                })
            })
            .and_then(|region| region.get("clip_gain"))
            .and_then(serde_json::Value::as_f64)
            .expect("target region gain must be numeric")
    }

    #[test]
    fn toggle_play_does_not_report_success_when_audio_backend_is_unavailable() {
        let _guard = native_engine_test_guard();
        let core = Rc::new(AuraCore::new().expect("core must initialize"));
        let orchestrator = CoreOrchestrator::new(core.clone());
        let result = orchestrator.dispatch_result(UIAction::TogglePlay);
        if !core.is_playing() {
            assert!(matches!(result, Err(ActionError::AudioUnavailable(_))));
        } else {
            assert_eq!(result, Ok(ActionResult::Applied));
            assert!(core.try_set_playing(false));
        }
    }

    #[test]
    fn region_edit_round_trips_through_ui_action_undo_and_redo() {
        let _guard = offline_native_engine_test_guard();
        let core = AuraCore::new().expect("core must initialize");
        assert!(core.apply_audio_config(48_000, 1_024));
        core.start_recording_capture(48_000.0, 2, 128, 0)
            .expect("capture must start");
        let input = (0..4_096usize)
            .flat_map(|frame| {
                let sample = if frame % 32 < 16 { 0.35 } else { -0.15 };
                [sample, sample]
            })
            .collect::<Vec<_>>();
        core.append_recording_preview(&input)
            .expect("capture must accept audio");
        let track_id = core.add_track(0);
        core.commit_recording_capture_to_track(track_id, None)
            .expect("capture must publish");
        let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json())
            .expect("layout must be valid JSON");
        let region_id = layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("regions"))
            .and_then(serde_json::Value::as_array)
            .and_then(|regions| regions.first())
            .and_then(|region| region.get("id"))
            .and_then(serde_json::Value::as_u64)
            .expect("capture must publish a region") as u32;

        let orchestrator = CoreOrchestrator::new(Rc::new(core));
        let render_token = format!(
            "aura-ui-undo-render-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock must be valid")
                .as_nanos()
        );
        let edited_path = std::env::temp_dir().join(format!("{render_token}-edited.wav"));
        let undone_path = std::env::temp_dir().join(format!("{render_token}-undone.wav"));
        let redone_path = std::env::temp_dir().join(format!("{render_token}-redone.wav"));
        let undo_before_gesture = orchestrator.core().undo_depth();
        orchestrator.dispatch(UIAction::SetRegionGain(track_id, region_id, 0.25));
        let undo_after_first_gesture_value = orchestrator.core().undo_depth();
        let edited: serde_json::Value =
            serde_json::from_str(&orchestrator.core().get_project_layout_json())
                .expect("edited layout must be valid JSON");
        assert_eq!(region_gain(&edited, track_id, region_id), 0.25);

        // A short knob gesture emits multiple values but must remain one undo
        // transaction; redo restores only the final value.
        orchestrator.dispatch(UIAction::SetRegionGain(track_id, region_id, 0.5));
        let undo_after_final_gesture_value = orchestrator.core().undo_depth();
        assert_eq!(
            undo_after_final_gesture_value, undo_after_first_gesture_value,
            "successive values from one UI gesture must share one undo transaction"
        );
        assert_eq!(
            undo_after_final_gesture_value,
            undo_before_gesture.saturating_add(1),
            "the gesture must add exactly one undo transaction"
        );
        assert!(orchestrator
            .core()
            .bounce_project(edited_path.to_str().unwrap(), 0));

        orchestrator.dispatch(UIAction::Undo);
        let undone: serde_json::Value =
            serde_json::from_str(&orchestrator.core().get_project_layout_json())
                .expect("undone layout must be valid JSON");
        assert_eq!(region_gain(&undone, track_id, region_id), 1.0);
        assert!(orchestrator
            .core()
            .bounce_project(undone_path.to_str().unwrap(), 0));

        orchestrator.dispatch(UIAction::Redo);
        let redone: serde_json::Value =
            serde_json::from_str(&orchestrator.core().get_project_layout_json())
                .expect("redone layout must be valid JSON");
        assert_eq!(region_gain(&redone, track_id, region_id), 0.5);
        assert!(orchestrator
            .core()
            .bounce_project(redone_path.to_str().unwrap(), 0));

        let edited_audio = std::fs::read(&edited_path).expect("edited render must exist");
        let undone_audio = std::fs::read(&undone_path).expect("undone render must exist");
        let redone_audio = std::fs::read(&redone_path).expect("redone render must exist");
        assert_ne!(edited_audio[44..], undone_audio[44..]);
        assert_eq!(edited_audio[44..], redone_audio[44..]);

        // A new UI action after Undo must invalidate the Redo branch.
        orchestrator.dispatch(UIAction::Undo);
        orchestrator.dispatch(UIAction::SetRegionGain(track_id, region_id, 0.75));
        orchestrator.dispatch(UIAction::Redo);
        let branched: serde_json::Value =
            serde_json::from_str(&orchestrator.core().get_project_layout_json())
                .expect("branched layout must be valid JSON");
        assert_eq!(region_gain(&branched, track_id, region_id), 0.75);
        for path in [edited_path, undone_path, redone_path] {
            std::fs::remove_file(path).expect("undo render cleanup must succeed");
        }
    }

    #[test]
    fn midi_events_reach_core_through_ui_action_router() {
        let _guard = native_engine_test_guard();
        let core = AuraCore::new().expect("core must initialize");
        let events = r#"[
            {"beat":0.0,"channel":0,"kind":{"ControlChange":{"controller":1,"value":64}}},
            {"beat":0.5,"channel":1,"kind":{"PitchBend":{"value":0.0}}},
            {"beat":1.0,"channel":2,"kind":{"ChannelAftertouch":{"pressure":0.5}}}
        ]"#;
        let orchestrator = CoreOrchestrator::new(Rc::new(core));
        orchestrator.dispatch(UIAction::SetMidiEvents(events.to_string()));
        let expected: serde_json::Value = serde_json::from_str(events).expect("input must be JSON");
        let actual: serde_json::Value =
            serde_json::from_str(&orchestrator.core().midi_events_json())
                .expect("core MIDI snapshot must be JSON");
        assert_eq!(actual, expected);

        let project_path = std::env::temp_dir().join(format!(
            "aura-ui-midi-action-{}-{}.aura",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock must be valid")
                .as_nanos()
        ));
        assert!(orchestrator
            .core()
            .save_project(project_path.to_str().unwrap()));
        let restored = AuraCore::new().expect("restored core must initialize");
        assert!(restored.load_project(project_path.to_str().unwrap()));
        let restored_events: serde_json::Value = serde_json::from_str(&restored.midi_events_json())
            .expect("restored MIDI snapshot must be JSON");
        assert_eq!(restored_events, expected);
        std::fs::remove_file(project_path).expect("project cleanup must succeed");
    }

    #[test]
    fn dispatch_result_reports_core_rejection_instead_of_silent_failure() {
        let _guard = native_engine_test_guard();
        let core = AuraCore::new().expect("core must initialize");
        let orchestrator = CoreOrchestrator::new(Rc::new(core));
        assert_eq!(
            orchestrator.dispatch_result(UIAction::SetRegionGain(0, 0, 0.5)),
            Err(ActionError::CoreRejected {
                operation: "region gain rejected by Core",
                track_id: Some(0),
                region_id: Some(0),
            })
        );
        assert_eq!(
            orchestrator.dispatch_result(UIAction::SetVolume(0, f32::NAN)),
            Err(ActionError::InvalidValue("volume must be finite"))
        );
        assert_eq!(
            orchestrator.dispatch_result(UIAction::SetVolume(u32::MAX, 0.5)),
            Err(ActionError::CoreRejected {
                operation: "volume rejected by Core",
                track_id: Some(u32::MAX),
                region_id: None,
            })
        );
        assert_eq!(
            orchestrator.dispatch_result(UIAction::SetPan(u32::MAX, 0.0)),
            Err(ActionError::CoreRejected {
                operation: "pan rejected by Core",
                track_id: Some(u32::MAX),
                region_id: None,
            })
        );
        let undo_expected = if orchestrator.core().undo_depth() == 0 {
            ActionResult::NoChange
        } else {
            ActionResult::Applied
        };
        assert_eq!(
            orchestrator.dispatch_result(UIAction::Undo),
            Ok(undo_expected)
        );
        let redo_expected = if orchestrator.core().redo_depth() == 0 {
            ActionResult::NoChange
        } else {
            ActionResult::Applied
        };
        assert_eq!(
            orchestrator.dispatch_result(UIAction::Redo),
            Ok(redo_expected)
        );
        assert!(matches!(
            orchestrator
                .dispatch_result(UIAction::AddTrack("AUDIO".into()))
                .expect("track creation must succeed"),
            ActionResult::Created(_)
        ));
    }
}
