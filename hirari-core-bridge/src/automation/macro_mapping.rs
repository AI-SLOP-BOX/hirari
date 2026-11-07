//! Rust-owned mappings from macro controls to plug-in parameter ranges.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::parameter_smoother::SmootherOrchestrator;

const MAX_MACROS: usize = 128;

#[derive(Clone, Copy)]
struct Mapping {
    target_id: u32,
    minimum: f32,
    maximum: f32,
    invert: bool,
}

struct SnapshotStore {
    active: Box<Vec<Mapping>>,
    retired: Vec<Box<Vec<Mapping>>>,
}

struct MacroSlot {
    current: AtomicPtr<Vec<Mapping>>,
    readers: AtomicUsize,
    snapshots: Mutex<SnapshotStore>,
}

impl MacroSlot {
    fn new() -> Self {
        let mut initial = Box::new(Vec::new());
        let current = initial.as_mut() as *mut Vec<Mapping>;
        Self {
            current: AtomicPtr::new(current),
            readers: AtomicUsize::new(0),
            snapshots: Mutex::new(SnapshotStore {
                active: initial,
                retired: Vec::new(),
            }),
        }
    }

                }

struct MacroMappingState {
    slots: [MacroSlot; MAX_MACROS],
    target_values: [AtomicU32; MAX_MACROS],
    smoothers: [SmootherOrchestrator; MAX_MACROS],
}

impl MacroMappingState {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| MacroSlot::new()),
            target_values: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            smoothers: std::array::from_fn(|_| SmootherOrchestrator::new(0.0)),
        }
    }

    fn set_target(&self, index: usize, value: f32) {
        if index < MAX_MACROS && value.is_finite() {
            self.target_values[index].store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    fn target(&self, index: usize) -> f32 {
        self.target_values
            .get(index)
            .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    }

#[no_mangle]
pub extern "C" fn hirari_macro_mapping_create() -> *mut c_void {
    Box::into_raw(Box::new(MacroMappingState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_mapping_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<MacroMappingState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_control_set_value(
    state: *mut c_void,
    macro_index: u32,
    value: f32,
) {
    if macro_index as usize >= MAX_MACROS {
        return;
    }
    if let Some(state) = unsafe { state.cast::<MacroMappingState>().as_ref() } {
        state.set_target(macro_index as usize, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_control_get_value(
    state: *const c_void,
    macro_index: u32,
) -> f32 {
    if macro_index as usize >= MAX_MACROS {
        return 0.0;
    }
    unsafe { state.cast::<MacroMappingState>().as_ref() }
        .map_or(0.0, |state| state.target(macro_index as usize))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_control_set_midi_target(
    state: *const c_void,
    macro_index: u32,
    value: f32,
) {
    if macro_index as usize >= MAX_MACROS {
        return;
    }
    if let Some(state) = unsafe { state.cast::<MacroMappingState>().as_ref() } {
        state.set_target(macro_index as usize, value);
    }
}

