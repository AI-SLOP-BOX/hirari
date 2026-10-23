use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Complex { re: f32, im: f32 }

static TWIDDLES: OnceLock<Mutex<HashMap<usize, Arc<Vec<Complex>>>>> = OnceLock::new();

fn twiddles(n: usize) -> Arc<Vec<Complex>> {
    let cache = TWIDDLES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.entry(n).or_insert_with(|| {
        let mut values = Vec::with_capacity(n.saturating_sub(1));
        let mut len = 2usize;
        while len <= n {
            for j in 0..len / 2 {
                let angle = (-2.0f64 * std::f64::consts::PI * j as f64 / len as f64) as f32;
                values.push(Complex { re: angle.cos(), im: angle.sin() });
            }
            let Some(next) = len.checked_mul(2) else { break };
            len = next;
        }
        Arc::new(values)
    }).clone()
}

fn fft(data: &mut [Complex]) {
    let n = data.len();
    if n <= 1 { return; }
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j ^= bit;
        if i < j { data.swap(i, j); }
    }
    let table = twiddles(n);
    let mut twiddle_index = 0usize;
    let mut len = 2usize;
    while len <= n {
        for i in (0..n).step_by(len) {
            for j in 0..len / 2 {
                let u = data[i + j];
                let a = data[i + j + len / 2];
                let b = table[twiddle_index + j];
                let v = Complex { re: a.re * b.re - a.im * b.im,
                                  im: a.re * b.im + a.im * b.re };
                data[i + j] = Complex { re: u.re + v.re, im: u.im + v.im };
                data[i + j + len / 2] = Complex { re: u.re - v.re, im: u.im - v.im };
            }
        }
        twiddle_index += len / 2;
        let Some(next) = len.checked_mul(2) else { break };
        len = next;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fft_utils_process(data: *mut f32, n: usize, inverse: bool) -> bool {
    if n <= 1 { return true; }
    if data.is_null() || n > isize::MAX as usize / (2 * std::mem::size_of::<f32>()) { return false; }
    let values = std::slice::from_raw_parts_mut(data.cast::<Complex>(), n);
    if inverse {
        for value in values.iter_mut() { value.im = -value.im; }
        fft(values);
        let scale = 1.0f32 / n as f32;
        for value in values.iter_mut() { value.im = -value.im; value.re *= scale; value.im *= scale; }
    } else { fft(values); }
    true
}
