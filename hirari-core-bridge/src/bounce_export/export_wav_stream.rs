use std::io::{Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
#[path = "export_wav_stream_float32.rs"]
mod stream_float32;
#[path = "export_wav_stream_pcm_wave64.rs"]
mod stream_pcm_wave64;
pub(crate) use stream_float32::*;
pub(crate) use stream_pcm_wave64::*;

/// Writes a complete planar buffer through the canonical bounded-memory
/// stream writers. Format values match the native compatibility facade:
/// 0=float32 RIFF/RF64, 1=PCM16, 2=PCM24, 3=WAVE64 float32.
#[no_mangle]

pub unsafe extern "C" fn hirari_wav_write_planar(
    path: *const u8,
    path_size: usize,
    channels: *const *const f32,
    channel_count: usize,
    frames: u64,
    sample_rate: u32,
    format: u8,
) -> u8 {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024
        || channels.is_null() || channel_count == 0 || channel_count > 32
        || frames == 0 || frames > usize::MAX as u64
        || !(8_000..=384_000).contains(&sample_rate) || format > 3
    {
        return 1;
    }
    let channel_data = unsafe { std::slice::from_raw_parts(channels, channel_count) };
    if channel_data.iter().any(|channel| channel.is_null()) {
        return 2;
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    #[cfg(unix)]
    let output = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(path_bytes))
    };
    #[cfg(not(unix))]
    let output = {
        let Ok(path) = std::str::from_utf8(path_bytes) else { return 1 };
        Path::new(path)
    };
    let parent = output.parent().filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if std::fs::create_dir_all(parent).is_err() {
        return 3;
    }
    let path = path.cast::<std::ffi::c_char>();
    let mut stream = match format {
        0 => {
            let handle = unsafe { hirari_wav_float32_stream_create(
                path, path_size, sample_rate, channel_count as u16,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Float32(handle)
        }
        1 | 2 => {
            let handle = unsafe { hirari_wav_pcm_stream_create(
                path, path_size, frames, sample_rate, channel_count as u16,
                if format == 1 { 16 } else { 24 }, 0,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Pcm(handle)
        }
        _ => {
            let handle = unsafe { hirari_wav_wave64_stream_create(
                path, path_size, frames, sample_rate, channel_count as u16,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Wave64(handle)
        }
    };
    let mut block = Vec::with_capacity(channel_count);
    let mut offset = 0u64;
    while offset < frames {
        let count = (frames - offset).min(u64::from(MAX_STREAM_BLOCK_FRAMES)) as u32;
        block.clear();
        for channel in channel_data {
            block.push(unsafe { channel.add(offset as usize) });
        }
        let written = unsafe { stream.write(block.as_ptr(), count) };
        if written == 0 { return 5; }
        offset += u64::from(count);
    }
    if unsafe { stream.finish() } == 0 { return 6; }
    0
}

/// Channel-layout admission for planar exports. Every plane must hold exactly
/// `frames` samples and the channel count must fit the WAVE field width, so a
/// caller never has to re-derive the rule before building its pointer array.
#[no_mangle]
pub unsafe extern "C" fn hirari_wav_planar_shapes_valid(
    sizes: *const usize,
    channel_count: usize,
    frames: u64,
) -> bool {
    if sizes.is_null() || channel_count == 0 || channel_count > u16::MAX as usize || frames == 0 {
        return false;
    }
    let sizes = unsafe { std::slice::from_raw_parts(sizes, channel_count) };
    sizes.iter().all(|size| *size as u64 == frames)
}

enum StreamHandle {
    Float32(*mut std::ffi::c_void),
    Pcm(*mut std::ffi::c_void),
    Wave64(*mut std::ffi::c_void),
}

struct BounceStream {
    stream: StreamHandle,
    frames: u64,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_create(
    path: *const u8, path_size: usize, frames: u64, sample_rate: u32, format: u8,
) -> *mut std::ffi::c_void {
    // Format codes: 0 float32 RIFF/RF64, 1 PCM16, 2 PCM24, 3 WAVE64 float.
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 || frames == 0
        || !(8_000..=384_000).contains(&sample_rate) || format > 3
    { return std::ptr::null_mut(); }
    // Do not create missing parent directories here. The writer's contract is
    // to fail when the destination cannot be opened, and a render must report
    // that failure instead of silently materialising a path the caller never
    // created.
    let cpath = path.cast::<std::ffi::c_char>();
    let stream = match format {
        0 => unsafe { hirari_wav_float32_stream_create(cpath, path_size, sample_rate, 2) },
        1 | 2 => unsafe { hirari_wav_pcm_stream_create(
            cpath, path_size, frames, sample_rate, 2, if format == 1 { 16 } else { 24 }, 0,
        ) },
        _ => unsafe { hirari_wav_wave64_stream_create(cpath, path_size, frames, sample_rate, 2) },
    };
    if stream.is_null() { return std::ptr::null_mut(); }
    Box::into_raw(Box::new(BounceStream {
        stream: match format {
            0 => StreamHandle::Float32(stream),
            1 | 2 => StreamHandle::Pcm(stream),
            _ => StreamHandle::Wave64(stream),
        },
        frames,
    })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_write(
    stream: *mut std::ffi::c_void, channels: *const *const f32, frames: u32,
) -> u8 {
    let Some(stream) = (unsafe { stream.cast::<BounceStream>().as_mut() }) else { return 0 };
    if frames == 0 || u64::from(frames) > stream.frames { return 0; }
    let result = unsafe { stream.stream.write(channels, frames) };
    if result != 0 { stream.frames -= u64::from(frames); }
    result
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_finish(stream: *mut std::ffi::c_void) -> u8 {
    let Some(stream) = (unsafe { stream.cast::<BounceStream>().as_mut() }) else { return 0 };
    if stream.frames != 0 { return 0; }
    unsafe { stream.stream.finish() }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_free(stream: *mut std::ffi::c_void) {
    if !stream.is_null() { drop(unsafe { Box::from_raw(stream.cast::<BounceStream>()) }); }
}

impl StreamHandle {
    unsafe fn write(&mut self, channels: *const *const f32, frames: u32) -> u8 {
        match self {
            Self::Float32(stream) => unsafe { hirari_wav_float32_stream_write(*stream, channels, frames) },
            Self::Pcm(stream) => unsafe { hirari_wav_pcm_stream_write(*stream, channels, frames) },
            Self::Wave64(stream) => unsafe { hirari_wav_wave64_stream_write(*stream, channels, frames) },
        }
    }

    unsafe fn finish(&mut self) -> u8 {
        match self {
            Self::Float32(stream) => unsafe { hirari_wav_float32_stream_finish(*stream) },
            Self::Pcm(stream) => unsafe { hirari_wav_pcm_stream_finish(*stream) },
            Self::Wave64(stream) => unsafe { hirari_wav_wave64_stream_finish(*stream) },
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        unsafe {
            match self {
                Self::Float32(stream) => hirari_wav_float32_stream_free(*stream),
                Self::Pcm(stream) => hirari_wav_pcm_stream_free(*stream),
                Self::Wave64(stream) => hirari_wav_wave64_stream_free(*stream),
            }
        }
    }
}

#[cfg(test)]
#[path = "bounce_stream_tests.rs"]
mod bounce_stream_tests;

