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

    fn publish(&self, store: &mut SnapshotStore, mappings: Vec<Mapping>) {
        let mut next = Box::new(mappings);
        let pointer = next.as_mut() as *mut Vec<Mapping>;
        let previous = std::mem::replace(&mut store.active, next);
        self.current.store(pointer, Ordering::SeqCst);
        store.retired.push(previous);
        if self.readers.load(Ordering::SeqCst) == 0 {
            store.retired.clear();
        }
    }

    fn add(&self, mapping: Mapping) {
        let mut store = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let mut mappings = store.active.as_ref().clone();
        if let Some(existing) = mappings
            .iter_mut()
            .find(|existing| existing.target_id == mapping.target_id)
        {
            *existing = mapping;
        } else {
            mappings.push(mapping);
        }
        self.publish(&mut store, mappings);
    }

    fn clear(&self) {
        let mut store = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        self.publish(&mut store, Vec::new());
    }

    fn evaluate(&self, target_id: u32, normalized: f32) -> f32 {
        self.readers.fetch_add(1, Ordering::SeqCst);
        let _reader = ReaderGuard(&self.readers);
        let snapshot = self.current.load(Ordering::SeqCst);
        if snapshot.is_null() {
            return normalized;
        }
        let Some(mapping) = (unsafe { &*snapshot })
            .iter()
            .find(|mapping| mapping.target_id == target_id)
        else {
            return normalized;
        };
        let source = if mapping.invert {
            1.0 - normalized
        } else {
            normalized
        };
        mapping.minimum + source * (mapping.maximum - mapping.minimum)
    }
}

struct ReaderGuard<'a>(&'a AtomicUsize);

impl Drop for ReaderGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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

    fn update_smoothers(&self, sample_rate: f32) {
        for (index, smoother) in self.smoothers.iter().enumerate() {
            smoother.set_smoothing_time(10.0, sample_rate);
            smoother.set_target(self.target(index));
        }
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

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_control_update_smoothers(
    state: *const c_void,
    sample_rate: f32,
) {
    if let Some(state) = unsafe { state.cast::<MacroMappingState>().as_ref() } {
        state.update_smoothers(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_mapping_add(
    state: *mut c_void,
    macro_index: u32,
    target_id: u32,
    minimum: f32,
    maximum: f32,
    invert: bool,
) {
    if macro_index as usize >= MAX_MACROS || !minimum.is_finite() || !maximum.is_finite() {
        return;
    }
    let Some(state) = (unsafe { state.cast::<MacroMappingState>().as_ref() }) else {
        return;
    };
    let mut minimum = minimum.clamp(0.0, 1.0);
    let mut maximum = maximum.clamp(0.0, 1.0);
    if minimum > maximum {
        std::mem::swap(&mut minimum, &mut maximum);
    }
    state.slots[macro_index as usize].add(Mapping {
        target_id,
        minimum,
        maximum,
        invert,
    });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_mapping_clear(state: *mut c_void, macro_index: u32) {
    if macro_index as usize >= MAX_MACROS {
        return;
    }
    if let Some(state) = unsafe { state.cast::<MacroMappingState>().as_ref() } {
        state.slots[macro_index as usize].clear();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_macro_mapping_evaluate(
    state: *const c_void,
    macro_index: u32,
    target_id: u32,
) -> f32 {
    if macro_index as usize >= MAX_MACROS {
        return 0.0;
    }
    let Some(state) = (unsafe { state.cast::<MacroMappingState>().as_ref() }) else {
        return 0.0;
    };
    let index = macro_index as usize;
    let normalized = state.smoothers[index].current_value().clamp(0.0, 1.0);
    state.slots[index].evaluate(target_id, normalized)
}
