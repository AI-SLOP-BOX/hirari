#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use crate::engine_edit_ext::TrackEditExt;
    use crate::hirari_wavetable_synth::HirariWavetableSynthEngine;
    use crate::loudness_analyzer::LoudnessAnalyzerEngine;
    use crate::passive_curing_eq::PassiveCuringEqEngine;
    use crate::tape_machine::TapeMachineEngine;
    use crate::true_peak_limiter::TruePeakLimiterEngine;
    use crate::virtuoso_space::VirtuosoSpaceEngine;

include!("lib_tests_sandbox.rs");
include!("lib_tests_composition.rs");
include!("lib_tests_rendering.rs");
include!("lib_tests_automation.rs");
include!("lib_tests_automation_edit.rs");
include!("lib_tests_note_edits.rs");
include!("lib_tests_plugin_edit.rs");
include!("lib_tests_plugin_hydration.rs");
include!("lib_tests_routing.rs");
include!("lib_tests_track_freeze.rs");
include!("lib_tests_analysis_reads.rs");
include!("lib_tests_offline_bounce.rs");
include!("lib_tests_project_load.rs");
}

#[test]
fn engine_event_ops_contract() {
    let mut ev = crate::ffi::BridgeEvent {
        timestamp: 0,
        event_type: 0,
        track_id: 0,
        value: 0.0,
        label: [0; 128],
    };
    assert!(!crate::engine_event_ops::pop_event(std::ptr::null_mut(), &mut ev));
    assert!(!crate::engine_event_ops::set_automation_record_mode(std::ptr::null_mut(), 0));
    assert!(!crate::engine_event_ops::execute_vocal_remover(std::ptr::null_mut(), 1));
    assert_eq!(crate::engine_event_ops::get_track_correlation(std::ptr::null_mut(), 1), 1.0);
}
