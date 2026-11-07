//! Frozen Hann-window STFT/OLA spectral gain reference.

use crate::fft_utils::hirari_fft_utils_process;

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

unsafe fn fft_process(values: &mut [Complex], inverse: bool) {
    hirari_fft_utils_process(
        values.as_mut_ptr() as *mut f32,
        values.len(),
        inverse,
    );
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

