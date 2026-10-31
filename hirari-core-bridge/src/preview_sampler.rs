//! One-shot preview-sample store and playback cursor. The control thread
//! publishes an immutable sample buffer and owns retired generations; the
//! audio callback only loads the published pointer and advances the cursor
//! through atomics, so replacing a sample never frees memory under it.

use crate::preview_sample::render_into;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::Mutex;

struct PreviewSampler {
    // Published buffer. Audio reads it lock-free; only control code writes it.
    published: AtomicPtr<f32>,
    len: AtomicU64,
    rate_bits: AtomicU64,
    trigger: AtomicBool,
    playing: AtomicBool,
    position_bits: AtomicU64,
    // Superseded buffers stay allocated until reclaim runs, which the caller
    // only does after the audio callbacks are known to be quiescent.
    owned: Mutex<Vec<Box<[f32]>>>,
}

unsafe fn sampler(handle: *const c_void) -> Option<&'static PreviewSampler> {
    if handle.is_null() {
        None
    } else {
        // SAFETY: handles are allocated and released by this module.
        Some(unsafe { &*handle.cast::<PreviewSampler>() })
    }
}

fn state(handle: *const c_void) -> PreviewSamplerState {
    PreviewSamplerState { sampler: unsafe { sampler(handle) } }
}

struct PreviewSamplerState {
    sampler: Option<&'static PreviewSampler>,
}

impl PreviewSamplerState {
    fn clear(&self) {
        let Some(sampler) = self.sampler else { return };
        sampler.published.store(std::ptr::null_mut(), Ordering::Release);
        sampler.len.store(0, Ordering::Release);
        sampler.trigger.store(false, Ordering::Release);
        sampler.playing.store(false, Ordering::Release);
        sampler.position_bits.store(0.0f64.to_bits(), Ordering::Release);
    }

    fn set(&self, samples: *const f32, count: usize, source_rate: f64) {
        if samples.is_null() || count == 0 || !source_rate.is_finite() || source_rate <= 0.0 {
            self.clear();
            return;
        }
        let Some(sampler) = self.sampler else { return };
        // SAFETY: the caller guarantees `count` readable floats.
        let source = unsafe { std::slice::from_raw_parts(samples, count) };
        let buffer = source.to_vec().into_boxed_slice();
        let pointer = buffer.as_ptr() as *mut f32;
        let len = buffer.len() as u64;
        let Ok(mut owned) = sampler.owned.lock() else { return };
        sampler.len.store(len, Ordering::Release);
        sampler.rate_bits.store(source_rate.to_bits(), Ordering::Release);
        owned.push(buffer);
        sampler.published.store(pointer, Ordering::Release);
    }

    fn reclaim(&self) {
        let Some(sampler) = self.sampler else { return };
        let published = sampler.published.load(Ordering::Acquire);
        let Ok(mut owned) = sampler.owned.lock() else { return };
        owned.retain(|buffer| buffer.as_ptr() as *const f32 == published);
    }

    fn render(&self, target_rate: f64, left: *mut f32, right: *mut f32, frames: u32) {
        let Some(sampler) = self.sampler else { return };
        if sampler.trigger.swap(false, Ordering::AcqRel) {
            sampler.position_bits.store(0.0f64.to_bits(), Ordering::Release);
            sampler.playing.store(true, Ordering::Release);
        }
        let published = sampler.published.load(Ordering::Acquire);
        let len = sampler.len.load(Ordering::Acquire) as usize;
        if published.is_null() || len == 0 || !sampler.playing.load(Ordering::Acquire) {
            sampler.position_bits.store(0.0f64.to_bits(), Ordering::Release);
            sampler.playing.store(false, Ordering::Release);
            return;
        }
        // SAFETY: the published buffer stays allocated until reclaim, which
        // only runs while no callback is in flight.
        let samples = unsafe { std::slice::from_raw_parts(published, len) };
        let (left, right) = unsafe {
            (
                std::slice::from_raw_parts_mut(left, frames as usize),
                std::slice::from_raw_parts_mut(right, frames as usize),
            )
        };
        let mut position = f64::from_bits(sampler.position_bits.load(Ordering::Acquire));
        let source_rate = f64::from_bits(sampler.rate_bits.load(Ordering::Acquire));
        let still_playing =
            unsafe { render_into(samples, source_rate, target_rate, left, right, &mut position) };
        sampler.position_bits.store(position.to_bits(), Ordering::Release);
        sampler.playing.store(still_playing, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn hirari_preview_sampler_create() -> *mut c_void {
    Box::into_raw(Box::new(PreviewSampler {
        published: AtomicPtr::new(std::ptr::null_mut()),
        len: AtomicU64::new(0),
        rate_bits: AtomicU64::new(0.0f64.to_bits()),
        trigger: AtomicBool::new(false),
        playing: AtomicBool::new(false),
        position_bits: AtomicU64::new(0.0f64.to_bits()),
        owned: Mutex::new(Vec::new()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: handle was returned by `hirari_preview_sampler_create`.
        drop(unsafe { Box::from_raw(handle.cast::<PreviewSampler>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_set(
    handle: *const c_void,
    samples: *const f32,
    count: usize,
    source_rate: f64,
) {
    state(handle).set(samples, count, source_rate);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_clear(handle: *const c_void) {
    state(handle).clear();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_trigger(handle: *const c_void) {
    if let Some(sampler) = unsafe { sampler(handle) } {
        sampler.trigger.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_reclaim(handle: *const c_void) {
    state(handle).reclaim();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_is_active(handle: *const c_void) -> bool {
    let Some(sampler) = (unsafe { sampler(handle) }) else { return false };
    sampler.trigger.load(Ordering::Acquire) || sampler.playing.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_preview_sampler_render(
    handle: *const c_void,
    target_rate: f64,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if left.is_null() || right.is_null() {
        return;
    }
    state(handle).render(target_rate, left, right, frames);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(sampler: *const c_void, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        unsafe {
            hirari_preview_sampler_render(
                sampler,
                48000.0,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                frames as u32,
            );
        }
        (left, right)
    }

    #[test]
    fn trigger_renders_the_sample_and_stops_at_the_end() {
        let sampler = hirari_preview_sampler_create();
        let samples = vec![0.5f32; 8];
        unsafe {
            hirari_preview_sampler_set(sampler, samples.as_ptr(), samples.len(), 48000.0);
            hirari_preview_sampler_trigger(sampler);
            assert!(hirari_preview_sampler_is_active(sampler));
        }
        let (left, _) = render(sampler, 4);
        assert!(left.iter().any(|value| *value != 0.0));
        let _ = render(sampler, 4);
        let (tail, _) = render(sampler, 4);
        assert!(tail.iter().all(|value| *value == 0.0));
        unsafe {
            assert!(!hirari_preview_sampler_is_active(sampler));
            hirari_preview_sampler_destroy(sampler);
        }
    }

    #[test]
    fn clear_drops_the_published_buffer_after_reclaim() {
        let sampler = hirari_preview_sampler_create();
        let samples = vec![1.0f32; 4];
        unsafe {
            hirari_preview_sampler_set(sampler, samples.as_ptr(), samples.len(), 44100.0);
            hirari_preview_sampler_trigger(sampler);
            hirari_preview_sampler_clear(sampler);
            hirari_preview_sampler_reclaim(sampler);
            let (left, _) = render(sampler, 4);
            assert!(left.iter().all(|value| *value == 0.0));
            hirari_preview_sampler_destroy(sampler);
        }
    }

    #[test]
    fn invalid_input_clears_the_published_sample() {
        let sampler = hirari_preview_sampler_create();
        let samples = vec![1.0f32; 4];
        unsafe {
            hirari_preview_sampler_set(sampler, samples.as_ptr(), samples.len(), 44100.0);
            hirari_preview_sampler_set(sampler, samples.as_ptr(), samples.len(), -1.0);
            hirari_preview_sampler_trigger(sampler);
            let (left, _) = render(sampler, 4);
            assert!(left.iter().all(|value| *value == 0.0));
            hirari_preview_sampler_destroy(sampler);
        }
    }
}
