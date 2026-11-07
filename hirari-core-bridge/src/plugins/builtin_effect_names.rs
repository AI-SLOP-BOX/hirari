//! Display names for built-in effect parameters.
//!
//! The host UI and project diagnostics show these strings verbatim, so the
//! tables live beside the Rust effect owners instead of in each C++ facade.

use std::ffi::{c_char, CStr};

const COMPRESSOR: u32 = 0;
const MASTER_LIMITER: u32 = 1;
const PRO_LIMITER: u32 = 2;
const TUBE_SATURATION: u32 = 3;
const SUB_BASS_GENERATOR: u32 = 4;
const CHORD_TRIGGER: u32 = 5;

/// Parameter name for built-in effect `kind`, or an empty string for an
/// unknown kind or parameter id.
#[no_mangle]
pub extern "C" fn hirari_builtin_effect_parameter_name(kind: u32, id: u32) -> *const c_char {
    let name: &'static CStr = match (kind, id) {
        (COMPRESSOR, 0) => c"Threshold",
        (COMPRESSOR, 1) => c"Ratio",
        (COMPRESSOR, 2) => c"Attack",
        (COMPRESSOR, 3) => c"Release",
        (COMPRESSOR, 4) => c"Makeup",
        (COMPRESSOR, 5) => c"Knee",
        (COMPRESSOR, 6) => c"Lookahead",
        (MASTER_LIMITER, 0) => c"Threshold",
        (MASTER_LIMITER, 1) => c"Ceiling",
        (MASTER_LIMITER, 2) => c"Release",
        (MASTER_LIMITER, 3) => c"Lookahead",
        (PRO_LIMITER, 0) => c"Threshold",
        (TUBE_SATURATION, 0) => c"Drive",
        (TUBE_SATURATION, 1) => c"Bias",
        (TUBE_SATURATION, 2) => c"Dry/Wet",
        (SUB_BASS_GENERATOR, 0) => c"Sub Amount",
        (CHORD_TRIGGER, 0) => c"Strum Time",
        _ => c"",
    };
    name.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(kind: u32, id: u32) -> String {
        unsafe { CStr::from_ptr(hirari_builtin_effect_parameter_name(kind, id)) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn tables_match_the_builtin_effects() {
        let compressor = ["Threshold", "Ratio", "Attack", "Release", "Makeup", "Knee", "Lookahead"];
        for (id, expected) in compressor.iter().enumerate() {
            assert_eq!(name(COMPRESSOR, id as u32), *expected);
        }
        let limiter = ["Threshold", "Ceiling", "Release", "Lookahead"];
        for (id, expected) in limiter.iter().enumerate() {
            assert_eq!(name(MASTER_LIMITER, id as u32), *expected);
        }
        let tube = ["Drive", "Bias", "Dry/Wet"];
        for (id, expected) in tube.iter().enumerate() {
            assert_eq!(name(TUBE_SATURATION, id as u32), *expected);
        }
        assert_eq!(name(PRO_LIMITER, 0), "Threshold");
        assert_eq!(name(SUB_BASS_GENERATOR, 0), "Sub Amount");
        assert_eq!(name(CHORD_TRIGGER, 0), "Strum Time");
    }

    #[test]
    fn unknown_kinds_and_ids_are_empty() {
        assert_eq!(name(COMPRESSOR, 7), "");
        assert_eq!(name(MASTER_LIMITER, 4), "");
        assert_eq!(name(TUBE_SATURATION, 3), "");
        assert_eq!(name(PRO_LIMITER, 1), "");
        assert_eq!(name(99, 0), "");
        assert_eq!(name(SUB_BASS_GENERATOR, u32::MAX), "");
    }
}
