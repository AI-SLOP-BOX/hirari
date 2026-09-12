#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use crate::aura_wavetable_synth::AuraWavetableSynthEngine;
    use crate::loudness_analyzer::LoudnessAnalyzerEngine;
    use crate::passive_curing_eq::PassiveCuringEqEngine;
    use crate::tape_machine::TapeMachineEngine;
    use crate::true_peak_limiter::TruePeakLimiterEngine;
    use crate::virtuoso_space::VirtuosoSpaceEngine;

include!("lib_tests_sandbox.rs");
include!("lib_tests_composition.rs");
include!("lib_tests_rendering.rs");
include!("lib_tests_automation.rs");
}
