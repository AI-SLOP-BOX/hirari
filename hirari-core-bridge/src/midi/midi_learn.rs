//! Lock-free realtime MIDI controller mappings targeting Rust-owned macro state.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Mapping {
    controller: u16,
    channel: u8,
    macro_state: usize,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
    fourteen_bit: bool,
    picked_up: Arc<AtomicBool>,
}

struct MidiLearnState {
    current: AtomicPtr<Vec<Mapping>>,
    readers: AtomicUsize,
    snapshots: Mutex<SnapshotStore>,
}

struct SnapshotStore {
    active: Box<Vec<Mapping>>,
    retired: Vec<Box<Vec<Mapping>>>,
}

struct ReaderGuard<'a>(&'a AtomicUsize);

impl<'a> ReaderGuard<'a> {
    fn new(readers: &'a AtomicUsize) -> Self {
        readers.fetch_add(1, Ordering::SeqCst);
        Self(readers)
    }
}

impl Drop for ReaderGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl MidiLearnState {
    fn publish(&self, store: &mut SnapshotStore, mappings: Vec<Mapping>) {
        let mut next = Box::new(mappings);
        let pointer = next.as_mut() as *mut Vec<Mapping>;
        let previous = std::mem::replace(&mut store.active, next);
        self.current.store(pointer, Ordering::SeqCst);
        store.retired.push(previous);
        // Reclaim only on the control thread. Sequential consistency orders
        // reader admission, pointer publication, and this quiescence check.
        if self.readers.load(Ordering::SeqCst) == 0 {
            store.retired.clear();
        }
    }

    fn add_mapping(
        &self,
        controller: u16,
        channel: u8,
        macro_state: *const c_void,
        macro_index: u32,
        minimum: f32,
        maximum: f32,
        curve: f32,
        pickup: bool,
        fourteen_bit: bool,
    ) {
        let controller_max = if fourteen_bit { 16_383 } else { 127 };
        if controller > controller_max
            || channel > 16
            || macro_state.is_null()
            || !minimum.is_finite()
            || !maximum.is_finite()
            || minimum > maximum
            || !curve.is_finite()
            || !(-1.0..=1.0).contains(&curve)
        {
            return;
        }
        let mut store = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let mut mappings = store.active.as_ref().clone();
        mappings.retain(|mapping| mapping.controller != controller || mapping.channel != channel);
        mappings.push(Mapping {
            controller,
            channel,
            macro_state: macro_state as usize,
            macro_index,
            minimum,
            maximum,
            curve,
            pickup,
            fourteen_bit,
            picked_up: Arc::new(AtomicBool::new(!pickup)),
        });
        self.publish(&mut store, mappings);
    }

    fn remove_mapping(&self, controller: u16, channel: u8) {
        if controller > 127 || channel > 16 {
            return;
        }
        let mut store = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let mut mappings = store.active.as_ref().clone();
        mappings.retain(|mapping| mapping.controller != controller || mapping.channel != channel);
        self.publish(&mut store, mappings);
    }

    fn handle(&self, channel: u8, controller: u16, value: u16, fourteen_bit: bool) {
        if channel > 15
            || controller > if fourteen_bit { 16_383 } else { 127 }
            || value > if fourteen_bit { 16_383 } else { 127 }
        {
            return;
        }
        let _reader = ReaderGuard::new(&self.readers);
        let snapshot = self.current.load(Ordering::SeqCst);
        if snapshot.is_null() {
            return;
        }
        let normalized = value as f32 / if fourteen_bit { 16_383.0 } else { 127.0 };
        for mapping in unsafe { &*snapshot } {
            if mapping.fourteen_bit != fourteen_bit
                || mapping.controller != controller
                || (mapping.channel != 16 && mapping.channel != channel)
                || mapping.macro_state == 0
            {
                continue;
            }
            let macro_state = mapping.macro_state as *const c_void;
            let current = unsafe {
                crate::macro_mapping::hirari_macro_control_get_value(
                    macro_state,
                    mapping.macro_index,
                )
            }
            .clamp(mapping.minimum, mapping.maximum);
            if mapping.pickup && !mapping.picked_up.load(Ordering::Relaxed) {
                let span = (mapping.maximum - mapping.minimum).max(0.0001);
                let target = mapping.minimum + span * normalized;
                if (current - target).abs() > span * 0.02 {
                    continue;
                }
                mapping.picked_up.store(true, Ordering::Relaxed);
            }
            let shaped = if mapping.curve == 0.0 {
                normalized
            } else if mapping.curve > 0.0 {
                normalized.powf(1.0 + mapping.curve * 3.0)
            } else {
                1.0 - (1.0 - normalized).powf(1.0 - mapping.curve * 3.0)
            };
            let output =
                mapping.minimum + (mapping.maximum - mapping.minimum) * shaped.clamp(0.0, 1.0);
            unsafe {
                crate::macro_mapping::hirari_macro_control_set_midi_target(
                    macro_state,
                    mapping.macro_index,
                    output,
                )
            };
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_learn_create() -> *mut c_void {
    let mut initial = Box::new(Vec::new());
    let current = initial.as_mut() as *mut Vec<Mapping>;
    Box::into_raw(Box::new(MidiLearnState {
        current: AtomicPtr::new(current),
        readers: AtomicUsize::new(0),
        snapshots: Mutex::new(SnapshotStore {
            active: initial,
            retired: Vec::new(),
        }),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<MidiLearnState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_add_mapping(
    state: *mut c_void,
    cc: u8,
    channel: u8,
    macro_state: *const c_void,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
) {
    if let Some(state) = unsafe { state.cast::<MidiLearnState>().as_ref() } {
        state.add_mapping(
            cc as u16,
            channel,
            macro_state,
            macro_index,
            minimum,
            maximum,
            curve,
            pickup,
            false,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_add_mapping_14bit(
    state: *mut c_void,
    controller: u16,
    channel: u8,
    macro_state: *const c_void,
    macro_index: u32,
    minimum: f32,
    maximum: f32,
    curve: f32,
    pickup: bool,
) {
    if let Some(state) = unsafe { state.cast::<MidiLearnState>().as_ref() } {
        state.add_mapping(
            controller,
            channel,
            macro_state,
            macro_index,
            minimum,
            maximum,
            curve,
            pickup,
            true,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_remove_mapping(state: *mut c_void, cc: u8, channel: u8) {
    if let Some(state) = unsafe { state.cast::<MidiLearnState>().as_ref() } {
        state.remove_mapping(cc as u16, channel);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_handle_cc(
    state: *const c_void,
    channel: u8,
    cc: u8,
    value: u8,
) {
    if let Some(state) = unsafe { state.cast::<MidiLearnState>().as_ref() } {
        state.handle(channel, cc as u16, value as u16, false);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_learn_handle_cc14(
    state: *const c_void,
    channel: u8,
    controller: u16,
    value: u16,
) {
    if let Some(state) = unsafe { state.cast::<MidiLearnState>().as_ref() } {
        state.handle(channel, controller, value, true);
    }
}
