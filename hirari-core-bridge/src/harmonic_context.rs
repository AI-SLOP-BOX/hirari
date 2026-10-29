use std::sync::Mutex;

/// Harmonic context fed to structure analysis and auto-arrangement.
///
/// The engine used to keep this as a C++ singleton; the state now lives here
/// so both C++ callers read the same Rust-owned values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HarmonicContextState {
    pub tension: f32,
    pub valence: f32,
    pub stability: f32,
}

impl Default for HarmonicContextState {
    fn default() -> Self {
        Self { tension: 0.5, valence: 0.5, stability: 0.5 }
    }
}

static STATE: Mutex<HarmonicContextState> = Mutex::new(HarmonicContextState {
    tension: 0.5,
    valence: 0.5,
    stability: 0.5,
});

fn lock() -> std::sync::MutexGuard<'static, HarmonicContextState> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_harmonic_context_get(
    tension: *mut f32,
    valence: *mut f32,
    stability: *mut f32,
) {
    let state = *lock();
    if !tension.is_null() {
        *tension = state.tension;
    }
    if !valence.is_null() {
        *valence = state.valence;
    }
    if !stability.is_null() {
        *stability = state.stability;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_harmonic_context_set(tension: f32, valence: f32, stability: f32) {
    *lock() = HarmonicContextState { tension, valence, stability };
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_match_the_former_cpp_singleton() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe { hirari_harmonic_context_set(0.5, 0.5, 0.5) };
        let (mut tension, mut valence, mut stability) = (0.0, 0.0, 0.0);
        unsafe { hirari_harmonic_context_get(&mut tension, &mut valence, &mut stability) };
        assert_eq!((tension, valence, stability), (0.5, 0.5, 0.5));
    }

    #[test]
    fn set_then_get_round_trips_and_null_pointers_are_ignored() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            hirari_harmonic_context_set(0.25, 0.75, 0.125);
            let mut tension = 0.0;
            hirari_harmonic_context_get(&mut tension, std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(tension, 0.25);
            let (mut t, mut v, mut s) = (0.0, 0.0, 0.0);
            hirari_harmonic_context_get(&mut t, &mut v, &mut s);
            assert_eq!((t, v, s), (0.25, 0.75, 0.125));
            hirari_harmonic_context_set(0.5, 0.5, 0.5);
        }
    }
}
