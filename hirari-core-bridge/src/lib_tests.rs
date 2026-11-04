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
}
