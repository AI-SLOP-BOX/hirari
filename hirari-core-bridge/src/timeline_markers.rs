//! Rust-owned timeline marker storage shared by native UI and project code.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Eq, PartialEq)]
struct Marker {
    sample_position: u64,
    name: Vec<u8>,
}

#[repr(C)]
struct TimelineState {
    markers: Mutex<Vec<Marker>>,
    playhead: AtomicU64,
    playing: AtomicBool,
    sample_rate: AtomicU64,
    bpm: AtomicU64,
}

#[no_mangle]
pub extern "C" fn hirari_timeline_markers_create() -> *mut c_void {
    Box::into_raw(Box::new(TimelineState {
        markers: Mutex::new(Vec::new()),
        playhead: AtomicU64::new(0),
        playing: AtomicBool::new(false),
        sample_rate: AtomicU64::new(44_100.0_f64.to_bits()),
        bpm: AtomicU64::new(120.0_f64.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_markers_destroy(store: *mut c_void) {
    if !store.is_null() {
        drop(unsafe { Box::from_raw(store.cast::<TimelineState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_markers_add(
    store: *mut c_void,
    sample_position: u64,
    name: *const u8,
    name_size: usize,
) -> bool {
    if store.is_null() || name.is_null() || name_size == 0 {
        return false;
    }
    let input = unsafe { std::slice::from_raw_parts(name, name_size) };
    let first = input
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
    let Some(first) = first else {
        return false;
    };
    let last = input
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap();
    let normalized = &input[first..=last];
    if normalized.len() > 256 || normalized.contains(&0) {
        return false;
    }
    let mut markers = unsafe { &*store.cast::<TimelineState>() }
        .markers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let marker = Marker {
        sample_position,
        name: normalized.to_vec(),
    };
    if markers.iter().any(|existing| existing == &marker) {
        return false;
    }
    markers.push(marker);
    markers.sort_by_key(|marker| marker.sample_position);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_markers_remove_at(
    store: *mut c_void,
    sample_position: u64,
) -> bool {
    if store.is_null() {
        return false;
    }
    let mut markers = unsafe { &*store.cast::<TimelineState>() }
        .markers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(index) = markers
        .iter()
        .position(|marker| marker.sample_position == sample_position)
    else {
        return false;
    };
    markers.remove(index);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_markers_normalize(store: *mut c_void) {
    if store.is_null() {
        return;
    }
    let mut markers = unsafe { &*store.cast::<TimelineState>() }
        .markers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    markers.sort_by_key(|marker| marker.sample_position);
    markers.dedup();
}

pub type MarkerVisitor = unsafe extern "C" fn(*mut c_void, u64, *const u8, usize) -> bool;

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_markers_visit(
    store: *const c_void,
    context: *mut c_void,
    visitor: Option<MarkerVisitor>,
) -> usize {
    if store.is_null() {
        return 0;
    }
    let Some(visitor) = visitor else {
        return 0;
    };
    let markers = unsafe { &*store.cast::<TimelineState>() }
        .markers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for marker in markers.iter() {
        let keep_going = unsafe {
            visitor(
                context,
                marker.sample_position,
                marker.name.as_ptr(),
                marker.name.len(),
            )
        };
        if !keep_going {
            break;
        }
    }
    markers.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_get_playhead(state: *const c_void) -> u64 {
    if state.is_null() {
        return 0;
    }
    unsafe { &*state.cast::<TimelineState>() }
        .playhead
        .load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_set_playhead(state: *mut c_void, sample: u64) {
    if state.is_null() {
        return;
    }
    unsafe { &*state.cast::<TimelineState>() }
        .playhead
        .store(sample, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_process(state: *mut c_void, frames: u32) {
    if state.is_null() || frames == 0 {
        return;
    }
    let state = unsafe { &*state.cast::<TimelineState>() };
    if !state.playing.load(Ordering::Acquire) {
        return;
    }
    let mut current = state.playhead.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(u64::from(frames));
        match state.playhead.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_prepare(
    state: *mut c_void,
    sample_rate: f64,
    block_size: u32,
) -> bool {
    if state.is_null()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || block_size == 0
        || block_size > 4096
    {
        return false;
    }
    let state = unsafe { &*state.cast::<TimelineState>() };
    state
        .sample_rate
        .store(sample_rate.to_bits(), Ordering::Release);
    state.playhead.store(0, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_sample_rate(state: *const c_void) -> f64 {
    if state.is_null() {
        return 0.0;
    }
    f64::from_bits(
        unsafe { &*state.cast::<TimelineState>() }
            .sample_rate
            .load(Ordering::Acquire),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_is_playing(state: *const c_void) -> bool {
    !state.is_null()
        && unsafe { &*state.cast::<TimelineState>() }
            .playing
            .load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_set_playing(state: *mut c_void, playing: bool) {
    if state.is_null() {
        return;
    }
    unsafe { &*state.cast::<TimelineState>() }
        .playing
        .store(playing, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_bpm(state: *const c_void) -> f64 {
    if state.is_null() {
        return 120.0;
    }
    f64::from_bits(
        unsafe { &*state.cast::<TimelineState>() }
            .bpm
            .load(Ordering::Acquire),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_timeline_state_set_bpm(state: *mut c_void, bpm: f64) {
    if state.is_null() || !bpm.is_finite() || !(20.0..=300.0).contains(&bpm) {
        return;
    }
    unsafe { &*state.cast::<TimelineState>() }
        .bpm
        .store(bpm.to_bits(), Ordering::Release);
}
