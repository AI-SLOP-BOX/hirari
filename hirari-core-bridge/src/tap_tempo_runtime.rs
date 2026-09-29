use std::ffi::c_void;

/// Stateful compatibility implementation for the native transport's tap API.
/// Mutations happen on the control/UI thread, never in the audio callback.
struct TapTempoRuntime {
    taps: Vec<u64>,
    max_taps: usize,
}

impl TapTempoRuntime {
    fn new(max_taps: usize) -> Self {
        let max_taps = max_taps.max(2);
        Self {
            taps: Vec::new(),
            max_taps,
        }
    }

    fn tap(&mut self, timestamp_ms: u64) {
        if self.taps.last().is_some_and(|&last| timestamp_ms <= last) {
            return;
        }
        self.taps.push(timestamp_ms);
        if self.taps.len() > self.max_taps {
            self.taps.remove(0);
        }
    }

    fn bpm(&self) -> f64 {
        if self.taps.len() < 2 {
            return 0.0;
        }
        let mut total = 0.0;
        let mut count = 0usize;
        for interval in self.taps.windows(2).map(|pair| pair[1] - pair[0]) {
            if (100..=5000).contains(&interval) {
                total += interval as f64;
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            (60_000.0 / (total / count as f64)).clamp(20.0, 300.0)
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_tap_tempo_create(max_taps: usize) -> *mut c_void {
    Box::into_raw(Box::new(TapTempoRuntime::new(max_taps))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tap_tempo_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<TapTempoRuntime>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tap_tempo_tap(state: *mut c_void, timestamp_ms: u64) {
    if let Some(runtime) = unsafe { state.cast::<TapTempoRuntime>().as_mut() } {
        runtime.tap(timestamp_ms);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tap_tempo_clear(state: *mut c_void) {
    if let Some(runtime) = unsafe { state.cast::<TapTempoRuntime>().as_mut() } {
        runtime.taps.clear();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tap_tempo_bpm(state: *const c_void) -> f64 {
    unsafe { state.cast::<TapTempoRuntime>().as_ref() }.map_or(0.0, TapTempoRuntime::bpm)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tap_tempo_count(state: *const c_void) -> usize {
    unsafe { state.cast::<TapTempoRuntime>().as_ref() }.map_or(0, |runtime| runtime.taps.len())
}
