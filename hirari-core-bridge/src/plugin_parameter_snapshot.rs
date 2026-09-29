//! Sparse, bounded parameter cache used to replay third-party plugin controls
//! after an isolated worker restarts.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

const CAPACITY: usize = 4096;

struct Slot {
    state: AtomicU8,
    parameter_id: AtomicU32,
    value_bits: AtomicU64,
}

impl Slot {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            parameter_id: AtomicU32::new(0),
            value_bits: AtomicU64::new(0.0f64.to_bits()),
        }
    }
}

struct Snapshot {
    slots: Box<[Slot]>,
}

impl Snapshot {
    fn new() -> Self {
        Self {
            slots: (0..CAPACITY)
                .map(|_| Slot::new())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    fn start(parameter_id: u32) -> usize {
        (u64::from(parameter_id).wrapping_mul(2_654_435_761) as usize) & (CAPACITY - 1)
    }

    fn set(&self, parameter_id: u32, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        let start = Self::start(parameter_id);
        for probe in 0..CAPACITY {
            let slot = &self.slots[(start + probe) & (CAPACITY - 1)];
            let mut state = slot.state.load(Ordering::Acquire);
            if state == 1 {
                for _ in 0..8 {
                    if state != 1 {
                        break;
                    }
                    state = slot.state.load(Ordering::Acquire);
                }
                if state == 1 {
                    return false;
                }
            }
            if state == 2 {
                if slot.parameter_id.load(Ordering::Relaxed) == parameter_id {
                    slot.value_bits.store(value.to_bits(), Ordering::Release);
                    return true;
                }
                continue;
            }
            if state != 0 {
                continue;
            }
            if slot
                .state
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            slot.parameter_id.store(parameter_id, Ordering::Relaxed);
            slot.value_bits.store(value.to_bits(), Ordering::Relaxed);
            slot.state.store(2, Ordering::Release);
            return true;
        }
        false
    }

    fn get(&self, parameter_id: u32) -> Option<f64> {
        let start = Self::start(parameter_id);
        for probe in 0..CAPACITY {
            let slot = &self.slots[(start + probe) & (CAPACITY - 1)];
            if slot.state.load(Ordering::Acquire) == 2
                && slot.parameter_id.load(Ordering::Relaxed) == parameter_id
            {
                let value = f64::from_bits(slot.value_bits.load(Ordering::Acquire));
                return value.is_finite().then_some(value);
            }
        }
        None
    }
}

#[no_mangle]
pub extern "C" fn hirari_plugin_parameter_snapshot_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(Snapshot::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_parameter_snapshot_destroy(handle: *mut std::ffi::c_void) {
    if !handle.is_null() {
        // SAFETY: Handle came from create and destruction is serialized after
        // the processor has stopped accessing its snapshot.
        drop(unsafe { Box::from_raw(handle.cast::<Snapshot>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_parameter_snapshot_set(
    handle: *const std::ffi::c_void,
    parameter_id: u32,
    value: f64,
) -> bool {
    if handle.is_null() {
        return false;
    }
    // SAFETY: Handle remains live for the processor lifetime; slots are atomic.
    unsafe { &*handle.cast::<Snapshot>() }.set(parameter_id, value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_parameter_snapshot_get(
    handle: *const std::ffi::c_void,
    parameter_id: u32,
    output: *mut f64,
) -> bool {
    if handle.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: Handle remains live and output is a writable f64 from the caller.
    let Some(value) = (unsafe { &*handle.cast::<Snapshot>() }).get(parameter_id) else {
        return false;
    };
    unsafe { output.write(value) };
    true
}

pub type VisitCallback = unsafe extern "C" fn(*mut std::ffi::c_void, u32, f64) -> bool;

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_parameter_snapshot_visit(
    handle: *const std::ffi::c_void,
    context: *mut std::ffi::c_void,
    callback: Option<VisitCallback>,
) -> bool {
    if handle.is_null() || context.is_null() {
        return false;
    }
    let Some(callback) = callback else { return false };
    // SAFETY: Handle remains live during the synchronous traversal.
    let snapshot = unsafe { &*handle.cast::<Snapshot>() };
    for slot in snapshot.slots.iter() {
        if slot.state.load(Ordering::Acquire) != 2 {
            continue;
        }
        let parameter_id = slot.parameter_id.load(Ordering::Relaxed);
        let value = f64::from_bits(slot.value_bits.load(Ordering::Acquire));
        if !value.is_finite() || !unsafe { callback(context, parameter_id, value) } {
            return false;
        }
    }
    true
}
