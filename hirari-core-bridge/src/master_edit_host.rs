//! Master-gain edit policy.
//!
//! The engine keeps the atomic because the audio callback reads it every
//! block, but the admission range and the undo decision live here: the current
//! value is read back, the new one is written, and a single `Master Gain`
//! history entry is recorded only when the value actually changed.
//!
//! Every entry point runs on the control thread.

/// Project master gain is limited to unity-plus-headroom; negative and
/// non-finite values are refused before the engine is touched.
pub fn admit_master_gain(value: f32) -> bool {
    value.is_finite() && (0.0..=2.0).contains(&value)
}

/// Applies a master-gain edit and records its history entry.
///
/// `engine` is the facade the audio configuration lives behind, and the undo
/// and log addresses are the ones the core was handed at construction, so the
/// undo pair lands in the same history the engine's own edits use.
pub(crate) fn set_master_gain(
    engine: &crate::ffi::AudioEngine,
    undo_state: usize,
    log_state: usize,
    value: f32,
    record_undo: bool,
) -> bool {
    if !admit_master_gain(value) {
        return false;
    }
    let before = engine.get_master_gain();
    if !engine.set_master_gain(value) {
        return false;
    }
    if record_undo && before != value {
        let engine_ptr: *const crate::ffi::AudioEngine = engine;
        // Undo replays the stored value through the same façade setter. The
        // callbacks are owned by the engine's history, which is destroyed with
        // the engine, so the pointer stays valid for as long as they exist.
        crate::record_external_action(
            undo_state as *mut std::ffi::c_void,
            log_state as *const std::ffi::c_void,
            "Master Gain",
            move || unsafe {
                (*engine_ptr).set_master_gain(before);
            },
            move || unsafe {
                (*engine_ptr).set_master_gain(value);
            },
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_gain_admission_range_matches_engine_contract() {
        assert!(admit_master_gain(0.0));
        assert!(admit_master_gain(1.0));
        assert!(admit_master_gain(2.0));
        assert!(!admit_master_gain(-0.001));
        assert!(!admit_master_gain(2.001));
        assert!(!admit_master_gain(f32::NAN));
        assert!(!admit_master_gain(f32::INFINITY));
    }
}
