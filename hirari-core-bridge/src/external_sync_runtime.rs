use std::ffi::c_void;

/// MIDI clock receive state for external sync. Ticks arrive on the control
/// thread; the audio callback never touches this state.
struct ExternalSyncRuntime {
    bpm: f64,
    ticks: u64,
    last_tick: u64,
}

impl ExternalSyncRuntime {
    fn new() -> Self {
        Self {
            bpm: 120.0,
            ticks: 0,
            last_tick: 0,
        }
    }

    fn set_rate(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 1.0 && bpm < 1000.0 {
            self.bpm = bpm;
        }
    }

    fn tick(&mut self, timestamp: u64) {
        self.last_tick = timestamp;
        self.ticks = self.ticks.wrapping_add(1);
    }

    fn reset(&mut self) {
        self.ticks = 0;
        self.last_tick = 0;
    }
}

#[no_mangle]
pub extern "C" fn hirari_external_sync_create() -> *mut c_void {
    Box::into_raw(Box::new(ExternalSyncRuntime::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ExternalSyncRuntime>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_set_rate(state: *mut c_void, bpm: f64) {
    if let Some(runtime) = unsafe { state.cast::<ExternalSyncRuntime>().as_mut() } {
        runtime.set_rate(bpm);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_rate(state: *const c_void) -> f64 {
    unsafe { state.cast::<ExternalSyncRuntime>().as_ref() }.map_or(120.0, |runtime| runtime.bpm)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_tick(state: *mut c_void, timestamp: u64) {
    if let Some(runtime) = unsafe { state.cast::<ExternalSyncRuntime>().as_mut() } {
        runtime.tick(timestamp);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_ticks(state: *const c_void) -> u64 {
    unsafe { state.cast::<ExternalSyncRuntime>().as_ref() }.map_or(0, |runtime| runtime.ticks)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_last_tick(state: *const c_void) -> u64 {
    unsafe { state.cast::<ExternalSyncRuntime>().as_ref() }.map_or(0, |runtime| runtime.last_tick)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_external_sync_reset(state: *mut c_void) {
    if let Some(runtime) = unsafe { state.cast::<ExternalSyncRuntime>().as_mut() } {
        runtime.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_clock_rates() {
        let mut runtime = ExternalSyncRuntime::new();
        for rejected in [f64::NAN, f64::INFINITY, 1.0, 0.5, 1000.0, -120.0] {
            runtime.set_rate(rejected);
            assert_eq!(runtime.bpm, 120.0, "{rejected} must be rejected");
        }
        runtime.set_rate(140.5);
        assert_eq!(runtime.bpm, 140.5);
    }

    #[test]
    fn ticks_count_and_track_last_timestamp() {
        let mut runtime = ExternalSyncRuntime::new();
        runtime.tick(100);
        runtime.tick(250);
        assert_eq!(runtime.ticks, 2);
        assert_eq!(runtime.last_tick, 250);
        runtime.reset();
        assert_eq!((runtime.ticks, runtime.last_tick), (0, 0));
    }

    #[test]
    fn tick_counter_wraps_instead_of_panicking() {
        let mut runtime = ExternalSyncRuntime::new();
        runtime.ticks = u64::MAX;
        runtime.tick(7);
        assert_eq!(runtime.ticks, 0);
        assert_eq!(runtime.last_tick, 7);
    }

    #[test]
    fn null_state_is_tolerated() {
        unsafe {
            hirari_external_sync_set_rate(std::ptr::null_mut(), 99.0);
            hirari_external_sync_tick(std::ptr::null_mut(), 1);
            hirari_external_sync_reset(std::ptr::null_mut());
            hirari_external_sync_destroy(std::ptr::null_mut());
            assert_eq!(hirari_external_sync_rate(std::ptr::null()), 120.0);
            assert_eq!(hirari_external_sync_ticks(std::ptr::null()), 0);
            assert_eq!(hirari_external_sync_last_tick(std::ptr::null()), 0);
        }
    }
}
