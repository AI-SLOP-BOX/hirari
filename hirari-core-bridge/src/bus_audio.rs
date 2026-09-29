//! Realtime planar bus accumulation kernels.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MAX_BUS_FRAMES: usize = 8192;

struct BusAudioState {
    pre_left: UnsafeCell<Box<[f32]>>,
    pre_right: UnsafeCell<Box<[f32]>>,
    post_left: UnsafeCell<Box<[f32]>>,
    post_right: UnsafeCell<Box<[f32]>>,
    samples: AtomicU32,
}

impl BusAudioState {
    fn new() -> Self {
        let buffer = || vec![0.0; MAX_BUS_FRAMES].into_boxed_slice();
        Self {
            pre_left: UnsafeCell::new(buffer()),
            pre_right: UnsafeCell::new(buffer()),
            post_left: UnsafeCell::new(buffer()),
            post_right: UnsafeCell::new(buffer()),
            samples: AtomicU32::new(0),
        }
    }
}

unsafe fn bus_state(handle: *mut c_void) -> Option<&'static BusAudioState> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &*handle.cast::<BusAudioState>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_bus_audio_create() -> *mut c_void {
    Box::into_raw(Box::new(BusAudioState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<BusAudioState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_accumulate(
    handle: *mut c_void,
    input_left: *const f32,
    input_right: *const f32,
    frames: u32,
    gain: f32,
) -> bool {
    let Some(state) = (unsafe { bus_state(handle) }) else {
        return false;
    };
    let frames = frames as usize;
    if input_left.is_null()
        || input_right.is_null()
        || frames == 0
        || frames > MAX_BUS_FRAMES
        || !gain.is_finite()
    {
        return false;
    }
    unsafe {
        hirari_bus_accumulate_stereo(
            (&mut **state.pre_left.get()).as_mut_ptr(),
            (&mut **state.pre_right.get()).as_mut_ptr(),
            input_left,
            input_right,
            frames as u32,
            gain,
        )
    }
    .then(|| state.samples.store(frames as u32, Ordering::Release))
    .is_some()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_replace_post(
    handle: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
) -> bool {
    let Some(state) = (unsafe { bus_state(handle) }) else {
        return false;
    };
    let frames = frames as usize;
    if left.is_null() || right.is_null() || frames == 0 || frames > MAX_BUS_FRAMES {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(left, (&mut **state.post_left.get()).as_mut_ptr(), frames);
        std::ptr::copy_nonoverlapping(right, (&mut **state.post_right.get()).as_mut_ptr(), frames);
    }
    state.samples.store(frames as u32, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_clear(handle: *mut c_void, frames: u32) {
    let Some(state) = (unsafe { bus_state(handle) }) else {
        return;
    };
    let frames = (frames as usize).min(MAX_BUS_FRAMES);
    (&mut **state.pre_left.get())[..frames].fill(0.0);
    (&mut **state.pre_right.get())[..frames].fill(0.0);
    (&mut **state.post_left.get())[..frames].fill(0.0);
    (&mut **state.post_right.get())[..frames].fill(0.0);
    state.samples.store(0, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_commit_dry(handle: *mut c_void, frames: u32) -> bool {
    let Some(state) = (unsafe { bus_state(handle) }) else {
        return false;
    };
    let frames = frames as usize;
    if frames == 0 || frames > MAX_BUS_FRAMES {
        return false;
    }
    (&mut **state.post_left.get())[..frames].copy_from_slice(&(&*state.pre_left.get())[..frames]);
    (&mut **state.post_right.get())[..frames].copy_from_slice(&(&*state.pre_right.get())[..frames]);
    state.samples.store(frames as u32, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_read(
    handle: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    post: bool,
) -> bool {
    let Some(state) = (unsafe { bus_state(handle) }) else {
        return false;
    };
    let frames = frames as usize;
    if left.is_null() || right.is_null() || frames == 0 || frames > MAX_BUS_FRAMES {
        return false;
    }
    let (source_left, source_right) = if post {
        (&*state.post_left.get(), &*state.post_right.get())
    } else {
        (&*state.pre_left.get(), &*state.pre_right.get())
    };
    unsafe {
        std::ptr::copy_nonoverlapping(source_left.as_ptr(), left, frames);
        std::ptr::copy_nonoverlapping(source_right.as_ptr(), right, frames);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_audio_samples(handle: *const c_void) -> u32 {
    let Some(state) = (unsafe { handle.cast::<BusAudioState>().as_ref() }) else {
        return 0;
    };
    state.samples.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_accumulate_stereo(
    bus_left: *mut f32,
    bus_right: *mut f32,
    input_left: *const f32,
    input_right: *const f32,
    frames: u32,
    gain: f32,
) -> bool {
    let frames = frames as usize;
    if bus_left.is_null()
        || bus_right.is_null()
        || input_left.is_null()
        || input_right.is_null()
        || frames == 0
        || frames > MAX_BUS_FRAMES
        || !gain.is_finite()
    {
        return false;
    }
    let bus_left = unsafe { std::slice::from_raw_parts_mut(bus_left, frames) };
    let bus_right = unsafe { std::slice::from_raw_parts_mut(bus_right, frames) };
    let input_left = unsafe { std::slice::from_raw_parts(input_left, frames) };
    let input_right = unsafe { std::slice::from_raw_parts(input_right, frames) };
    for frame in 0..frames {
        let left = if input_left[frame].is_finite() {
            input_left[frame]
        } else {
            0.0
        };
        let right = if input_right[frame].is_finite() {
            input_right[frame]
        } else {
            0.0
        };
        bus_left[frame] += left * gain;
        bus_right[frame] += right * gain;
    }
    true
}

const MAX_BUSES: usize = 128;

struct BusSystemState {
    buses: [usize; MAX_BUSES],
    present: [AtomicBool; MAX_BUSES],
    order: [AtomicU32; MAX_BUSES],
    order_count: AtomicU32,
}

impl BusSystemState {
    fn new() -> Self {
        Self {
            buses: [0; MAX_BUSES],
            present: std::array::from_fn(|_| AtomicBool::new(false)),
            order: std::array::from_fn(|_| AtomicU32::new(0)),
            order_count: AtomicU32::new(0),
        }
    }
}

unsafe fn bus_system(handle: *mut c_void) -> Option<&'static BusSystemState> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &*handle.cast::<BusSystemState>() })
    }
}

unsafe fn bus_system_mut(handle: *mut c_void) -> Option<&'static mut BusSystemState> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &mut *handle.cast::<BusSystemState>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_bus_system_create() -> *mut c_void {
    Box::into_raw(Box::new(BusSystemState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<BusSystemState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_bind(
    handle: *mut c_void,
    bus_id: u32,
    bus_audio_state: *mut c_void,
) -> bool {
    let Some(state) = (unsafe { bus_system_mut(handle) }) else {
        return false;
    };
    if bus_id as usize >= MAX_BUSES || bus_audio_state.is_null() {
        return false;
    }
    // Bindings are installed during engine construction before audio starts.
    state.buses[bus_id as usize] = bus_audio_state as usize;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_register(handle: *mut c_void, bus_id: u32) -> bool {
    let Some(state) = (unsafe { bus_system(handle) }) else {
        return false;
    };
    if bus_id as usize >= MAX_BUSES || state.buses[bus_id as usize] == 0 {
        return false;
    }
    state.present[bus_id as usize].store(true, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_reconcile(
    handle: *mut c_void,
    active: *const bool,
    count: u32,
) -> bool {
    let Some(state) = (unsafe { bus_system(handle) }) else {
        return false;
    };
    if active.is_null() || count as usize != MAX_BUSES {
        return false;
    }
    for index in 0..MAX_BUSES {
        if unsafe { *active.add(index) } && state.buses[index] == 0 {
            return false;
        }
    }
    for index in 0..MAX_BUSES {
        state.present[index].store(unsafe { *active.add(index) }, Ordering::Release);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_reset(handle: *mut c_void) {
    let Some(state) = (unsafe { bus_system(handle) }) else {
        return;
    };
    for present in &state.present {
        present.store(false, Ordering::Release);
    }
    state.order_count.store(0, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_is_present(handle: *const c_void, bus_id: u32) -> bool {
    let Some(state) = (unsafe { handle.cast::<BusSystemState>().as_ref() }) else {
        return false;
    };
    if bus_id as usize >= MAX_BUSES {
        return false;
    }
    state.present[bus_id as usize].load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_update_routing(
    handle: *mut c_void,
    ids: *const u32,
    count: usize,
) -> bool {
    let Some(state) = (unsafe { bus_system(handle) }) else {
        return false;
    };
    let count = count.min(MAX_BUSES);
    if count > 0 && ids.is_null() {
        return false;
    }
    for index in 0..count {
        state.order[index].store(unsafe { *ids.add(index) }, Ordering::Relaxed);
    }
    state.order_count.store(count as u32, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_process(handle: *const c_void, frames: u32) {
    let Some(state) = (unsafe { handle.cast::<BusSystemState>().as_ref() }) else {
        return;
    };
    if frames == 0 || frames as usize > MAX_BUS_FRAMES {
        return;
    }
    let mut visited = [false; MAX_BUSES];
    let count = (state.order_count.load(Ordering::Acquire) as usize).min(MAX_BUSES);
    for index in 0..count {
        let id = state.order[index].load(Ordering::Relaxed) as usize;
        if id < MAX_BUSES && !visited[id] {
            visited[id] = true;
            if state.present[id].load(Ordering::Acquire) {
                unsafe { hirari_bus_audio_commit_dry(state.buses[id] as *mut c_void, frames) };
            }
        }
    }
    for (id, was_visited) in visited.iter().enumerate() {
        if !was_visited && state.present[id].load(Ordering::Acquire) {
            unsafe { hirari_bus_audio_commit_dry(state.buses[id] as *mut c_void, frames) };
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bus_system_clear(handle: *const c_void, frames: u32) {
    let Some(state) = (unsafe { handle.cast::<BusSystemState>().as_ref() }) else {
        return;
    };
    let frames = (frames as usize).min(MAX_BUS_FRAMES) as u32;
    for (id, present) in state.present.iter().enumerate() {
        if present.load(Ordering::Acquire) {
            unsafe { hirari_bus_audio_clear(state.buses[id] as *mut c_void, frames) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rust_bus_system_preserves_route_order_membership_and_clear() {
        let system = super::hirari_bus_system_create();
        let first = super::hirari_bus_audio_create();
        let second = super::hirari_bus_audio_create();
        let first_l = [0.25_f32; 2];
        let first_r = [-0.25_f32; 2];
        let second_l = [0.5_f32; 2];
        let second_r = [-0.5_f32; 2];
        unsafe {
            assert!(super::hirari_bus_system_bind(system, 2, first));
            assert!(super::hirari_bus_system_bind(system, 5, second));
            assert!(super::hirari_bus_system_register(system, 2));
            assert!(super::hirari_bus_system_register(system, 5));
            assert!(super::hirari_bus_audio_accumulate(
                first,
                first_l.as_ptr(),
                first_r.as_ptr(),
                2,
                1.0,
            ));
            assert!(super::hirari_bus_audio_accumulate(
                second,
                second_l.as_ptr(),
                second_r.as_ptr(),
                2,
                1.0,
            ));
            let order = [99_u32, 5, 5];
            assert!(super::hirari_bus_system_update_routing(
                system,
                order.as_ptr(),
                order.len(),
            ));
            super::hirari_bus_system_process(system, 2);
            assert!(super::hirari_bus_system_is_present(system, 2));
            assert!(super::hirari_bus_system_is_present(system, 5));
            let mut out_l = [0.0_f32; 2];
            let mut out_r = [0.0_f32; 2];
            assert!(super::hirari_bus_audio_read(
                first,
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                2,
                true,
            ));
            assert_eq!(out_l, first_l);
            assert!(super::hirari_bus_audio_read(
                second,
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                2,
                true,
            ));
            assert_eq!(out_l, second_l);
            super::hirari_bus_system_clear(system, 2);
            assert_eq!(super::hirari_bus_audio_samples(first), 0);
            assert_eq!(super::hirari_bus_audio_samples(second), 0);
            let mut active = [false; 128];
            active[2] = true;
            assert!(super::hirari_bus_system_reconcile(
                system,
                active.as_ptr(),
                active.len() as u32,
            ));
            assert!(super::hirari_bus_system_is_present(system, 2));
            assert!(!super::hirari_bus_system_is_present(system, 5));
            super::hirari_bus_system_reset(system);
            assert!(!super::hirari_bus_system_is_present(system, 2));
            super::hirari_bus_system_destroy(system);
            super::hirari_bus_audio_destroy(first);
            super::hirari_bus_audio_destroy(second);
        }
    }

    #[test]
    fn rust_owned_bus_runs_pre_post_lifecycle() {
        let state = super::hirari_bus_audio_create();
        assert!(!state.is_null());
        let left = [0.5_f32, f32::NAN, -0.25];
        let right = [-0.5_f32, 0.25, f32::INFINITY];
        unsafe {
            assert!(super::hirari_bus_audio_accumulate(
                state,
                left.as_ptr(),
                right.as_ptr(),
                3,
                0.5,
            ));
            assert_eq!(super::hirari_bus_audio_samples(state), 3);
            assert!(super::hirari_bus_audio_commit_dry(state, 3));
        }
        let mut pre_left = [0.0_f32; 3];
        let mut pre_right = [0.0_f32; 3];
        let mut post_left = [0.0_f32; 3];
        let mut post_right = [0.0_f32; 3];
        unsafe {
            assert!(super::hirari_bus_audio_read(
                state,
                pre_left.as_mut_ptr(),
                pre_right.as_mut_ptr(),
                3,
                false,
            ));
            assert!(super::hirari_bus_audio_read(
                state,
                post_left.as_mut_ptr(),
                post_right.as_mut_ptr(),
                3,
                true,
            ));
        }
        assert_eq!(pre_left, [0.25, 0.0, -0.125]);
        assert_eq!(pre_right, [-0.25, 0.125, 0.0]);
        assert_eq!(post_left, pre_left);
        assert_eq!(post_right, pre_right);
        unsafe {
            super::hirari_bus_audio_clear(state, 3);
            assert_eq!(super::hirari_bus_audio_samples(state), 0);
            super::hirari_bus_audio_destroy(state);
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_bus_accumulate_stereo_reference(
            bus_left: *mut f32,
            bus_right: *mut f32,
            input_left: *const f32,
            input_right: *const f32,
            frames: u32,
            gain: f32,
        ) -> bool;
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn realtime_bus_accumulation_matches_cpp_reference() {
        let input_left = [0.5_f32, f32::NAN, -1.5, f32::INFINITY, 0.0];
        let input_right = [-0.25_f32, 1.0, f32::NEG_INFINITY, 0.5, 2.0];
        let mut rust_left = [0.1_f32, 0.2, 0.3, 0.4, 0.5];
        let mut rust_right = [0.5_f32, 0.4, 0.3, 0.2, 0.1];
        let mut cpp_left = rust_left;
        let mut cpp_right = rust_right;
        unsafe {
            assert!(super::hirari_bus_accumulate_stereo(
                rust_left.as_mut_ptr(),
                rust_right.as_mut_ptr(),
                input_left.as_ptr(),
                input_right.as_ptr(),
                5,
                0.75,
            ));
            assert!(hirari_bus_accumulate_stereo_reference(
                cpp_left.as_mut_ptr(),
                cpp_right.as_mut_ptr(),
                input_left.as_ptr(),
                input_right.as_ptr(),
                5,
                0.75,
            ));
        }
        assert_eq!(rust_left.map(f32::to_bits), cpp_left.map(f32::to_bits));
        assert_eq!(rust_right.map(f32::to_bits), cpp_right.map(f32::to_bits));
    }

    #[test]
    fn bus_accumulation_rejects_invalid_bounds_and_gain() {
        assert!(!unsafe {
            super::hirari_bus_accumulate_stereo(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                1.0,
            )
        });
        let mut left = [0.0_f32; 1];
        let mut right = [0.0_f32; 1];
        let input = [1.0_f32; 1];
        assert!(!unsafe {
            super::hirari_bus_accumulate_stereo(
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                input.as_ptr(),
                input.as_ptr(),
                0,
                1.0,
            )
        });
        assert!(!unsafe {
            super::hirari_bus_accumulate_stereo(
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                input.as_ptr(),
                input.as_ptr(),
                1,
                f32::NAN,
            )
        });
    }
}
