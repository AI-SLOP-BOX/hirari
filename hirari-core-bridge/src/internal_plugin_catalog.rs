//! The built-in effect catalog: the serialized plugin type is the index into
//! this list. Track chains and bus chains share the catalog except for the
//! limiter slot, where a bus uses its own summing-aware limiter.

use std::ffi::{c_char, CStr};

pub const MAX_INTERNAL_PLUGIN_TYPE: u32 = 10;

const TRACK_PATHS: [&CStr; 11] = [
    c"Hirari/Limiter",
    c"Hirari/Compressor",
    c"Hirari/Gate",
    c"Hirari/Saturation",
    c"Hirari/Transient",
    c"Hirari/DeEsser",
    c"Hirari/Delay",
    c"Hirari/Reverb",
    c"Hirari/DynamicEQ",
    c"Hirari/MidSide",
    c"Hirari/Width",
];

const BUS_LIMITER_PATH: &CStr = c"Hirari/BusLimiter";

/// Resolves a serialized plugin type to the built-in processor path. Unknown
/// types stay unresolved so an inert processor is never inserted.
pub fn internal_plugin_path(plugin_type: u32, bus: bool) -> Option<&'static CStr> {
    if plugin_type > MAX_INTERNAL_PLUGIN_TYPE {
        return None;
    }
    if plugin_type == 0 && bus {
        return Some(BUS_LIMITER_PATH);
    }
    Some(TRACK_PATHS[plugin_type as usize])
}

/// Inverse of [`internal_plugin_path`] for the paths a project file or a
/// catalog entry can name. The bus limiter is the same plugin type as the
/// track limiter.
pub fn internal_plugin_type(path: &str) -> Option<u32> {
    if path.as_bytes() == BUS_LIMITER_PATH.to_bytes() {
        return Some(0);
    }
    TRACK_PATHS
        .iter()
        .position(|candidate| candidate.to_bytes() == path.as_bytes())
        .map(|index| index as u32)
}

#[no_mangle]
pub extern "C" fn hirari_internal_plugin_path(plugin_type: u32, bus: bool) -> *const c_char {
    internal_plugin_path(plugin_type, bus).map_or(std::ptr::null(), CStr::as_ptr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_internal_type_round_trips_through_its_path() {
        for plugin_type in 0..=MAX_INTERNAL_PLUGIN_TYPE {
            let path = internal_plugin_path(plugin_type, false).expect("known type");
            assert!(path.to_str().is_ok());
            assert_eq!(internal_plugin_type(path.to_str().unwrap()), Some(plugin_type));
        }
    }

    #[test]
    fn buses_use_dedicated_limiter_for_the_same_type() {
        let track = internal_plugin_path(0, false).unwrap();
        let bus = internal_plugin_path(0, true).unwrap();
        assert_eq!(track.to_str().unwrap(), "Hirari/Limiter");
        assert_eq!(bus.to_str().unwrap(), "Hirari/BusLimiter");
        assert_eq!(internal_plugin_type("Hirari/BusLimiter"), Some(0));
        // Only the limiter slot differs; the rest of the chain is shared.
        for plugin_type in 1..=MAX_INTERNAL_PLUGIN_TYPE {
            assert_eq!(
                internal_plugin_path(plugin_type, true),
                internal_plugin_path(plugin_type, false)
            );
        }
    }

    #[test]
    fn unknown_types_and_paths_stay_unresolved() {
        assert!(internal_plugin_path(MAX_INTERNAL_PLUGIN_TYPE + 1, false).is_none());
        assert!(internal_plugin_path(u32::MAX, true).is_none());
        assert!(internal_plugin_type("Hirari/Unknown").is_none());
        assert!(internal_plugin_type("").is_none());
    }

    #[test]
    fn ffi_returns_a_static_c_string_or_null() {
        let limiter = hirari_internal_plugin_path(0, false);
        assert!(!limiter.is_null());
        assert_eq!(unsafe { CStr::from_ptr(limiter) }.to_str().unwrap(), "Hirari/Limiter");
        assert!(hirari_internal_plugin_path(11, false).is_null());
    }
}
