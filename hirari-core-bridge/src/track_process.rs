use std::ffi::c_void;
use std::slice;

pub(crate) const MAX_TRACK_BLOCK_FRAMES: u32 = 65_536;
const MAX_TRACK_FREEZE_SAMPLES: u64 = 64 * 1024 * 1024;

pub type TrackFreezeReset = unsafe extern "C" fn(*mut c_void) -> bool;
pub type TrackFreezeProcess = unsafe extern "C" fn(
    *mut c_void,
    *mut f32,
    *mut f32,
    u32,
    u64,
) -> bool;

/// Owns the bounded offline freeze render loop. Native callbacks only reset
/// and render one track block into caller-provided scratch storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_render(
    total_samples: u64,
    block_size: u32,
    output_left: *mut f32,
    output_right: *mut f32,
    output_capacity: usize,
    context: *mut c_void,
    reset: Option<TrackFreezeReset>,
    process: Option<TrackFreezeProcess>,
) -> bool {
    let render = || {
        let (Some(reset), Some(process)) = (reset, process) else {
            return false;
        };
        let Ok(total_samples) = usize::try_from(total_samples) else {
            return false;
        };
        if total_samples == 0
            || total_samples as u64 > MAX_TRACK_FREEZE_SAMPLES
            || total_samples > output_capacity
            || output_left.is_null()
            || output_right.is_null()
            || block_size == 0
            || block_size > MAX_TRACK_BLOCK_FRAMES
            || context.is_null()
        {
            return false;
        }
        let Some(output_bytes) = total_samples.checked_mul(std::mem::size_of::<f32>()) else {
            return false;
        };
        let left_start = output_left as usize;
        let right_start = output_right as usize;
        let (Some(left_end), Some(right_end)) = (
            left_start.checked_add(output_bytes),
            right_start.checked_add(output_bytes),
        ) else {
            return false;
        };
        if left_start < right_end && right_start < left_end {
            return false;
        }
        let mut left_block = vec![0.0f32; block_size as usize];
        let mut right_block = vec![0.0f32; block_size as usize];
        if !unsafe { reset(context) } {
            return false;
        }
        let left = unsafe { slice::from_raw_parts_mut(output_left, total_samples) };
        let right = unsafe { slice::from_raw_parts_mut(output_right, total_samples) };
        let mut offset = 0usize;
        while offset < total_samples {
            let frames = (total_samples - offset).min(block_size as usize);
            if !unsafe {
                process(
                    context,
                    left_block.as_mut_ptr(),
                    right_block.as_mut_ptr(),
                    frames as u32,
                    offset as u64,
                )
            } {
                return false;
            }
            left[offset..offset + frames].copy_from_slice(&left_block[..frames]);
            right[offset..offset + frames].copy_from_slice(&right_block[..frames]);
            offset += frames;
        }
        true
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(render)).unwrap_or(false)
}

/// Streams a completed stereo freeze buffer to a Float32 WAV without making
/// another full-length copy. The writer remains Rust-owned alongside the
/// bounded render loop above.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_write_float32(
    path: *const u8,
    path_size: usize,
    sample_rate: u32,
    left: *const f32,
    right: *const f32,
    total_samples: u64,
) -> bool {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024
        || left.is_null() || right.is_null() || sample_rate == 0
        || total_samples == 0 || total_samples > MAX_TRACK_FREEZE_SAMPLES
    {
        return false;
    }
    let Ok(total_samples) = usize::try_from(total_samples) else { return false };
    let left = unsafe { slice::from_raw_parts(left, total_samples) };
    let right = unsafe { slice::from_raw_parts(right, total_samples) };
    crate::export::write_stereo_float32_stream(path, path_size, sample_rate, left, right)
}

