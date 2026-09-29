use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MAX_TRACKS: usize = 1024;
const MAX_PARAMS_PER_TRACK: usize = 512;
const MASK_WORDS: usize = MAX_PARAMS_PER_TRACK / 64;

struct TrackAutomationState {
    current_bits: [AtomicU32; MAX_PARAMS_PER_TRACK],
    target_bits: [AtomicU32; MAX_PARAMS_PER_TRACK],
    active_mask: [AtomicU64; MASK_WORDS],
}

impl TrackAutomationState {
    fn new() -> Self {
        Self {
            current_bits: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            target_bits: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            active_mask: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

struct AutomationManagerState {
    tracks: Box<[TrackAutomationState]>,
    active_tracks: AtomicU32,
    global_coeff_bits: AtomicU32,
}

impl AutomationManagerState {
    fn new() -> Self {
        let tracks = (0..MAX_TRACKS)
            .map(|_| TrackAutomationState::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            tracks,
            active_tracks: AtomicU32::new(0),
            global_coeff_bits: AtomicU32::new(0.05_f32.to_bits()),
        }
    }

    fn prepare(&self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        let coeff = 1.0 - (-1.0 / (sample_rate as f32 * 0.010)).exp();
        let coeff = if coeff.is_finite() {
            coeff.clamp(0.0, 1.0)
        } else {
            0.05
        };
        self.global_coeff_bits
            .store(coeff.to_bits(), Ordering::Release);
    }

    fn process(&self, num_samples: u32) {
        if num_samples == 0 {
            return;
        }
        let coefficient = f32::from_bits(self.global_coeff_bits.load(Ordering::Acquire));
        let block_coeff = 1.0 - (1.0 - coefficient).powf(num_samples as f32);
        let active_tracks = self.active_tracks.load(Ordering::Acquire) as usize;
        for track in self.tracks.iter().take(active_tracks) {
            for (word_index, mask) in track.active_mask.iter().enumerate() {
                let mut active = mask.load(Ordering::Acquire);
                while active != 0 {
                    let bit = active.trailing_zeros() as usize;
                    let parameter = word_index * 64 + bit;
                    let bit_mask = 1_u64 << bit;
                    let current =
                        f32::from_bits(track.current_bits[parameter].load(Ordering::Relaxed));
                    let target =
                        f32::from_bits(track.target_bits[parameter].load(Ordering::Relaxed));
                    let next = current + (target - current) * block_coeff;
                    let resolved = if next.is_finite() { next } else { target };
                    track.current_bits[parameter].store(resolved.to_bits(), Ordering::Release);
                    if (resolved - target).abs() <= 1.0e-5 {
                        mask.fetch_and(!bit_mask, Ordering::Release);
                        if track.target_bits[parameter].load(Ordering::Acquire) != target.to_bits()
                        {
                            mask.fetch_or(bit_mask, Ordering::Release);
                        }
                    }
                    active &= active - 1;
                }
            }
        }
    }

    fn set_target(&self, track_id: u32, parameter_id: u32, value: f32) -> bool {
        if track_id as usize >= MAX_TRACKS
            || parameter_id as usize >= MAX_PARAMS_PER_TRACK
            || !value.is_finite()
        {
            return false;
        }
        let track = &self.tracks[track_id as usize];
        track.target_bits[parameter_id as usize].store(value.to_bits(), Ordering::Relaxed);
        track.active_mask[parameter_id as usize / 64]
            .fetch_or(1_u64 << (parameter_id as usize % 64), Ordering::Release);
        let required = track_id + 1;
        let mut active = self.active_tracks.load(Ordering::Relaxed);
        while active < required {
            match self.active_tracks.compare_exchange_weak(
                active,
                required,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => active = actual,
            }
        }
        true
    }

    fn reset(&self) {
        for track in self.tracks.iter() {
            for value in &track.current_bits {
                value.store(0.0_f32.to_bits(), Ordering::Relaxed);
            }
            for value in &track.target_bits {
                value.store(0.0_f32.to_bits(), Ordering::Relaxed);
            }
            for mask in &track.active_mask {
                mask.store(0, Ordering::Relaxed);
            }
        }
        self.active_tracks.store(0, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_manager_create() -> *mut c_void {
    Box::into_raw(Box::new(AutomationManagerState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<AutomationManagerState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_prepare(state: *const c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<AutomationManagerState>().as_ref() } {
        state.prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_process(state: *const c_void, num_samples: u32) {
    if let Some(state) = unsafe { state.cast::<AutomationManagerState>().as_ref() } {
        state.process(num_samples);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_get_value(
    state: *const c_void,
    track_id: u32,
    parameter_id: u32,
) -> f32 {
    let Some(state) = (unsafe { state.cast::<AutomationManagerState>().as_ref() }) else {
        return 0.0;
    };
    if track_id as usize >= MAX_TRACKS || parameter_id as usize >= MAX_PARAMS_PER_TRACK {
        return 0.0;
    }
    f32::from_bits(
        state.tracks[track_id as usize].current_bits[parameter_id as usize].load(Ordering::Relaxed),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_set_target(
    state: *const c_void,
    track_id: u32,
    parameter_id: u32,
    value: f32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<AutomationManagerState>().as_ref() }) else {
        return false;
    };
    state.set_target(track_id, parameter_id, value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_get_target(
    state: *const c_void,
    track_id: u32,
    parameter_id: u32,
) -> f32 {
    let Some(state) = (unsafe { state.cast::<AutomationManagerState>().as_ref() }) else {
        return 0.0;
    };
    if track_id as usize >= MAX_TRACKS || parameter_id as usize >= MAX_PARAMS_PER_TRACK {
        return 0.0;
    }
    f32::from_bits(
        state.tracks[track_id as usize].target_bits[parameter_id as usize].load(Ordering::Relaxed),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_manager_reset(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<AutomationManagerState>().as_ref() } {
        state.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_automation_manager_create, hirari_automation_manager_destroy,
        hirari_automation_manager_get_target, hirari_automation_manager_get_value,
        hirari_automation_manager_prepare, hirari_automation_manager_process,
        hirari_automation_manager_reset, hirari_automation_manager_set_target,
    };

    #[test]
    fn block_smoothing_tracks_active_parameters_and_reset_clears_them() {
        unsafe {
            let state = hirari_automation_manager_create();
            assert!(!state.is_null());
            hirari_automation_manager_prepare(state, 48_000.0);
            assert!(hirari_automation_manager_set_target(state, 2, 17, 1.0));
            assert_eq!(hirari_automation_manager_get_target(state, 2, 17), 1.0);
            assert_eq!(hirari_automation_manager_get_value(state, 2, 17), 0.0);
            hirari_automation_manager_process(state, 128);
            let value = hirari_automation_manager_get_value(state, 2, 17);
            assert!(value > 0.0 && value < 1.0);
            hirari_automation_manager_reset(state);
            assert_eq!(hirari_automation_manager_get_value(state, 2, 17), 0.0);
            assert_eq!(hirari_automation_manager_get_target(state, 2, 17), 0.0);
            assert!(!hirari_automation_manager_set_target(state, 1024, 0, 1.0));
            hirari_automation_manager_destroy(state);
        }
    }
}
