use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

struct GateRuntime {
    gain: f32,
    attack: f32,
    release: f32,
}
struct GateState {
    runtime: UnsafeCell<GateRuntime>,
    threshold: AtomicU32,
}
unsafe impl Sync for GateState {}

struct TransientRuntime {
    envelope: f32,
}
struct TransientState {
    runtime: UnsafeCell<TransientRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for TransientState {}

struct DeEsserRuntime {
    detector: f32,
}
struct DeEsserState {
    runtime: UnsafeCell<DeEsserRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for DeEsserState {}

const NATIVE_DELAY_SIZE: usize = 65_536;
struct DelayRuntime {
    sample_rate: f64,
    buffers: [Vec<f32>; 2],
    write_index: usize,
}
struct DelayState {
    runtime: UnsafeCell<DelayRuntime>,
    mix: AtomicU32,
}
unsafe impl Sync for DelayState {}

const NATIVE_REVERB_SIZE: usize = 8192;
struct ReverbRuntime {
    lines: [Vec<f32>; 4],
    index: usize,
}
struct ReverbState {
    runtime: UnsafeCell<ReverbRuntime>,
    mix: AtomicU32,
}
unsafe impl Sync for ReverbState {}

struct DynamicEqRuntime {
    envelope: f32,
}
struct DynamicEqState {
    runtime: UnsafeCell<DynamicEqRuntime>,
    amount: AtomicU32,
}
unsafe impl Sync for DynamicEqState {}

struct ScalarProcessorState {
    value: AtomicU32,
}
unsafe impl Sync for ScalarProcessorState {}

#[no_mangle]
pub extern "C" fn hirari_native_noise_gate_create() -> *mut c_void {
    Box::into_raw(Box::new(GateState {
        runtime: UnsafeCell::new(GateRuntime {
            gain: 0.0,
            attack: 0.05,
            release: 0.005,
        }),
        threshold: AtomicU32::new(0.01f32.to_bits()),
    }))
    .cast()
}

const KIND_LIMITER: i32 = 0;
const KIND_COMPRESSOR: i32 = 1;
const KIND_GATE: i32 = 2;
const KIND_SUB_BASS: i32 = 3;
const KIND_TRANSIENT: i32 = 4;
const KIND_DEESSER: i32 = 5;
const KIND_DELAY: i32 = 6;
const KIND_REVERB: i32 = 7;
const KIND_DYNAMIC_EQ: i32 = 8;
const KIND_MID_SIDE: i32 = 9;
const KIND_STEREO_WIDTH: i32 = 10;
const KIND_SATURATION: i32 = 11;

#[path = "native_builtin_nodes.rs"]
mod nodes;
pub(crate) use nodes::*;

pub unsafe extern "C" fn hirari_native_processor_kind_for_path(
    path: *const u8,
    path_len: usize,
) -> i32 {
    if path.is_null() {
        return -1;
    }
    let path = unsafe { std::slice::from_raw_parts(path, path_len) };
    match path {
        b"Hirari/Limiter" => KIND_LIMITER,
        b"Hirari/Compressor" => KIND_COMPRESSOR,
        b"Hirari/Gate" => KIND_GATE,
        b"Hirari/SubBass" => KIND_SUB_BASS,
        b"Hirari/Transient" => KIND_TRANSIENT,
        b"Hirari/DeEsser" => KIND_DEESSER,
        b"Hirari/Delay" => KIND_DELAY,
        b"Hirari/Reverb" => KIND_REVERB,
        b"Hirari/DynamicEQ" => KIND_DYNAMIC_EQ,
        b"Hirari/MidSide" => KIND_MID_SIDE,
        b"Hirari/Width" => KIND_STEREO_WIDTH,
        b"Hirari/Saturation" => KIND_SATURATION,
        _ => -1,
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_processor_create(kind: i32) -> *mut c_void {
    match kind {
        KIND_LIMITER => crate::pro_limiter::hirari_pro_limiter_create(44_100.0),
        KIND_COMPRESSOR => crate::compressor::hirari_dynamic_compressor_create(44_100.0),
        KIND_GATE => hirari_native_noise_gate_create(),
        KIND_SUB_BASS => crate::sub_bass_generator::hirari_sub_bass_create(44_100.0),
        KIND_TRANSIENT => hirari_native_transient_shaper_create(),
        KIND_DEESSER => hirari_native_deesser_create(),
        KIND_DELAY => hirari_native_delay_create(),
        KIND_REVERB => hirari_native_reverb_create(),
        KIND_DYNAMIC_EQ => hirari_native_dynamic_eq_create(),
        KIND_MID_SIDE => hirari_native_mid_side_create(),
        KIND_STEREO_WIDTH => hirari_native_stereo_width_create(),
        KIND_SATURATION => crate::tube_saturation::hirari_tube_saturation_create(),
        _ => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_destroy(kind: i32, state: *mut c_void) {
    match kind {
        KIND_LIMITER => unsafe { crate::pro_limiter::hirari_pro_limiter_destroy(state) },
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_destroy(state)
        },
        KIND_GATE => unsafe { hirari_native_noise_gate_destroy(state) },
        KIND_SUB_BASS => unsafe { crate::sub_bass_generator::hirari_sub_bass_destroy(state) },
        KIND_TRANSIENT => unsafe { hirari_native_transient_shaper_destroy(state) },
        KIND_DEESSER => unsafe { hirari_native_deesser_destroy(state) },
        KIND_DELAY => unsafe { hirari_native_delay_destroy(state) },
        KIND_REVERB => unsafe { hirari_native_reverb_destroy(state) },
        KIND_DYNAMIC_EQ => unsafe { hirari_native_dynamic_eq_destroy(state) },
        KIND_MID_SIDE => unsafe { hirari_native_mid_side_destroy(state) },
        KIND_STEREO_WIDTH => unsafe { hirari_native_stereo_width_destroy(state) },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_destroy(state)
        },
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_prepare(
    kind: i32,
    state: *const c_void,
    sample_rate: f64,
) {
    match kind {
        KIND_LIMITER => unsafe {
            crate::pro_limiter::hirari_pro_limiter_prepare(state.cast_mut(), sample_rate)
        },
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_prepare(state.cast_mut(), sample_rate)
        },
        KIND_SUB_BASS => unsafe {
            crate::sub_bass_generator::hirari_sub_bass_prepare(state.cast_mut(), sample_rate)
        },
        KIND_DELAY => unsafe { hirari_native_delay_prepare(state, sample_rate) },
        KIND_TRANSIENT => unsafe { hirari_native_transient_shaper_reset(state) },
        KIND_DEESSER => unsafe { hirari_native_deesser_reset(state) },
        KIND_REVERB => unsafe { hirari_native_reverb_reset(state) },
        KIND_DYNAMIC_EQ => unsafe { hirari_native_dynamic_eq_reset(state) },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_reset(state.cast_mut())
        },
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_reset(kind: i32, state: *const c_void) {
    match kind {
        KIND_LIMITER => unsafe { crate::pro_limiter::hirari_pro_limiter_reset(state.cast_mut()) },
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_reset(state.cast_mut())
        },
        KIND_GATE => unsafe { hirari_native_noise_gate_reset(state) },
        KIND_SUB_BASS => unsafe { crate::sub_bass_generator::hirari_sub_bass_reset(state.cast_mut()) },
        KIND_TRANSIENT => unsafe { hirari_native_transient_shaper_reset(state) },
        KIND_DEESSER => unsafe { hirari_native_deesser_reset(state) },
        KIND_DELAY => unsafe { hirari_native_delay_reset(state) },
        KIND_REVERB => unsafe { hirari_native_reverb_reset(state) },
        KIND_DYNAMIC_EQ => unsafe { hirari_native_dynamic_eq_reset(state) },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_reset(state.cast_mut())
        },
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_process(
    kind: i32,
    state: *const c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    frames: u32,
    bpm: f64,
) {
    if channels.is_null() || channel_count == 0 {
        return;
    }
    let left = unsafe { *channels };
    let right = if channel_count > 1 {
        unsafe { *channels.add(1) }
    } else {
        std::ptr::null_mut()
    };
    match kind {
        // The limiter is a two-bus brickwall and its per-effect class refused
        // mono blocks; the other three process whatever channel pair exists.
        KIND_LIMITER => unsafe {
            if channel_count >= 2 {
                crate::pro_limiter::hirari_pro_limiter_process(
                    state.cast_mut(),
                    left,
                    right,
                    frames as usize,
                )
            }
        },
        // Matches the class-side guard: an unexpectedly large offline block is
        // skipped instead of resizing the compressor's lookahead.
        KIND_COMPRESSOR => unsafe {
            if frames <= 4096 * 16 {
                crate::compressor::hirari_dynamic_compressor_process(
                    state.cast_mut(),
                    left,
                    right,
                    frames as usize,
                )
            }
        },
        KIND_GATE => unsafe {
            hirari_native_noise_gate_process(state, channels, channel_count, frames)
        },
        KIND_SUB_BASS => unsafe {
            crate::sub_bass_generator::hirari_sub_bass_process(
                state.cast_mut(),
                left,
                right,
                frames as usize,
            )
        },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_process(
                state,
                left,
                right,
                frames as usize,
            )
        },
        KIND_TRANSIENT => unsafe {
            hirari_native_transient_shaper_process(state, channels, channel_count, frames)
        },
        KIND_DEESSER => unsafe {
            hirari_native_deesser_process(state, channels, channel_count, frames)
        },
        KIND_DELAY => unsafe {
            hirari_native_delay_process(state, channels, channel_count, frames, bpm)
        },
        KIND_REVERB => unsafe {
            hirari_native_reverb_process(state, channels, channel_count, frames)
        },
        KIND_DYNAMIC_EQ => unsafe {
            hirari_native_dynamic_eq_process(state, channels, channel_count, frames)
        },
        KIND_MID_SIDE => unsafe {
            hirari_native_mid_side_process(state, channels, channel_count, frames)
        },
        KIND_STEREO_WIDTH => unsafe {
            hirari_native_stereo_width_process(state, channels, channel_count, frames)
        },
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_set_parameter(
    kind: i32,
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if id >= hirari_native_processor_parameter_count(kind) {
        return;
    }
    match kind {
        KIND_LIMITER => unsafe {
            crate::pro_limiter::hirari_pro_limiter_set_parameter(state.cast_mut(), id, value)
        },
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_set_parameter(state.cast_mut(), id, value)
        },
        KIND_SUB_BASS => unsafe {
            crate::sub_bass_generator::hirari_sub_bass_set_parameter(state, id, value)
        },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_set_parameter(state, id, value)
        },
        KIND_GATE => unsafe { hirari_native_noise_gate_set_threshold(state, value) },
        KIND_TRANSIENT => unsafe { hirari_native_transient_shaper_set_amount(state, value) },
        KIND_DEESSER => unsafe { hirari_native_deesser_set_amount(state, value) },
        KIND_DELAY => unsafe { hirari_native_delay_set_mix(state, value) },
        KIND_REVERB => unsafe { hirari_native_reverb_set_mix(state, value) },
        KIND_DYNAMIC_EQ => unsafe { hirari_native_dynamic_eq_set_amount(state, value) },
        KIND_MID_SIDE => unsafe { hirari_native_mid_side_set(state, value) },
        KIND_STEREO_WIDTH => unsafe { hirari_native_stereo_width_set(state, value) },
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_get_parameter(
    kind: i32,
    state: *const c_void,
    id: u32,
) -> f32 {
    if id >= hirari_native_processor_parameter_count(kind) {
        return 0.0;
    }
    match kind {
        KIND_LIMITER => unsafe {
            crate::pro_limiter::hirari_pro_limiter_get_parameter(state, id)
        },
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_get_parameter(state, id)
        },
        KIND_SUB_BASS => unsafe {
            crate::sub_bass_generator::hirari_sub_bass_get_parameter(state, id)
        },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_get_parameter(state, id)
        },
        KIND_GATE => unsafe { hirari_native_noise_gate_get_threshold(state) },
        KIND_TRANSIENT => unsafe { hirari_native_transient_shaper_get_amount(state) },
        KIND_DEESSER => unsafe { hirari_native_deesser_get_amount(state) },
        KIND_DELAY => unsafe { hirari_native_delay_get_mix(state) },
        KIND_REVERB => unsafe { hirari_native_reverb_get_mix(state) },
        KIND_DYNAMIC_EQ => unsafe { hirari_native_dynamic_eq_get_amount(state) },
        KIND_MID_SIDE => unsafe { hirari_native_mid_side_get(state) },
        KIND_STEREO_WIDTH => unsafe { hirari_native_stereo_width_get(state) },
        _ => 0.0,
    }
}

#[no_mangle]
pub extern "C" fn hirari_native_processor_parameter_count(kind: i32) -> u32 {
    match kind {
        KIND_COMPRESSOR => 7,
        KIND_SATURATION => 3,
        KIND_LIMITER | KIND_GATE | KIND_SUB_BASS | KIND_TRANSIENT | KIND_DEESSER
        | KIND_DELAY | KIND_REVERB | KIND_DYNAMIC_EQ | KIND_MID_SIDE | KIND_STEREO_WIDTH => 1,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_tail_samples(
    kind: i32,
    state: *const c_void,
) -> u32 {
    match kind {
        KIND_LIMITER => unsafe { crate::pro_limiter::hirari_pro_limiter_tail(state) },
        KIND_COMPRESSOR => unsafe { crate::compressor::hirari_dynamic_compressor_tail(state) },
        KIND_DELAY => 65_536 * 8,
        KIND_REVERB => 8_192 * 24,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_latency_samples(
    kind: i32,
    state: *const c_void,
) -> u32 {
    match kind {
        // Matches ProLimiterEngine::LOOKAHEAD; the plugin class reported it
        // without consulting the engine, so the constant stays here.
        KIND_LIMITER => 480,
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_latency(state)
        },
        _ => 0,
    }
}

/// Parameter display name. Only the five effect tables that shipped names in
/// the C++ facades have entries; everything else reports an empty string.
#[no_mangle]
pub extern "C" fn hirari_native_processor_parameter_name(
    kind: i32,
    id: u32,
) -> *const std::ffi::c_char {
    let table = match kind {
        KIND_LIMITER => 2,
        KIND_COMPRESSOR => 0,
        KIND_SUB_BASS => 4,
        KIND_SATURATION => 3,
        _ => return c"".as_ptr(),
    };
    crate::builtin_effect_names::hirari_builtin_effect_parameter_name(table, id)
}

#[no_mangle]
pub extern "C" fn hirari_native_processor_name(kind: i32) -> *const std::ffi::c_char {
    let name: &'static std::ffi::CStr = match kind {
        // These three facades never overrode IProcessor::getName, so the
        // processor display name stays the base-class default.
        KIND_LIMITER | KIND_COMPRESSOR | KIND_SUB_BASS => c"Processor",
        KIND_SATURATION => c"Tube Saturation",
        KIND_GATE => c"Hirari Gate",
        KIND_TRANSIENT => c"Hirari Transient Shaper",
        KIND_DEESSER => c"Hirari De-Esser",
        KIND_DELAY => c"Hirari Delay",
        KIND_REVERB => c"Hirari Reverb",
        KIND_DYNAMIC_EQ => c"Hirari Dynamic EQ",
        KIND_MID_SIDE => c"Hirari Mid/Side",
        KIND_STEREO_WIDTH => c"Hirari Stereo Width",
        _ => return std::ptr::null(),
    };
    name.as_ptr()
}

/// Kind-specific state blob. Returns 0 when the kind has no serializer of its
/// own; the caller then falls back to the shared host envelope, which keeps
/// every previously persisted format byte-identical.
#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_state_save(
    kind: i32,
    state: *const c_void,
    bypassed: u8,
    mix: f32,
    sidechain_bus_id: u32,
    output: *mut u8,
    capacity: usize,
) -> usize {
    match kind {
        KIND_LIMITER => {
            if state.is_null() || output.is_null() || capacity < 4 {
                return 0;
            }
            let threshold = unsafe { crate::pro_limiter::hirari_pro_limiter_threshold_linear(state) };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    threshold.to_ne_bytes().as_ptr(),
                    output,
                    4,
                );
            }
            4
        }
        KIND_COMPRESSOR => unsafe {
            crate::compressor::hirari_dynamic_compressor_save_state(
                state,
                output,
                capacity,
            )
        },
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_save_state(
                state,
                bypassed,
                mix,
                sidechain_bus_id,
                output,
                capacity,
            )
        },
        _ => 0,
    }
}

/// Restores a kind-specific blob and reports the host envelope fields it
/// carried. False leaves the caller on the shared 16-byte envelope decoder.
#[no_mangle]
pub unsafe extern "C" fn hirari_native_processor_state_restore(
    kind: i32,
    state: *mut c_void,
    bytes: *const u8,
    size: usize,
    bypassed: *mut u8,
    mix: *mut f32,
    sidechain_bus_id: *mut u32,
) -> bool {
    match kind {
        KIND_LIMITER => {
            if state.is_null() || bytes.is_null() || size != 4 {
                return false;
            }
            let threshold = f32::from_ne_bytes(unsafe {
                std::slice::from_raw_parts(bytes, 4).try_into().unwrap()
            });
            unsafe { crate::pro_limiter::hirari_pro_limiter_restore_threshold(state, threshold) }
        }
        KIND_COMPRESSOR => {
            if !unsafe {
                crate::compressor::hirari_dynamic_compressor_restore_state(state, bytes, size)
            } {
                return false;
            }
            if !bypassed.is_null() && !mix.is_null() && !sidechain_bus_id.is_null() {
                // The compressor blob carries settings only; the envelope
                // fields keep their host-side defaults.
                unsafe {
                    *bypassed = 0;
                    *mix = 1.0;
                    *sidechain_bus_id = 0;
                }
            }
            true
        }
        KIND_SATURATION => unsafe {
            crate::tube_saturation::hirari_tube_saturation_restore_state(
                state,
                bytes,
                size,
                bypassed,
                mix,
                sidechain_bus_id,
            )
        },
        _ => false,
    }
}

#[cfg(test)]
mod processor_dispatch_tests {
    use super::*;

    const BUILTINS: &[(&[u8], i32, &str, u32)] = &[
        (b"Hirari/Limiter", KIND_LIMITER, "Processor", 1),
        (b"Hirari/Compressor", KIND_COMPRESSOR, "Processor", 7),
        (b"Hirari/Gate", KIND_GATE, "Hirari Gate", 1),
        (b"Hirari/SubBass", KIND_SUB_BASS, "Processor", 1),
        (b"Hirari/Transient", KIND_TRANSIENT, "Hirari Transient Shaper", 1),
        (b"Hirari/DeEsser", KIND_DEESSER, "Hirari De-Esser", 1),
        (b"Hirari/Delay", KIND_DELAY, "Hirari Delay", 1),
        (b"Hirari/Reverb", KIND_REVERB, "Hirari Reverb", 1),
        (b"Hirari/DynamicEQ", KIND_DYNAMIC_EQ, "Hirari Dynamic EQ", 1),
        (b"Hirari/MidSide", KIND_MID_SIDE, "Hirari Mid/Side", 1),
        (b"Hirari/Width", KIND_STEREO_WIDTH, "Hirari Stereo Width", 1),
        (b"Hirari/Saturation", KIND_SATURATION, "Tube Saturation", 3),
    ];

    #[test]
    fn path_lookup_maps_each_builtin_and_rejects_unknown() {
        for &(path, kind, _, _) in BUILTINS {
            assert_eq!(
                unsafe { hirari_native_processor_kind_for_path(path.as_ptr(), path.len()) },
                kind
            );
        }
        let unknown = b"Hirari/Unknown";
        assert_eq!(
            unsafe { hirari_native_processor_kind_for_path(unknown.as_ptr(), unknown.len()) },
            -1
        );
        assert_eq!(
            unsafe { hirari_native_processor_kind_for_path(std::ptr::null(), 0) },
            -1
        );
    }

    #[test]
    fn dispatch_metadata_matches_each_builtin_contract() {
        for &(_, kind, name, parameters) in BUILTINS {
            assert_eq!(hirari_native_processor_parameter_count(kind), parameters);
            let reported = hirari_native_processor_name(kind);
            assert!(!reported.is_null());
            assert_eq!(unsafe { std::ffi::CStr::from_ptr(reported) }.to_str(), Ok(name));
            let state = hirari_native_processor_create(kind);
            assert!(!state.is_null());
            unsafe {
                // The compressor derives its latency from configured lookahead,
                // so only the fixed and zero cases are pinned here.
                if kind != KIND_COMPRESSOR {
                    let expected_latency = match kind {
                        KIND_LIMITER => 480,
                        _ => 0,
                    };
                    assert_eq!(
                        hirari_native_processor_latency_samples(kind, state),
                        expected_latency
                    );
                }
                hirari_native_processor_destroy(kind, state);
            }
        }
        assert_eq!(hirari_native_processor_parameter_count(-1), 0);
        assert!(hirari_native_processor_name(-1).is_null());
        assert!(hirari_native_processor_create(-1).is_null());
        assert_eq!(
            unsafe { hirari_native_processor_tail_samples(KIND_DELAY, std::ptr::null()) },
            65_536 * 8
        );
        assert_eq!(
            unsafe { hirari_native_processor_tail_samples(KIND_REVERB, std::ptr::null()) },
            8_192 * 24
        );
        assert_eq!(
            unsafe { hirari_native_processor_tail_samples(KIND_GATE, std::ptr::null()) },
            0
        );
    }

    #[test]
    fn parameter_names_come_from_the_effect_tables() {
        let name = |kind, id| {
            let pointer = hirari_native_processor_parameter_name(kind, id);
            unsafe { std::ffi::CStr::from_ptr(pointer) }
                .to_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(name(KIND_COMPRESSOR, 0), "Threshold");
        assert_eq!(name(KIND_COMPRESSOR, 6), "Lookahead");
        assert_eq!(name(KIND_LIMITER, 0), "Threshold");
        assert_eq!(name(KIND_SATURATION, 2), "Dry/Wet");
        assert_eq!(name(KIND_SUB_BASS, 0), "Sub Amount");
        assert_eq!(name(KIND_COMPRESSOR, 7), "");
        assert_eq!(name(KIND_GATE, 0), "");
    }

    #[test]
    fn dispatch_state_round_trips_each_persisted_format() {
        for &kind in &[KIND_LIMITER, KIND_COMPRESSOR, KIND_SATURATION] {
            let source = hirari_native_processor_create(kind);
            let target = hirari_native_processor_create(kind);
            assert!(!source.is_null() && !target.is_null());
            unsafe {
                hirari_native_processor_set_parameter(kind, source, 0, 0.25);
                let mut buffer = vec![0u8; 64];
                let written = hirari_native_processor_state_save(
                    kind,
                    source,
                    1,
                    0.5,
                    7,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                );
                let expected = match kind {
                    KIND_LIMITER => 4,
                    KIND_COMPRESSOR => 28,
                    _ => 32,
                };
                assert_eq!(written, expected);
                let mut bypassed = 99u8;
                let mut mix = -1.0f32;
                let mut sidechain = 99u32;
                assert!(hirari_native_processor_state_restore(
                    kind,
                    target,
                    buffer.as_ptr(),
                    written,
                    &mut bypassed,
                    &mut mix,
                    &mut sidechain,
                ));
                assert_eq!(
                    hirari_native_processor_get_parameter(kind, target, 0),
                    hirari_native_processor_get_parameter(kind, source, 0)
                );
                if kind == KIND_SATURATION {
                    assert_eq!((bypassed, mix, sidechain), (1, 0.5, 7));
                }
                // Kinds without their own serializer fall back to the shared envelope.
                assert_eq!(
                    hirari_native_processor_state_save(
                        KIND_GATE,
                        source,
                        0,
                        1.0,
                        0,
                        buffer.as_mut_ptr(),
                        buffer.len(),
                    ),
                    0
                );
                hirari_native_processor_destroy(kind, source);
                hirari_native_processor_destroy(kind, target);
            }
        }
    }

    #[test]
    fn dispatch_lifecycle_round_trips_parameter_state() {
        let state = hirari_native_processor_create(KIND_GATE);
        assert!(!state.is_null());
        unsafe {
            hirari_native_processor_prepare(KIND_GATE, state, 48_000.0);
            hirari_native_processor_set_parameter(KIND_GATE, state, 0, 0.25);
            assert_eq!(hirari_native_processor_get_parameter(KIND_GATE, state, 0), 0.25);
            // Ids beyond the first are ignored, matching the per-effect classes.
            hirari_native_processor_set_parameter(KIND_GATE, state, 1, 0.9);
            assert_eq!(hirari_native_processor_get_parameter(KIND_GATE, state, 1), 0.0);
            let mut left = vec![0.5_f32; 16];
            let mut right = vec![0.5_f32; 16];
            let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
            hirari_native_processor_process(KIND_GATE, state, channels.as_mut_ptr(), 2, 16, 120.0);
            hirari_native_processor_reset(KIND_GATE, state);
            hirari_native_processor_destroy(KIND_GATE, state);
        }
    }
}
