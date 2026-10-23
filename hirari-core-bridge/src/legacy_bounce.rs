//! Complete source-compatible PCM16 bounce orchestration and file streaming.

use std::ffi::c_void;
use std::path::Path;

type RenderTrackBlock = unsafe extern "C" fn(*mut c_void, u32, u64, *mut f32, *mut f32, u32) -> bool;

type RenderBlock = unsafe extern "C" fn(*mut c_void, *mut f32, *mut f32, u32) -> bool;
type IsCancelled = unsafe extern "C" fn(*mut c_void) -> bool;
type SetProgress = unsafe extern "C" fn(*mut c_void, f32);
type TrackInfo = unsafe extern "C" fn(*mut c_void, usize, *mut u32, *mut *const u8, *mut usize) -> bool;

#[no_mangle]
pub extern "C" fn hirari_bounce_options_valid(
    path_size: usize, sample_rate: u32, bit_depth: u32, render_tap: u32,
) -> bool {
    path_size != 0 && sample_rate != 0 && sample_rate <= 384_000
        && matches!(bit_depth, 16 | 24 | 32) && render_tap <= 2
}

#[no_mangle]
pub extern "C" fn hirari_bounce_total_samples(
    selection_beats: f64, bpm: f64, sample_rate: u32,
) -> u64 {
    let beats = if selection_beats <= 0.0 { 128.0 } else { selection_beats };
    let samples = beats * (60.0 / bpm) * f64::from(sample_rate);
    if !samples.is_finite() || samples <= 0.0 || samples > u64::MAX as f64 {
        return 0;
    }
    samples as u64
}

struct PcmStream(*mut c_void);

impl Drop for PcmStream {
    fn drop(&mut self) {
        unsafe { crate::export::hirari_wav_pcm_stream_free(self.0) };
    }
}

/// Returns 0 for invalid input or an open failure, 1 after publication, and
/// 2 when rendering, writing, or cancellation stops an opened export.
#[no_mangle]
pub unsafe extern "C" fn hirari_legacy_bounce_render(
    duration_sec: f64,
    sample_rate: f64,
    path: *const u8,
    path_size: usize,
    context: *mut c_void,
    render: Option<RenderBlock>,
    cancelled: Option<IsCancelled>,
    progress: Option<SetProgress>,
) -> u8 {
    if context.is_null() || path.is_null() || path_size == 0 || !duration_sec.is_finite()
        || !sample_rate.is_finite() || duration_sec <= 0.0
        || !(1000.0..=384_000.0).contains(&sample_rate)
    {
        return 0;
    }
    let total = duration_sec * sample_rate;
    if total > f64::from(u32::MAX) * 8.0 {
        return 0;
    }
    let total_samples = total as u64;
    if total_samples == 0 || total_samples > 0x3fff_ffff {
        return 0;
    }
    let (Some(render), Some(cancelled), Some(progress)) = (render, cancelled, progress) else {
        return 0;
    };
    let stream = unsafe {
        crate::export::hirari_wav_pcm_stream_create(
            path.cast(), path_size, total_samples, sample_rate as u32, 2, 16, 0,
        )
    };
    if stream.is_null() {
        return 0;
    }
    let _stream = PcmStream(stream);
    let mut left = vec![0.0_f32; 1024];
    let mut right = vec![0.0_f32; 1024];
    let mut rendered = 0_u64;
    while rendered < total_samples && !unsafe { cancelled(context) } {
        let count = (total_samples - rendered).min(1024) as u32;
        left[..count as usize].fill(0.0);
        right[..count as usize].fill(0.0);
        if !unsafe { render(context, left.as_mut_ptr(), right.as_mut_ptr(), count) } {
            break;
        }
        let channels = [left.as_ptr(), right.as_ptr()];
        if unsafe {
            crate::export::hirari_wav_pcm_stream_write(
                stream, channels.as_ptr(), count,
            )
        } == 0
        {
            break;
        }
        rendered += u64::from(count);
        unsafe { progress(context, rendered as f32 / total_samples as f32) };
    }
    if rendered == total_samples && !unsafe { cancelled(context) }
        && unsafe { crate::export::hirari_wav_pcm_stream_finish(stream) } != 0
    {
        1
    } else {
        2
    }
}

/// Rust-owned two-pass offline bounce. The callback is the only native seam:
/// it asks the engine to render and sum the selected track set for one block.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_stream(
    path: *const u8,
    path_size: usize,
    sample_rate: u32,
    bit_depth: u32,
    normalize: bool,
    dither: bool,
    total_samples: u64,
    track_ids: *const u32,
    track_count: usize,
    context: *mut c_void,
    render: Option<RenderTrackBlock>,
    cancelled: Option<IsCancelled>,
    progress: Option<SetProgress>,
    peak_out: *mut f32,
) -> bool {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 || context.is_null()
        || peak_out.is_null() || (track_ids.is_null() && track_count != 0)
        || total_samples == 0 || total_samples > usize::MAX as u64
        || !(8_000..=384_000).contains(&sample_rate)
        || !matches!(bit_depth, 16 | 24 | 32)
    {
        return false;
    }
    let (Some(render), Some(cancelled), Some(progress)) = (render, cancelled, progress) else {
        return false;
    };
    let format = match bit_depth { 16 => 1, 24 => 2, _ => 0 };
    let stream = unsafe {
        crate::export::hirari_bounce_stream_create(
            path, path_size, total_samples, sample_rate, format,
        )
    };
    if stream.is_null() { return false; }
    struct Stream(*mut c_void);
    impl Drop for Stream {
        fn drop(&mut self) { unsafe { crate::export::hirari_bounce_stream_free(self.0) } }
    }
    let stream = Stream(stream);
    struct PeakOutput(*mut f32, f32);
    impl Drop for PeakOutput {
        fn drop(&mut self) { unsafe { *self.0 = self.1; } }
    }
    let mut peak_output = PeakOutput(peak_out, 0.0);
    let mut left = vec![0.0_f32; 1024];
    let mut right = vec![0.0_f32; 1024];
    let mut track_left = vec![0.0_f32; 1024];
    let mut track_right = vec![0.0_f32; 1024];
    let track_ids = if track_count == 0 { &[][..] } else {
        unsafe { std::slice::from_raw_parts(track_ids, track_count) }
    };
    let mut peak = 0.0_f32;
    let mut offset = 0_u64;
    if normalize {
        while offset < total_samples {
            if unsafe { cancelled(context) } { return false; }
            let count = (total_samples - offset).min(1024) as u32;
            left[..count as usize].fill(0.0);
            right[..count as usize].fill(0.0);
            for &track_id in track_ids {
                track_left[..count as usize].fill(0.0);
                track_right[..count as usize].fill(0.0);
                if !unsafe { render(context, track_id, offset, track_left.as_mut_ptr(), track_right.as_mut_ptr(), count) } {
                    return false;
                }
                for index in 0..count as usize {
                    left[index] += if track_left[index].is_finite() { track_left[index] } else { 0.0 };
                    right[index] += if track_right[index].is_finite() { track_right[index] } else { 0.0 };
                }
            }
            for index in 0..count as usize {
                peak = peak.max(left[index].abs()).max(right[index].abs());
            }
            peak_output.1 = peak;
            offset += u64::from(count);
        }
    }
    let gain = if normalize && peak > 0.0 { peak.recip() } else { 1.0 };
    let dither_left = crate::tpdf_dither::hirari_tpdf_dither_create_with_seed(0x1234_5678);
    let dither_right = crate::tpdf_dither::hirari_tpdf_dither_create_with_seed(0x9E37_79B9);
    if dither_left.is_null() || dither_right.is_null() { return false; }
    struct Dither(*mut c_void, *mut c_void);
    impl Drop for Dither {
        fn drop(&mut self) {
            unsafe {
                crate::tpdf_dither::hirari_tpdf_dither_destroy(self.0);
                crate::tpdf_dither::hirari_tpdf_dither_destroy(self.1);
            }
        }
    }
    let _dither = Dither(dither_left, dither_right);
    offset = 0;
    while offset < total_samples {
        if unsafe { cancelled(context) } { return false; }
        let count = (total_samples - offset).min(1024) as u32;
        left[..count as usize].fill(0.0);
        right[..count as usize].fill(0.0);
        for &track_id in track_ids {
            track_left[..count as usize].fill(0.0);
            track_right[..count as usize].fill(0.0);
            if !unsafe { render(context, track_id, offset, track_left.as_mut_ptr(), track_right.as_mut_ptr(), count) } {
                return false;
            }
            for index in 0..count as usize {
                left[index] += if track_left[index].is_finite() { track_left[index] } else { 0.0 };
                right[index] += if track_right[index].is_finite() { track_right[index] } else { 0.0 };
            }
        }
        for index in 0..count as usize {
            let mut l = left[index] * gain;
            let mut r = right[index] * gain;
            if !normalize { peak = peak.max(l.abs()).max(r.abs()); }
            peak_output.1 = peak;
            if dither && bit_depth < 32 {
                l += unsafe { crate::tpdf_dither::hirari_tpdf_dither_next(dither_left) };
                r += unsafe { crate::tpdf_dither::hirari_tpdf_dither_next(dither_right) };
            }
            left[index] = l;
            right[index] = r;
        }
        let channels = [left.as_ptr(), right.as_ptr()];
        if unsafe { crate::export::hirari_bounce_stream_write(
            stream.0, channels.as_ptr(), count,
        ) } == 0 { return false; }
        offset += u64::from(count);
        unsafe { progress(context, offset as f32 / total_samples as f32) };
    }
    unsafe { crate::export::hirari_bounce_stream_finish(stream.0) != 0 }
}

/// Owns stem selection, deterministic filename construction, progress across
/// stems, and per-track stream orchestration. Native callbacks only expose the
/// engine's track identities and render one selected track block.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_stems(
    master_path: *const u8,
    path_size: usize,
    sample_rate: u32,
    bit_depth: u32,
    normalize: bool,
    dither: bool,
    render_tap: u32,
    selection_beats: f64,
    bpm: f64,
    track_count: usize,
    context: *mut c_void,
    track_info: Option<TrackInfo>,
    render: Option<RenderTrackBlock>,
    cancelled: Option<IsCancelled>,
    progress: Option<SetProgress>,
    peak_out: *mut f32,
) -> bool {
    if master_path.is_null() || path_size == 0 || context.is_null() || peak_out.is_null()
        || track_count == 0 || track_count > 1_000_000
        || !hirari_bounce_options_valid(path_size, sample_rate, bit_depth, render_tap)
    { return false; }
    let (Some(track_info), Some(render), Some(cancelled), Some(progress)) =
        (track_info, render, cancelled, progress) else { return false };
    let source_bytes = unsafe { std::slice::from_raw_parts(master_path, path_size) };
    #[cfg(unix)]
    let source = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(source_bytes))
    };
    #[cfg(not(unix))]
    let source = {
        let Ok(path) = std::str::from_utf8(source_bytes) else { return false };
        Path::new(path)
    };
    let extension = source.extension().filter(|value| !value.is_empty());
    let suffix = extension.map_or_else(|| b".wav".to_vec(), |value| {
        let mut bytes = vec![b'.'];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            bytes.extend_from_slice(value.as_bytes());
        }
        #[cfg(not(unix))]
        if let Some(value) = value.to_str() { bytes.extend_from_slice(value.as_bytes()); }
        bytes
    });
    let mut base = source.to_path_buf();
    if extension.is_some() { base.set_extension(""); }
    let base = base.into_os_string();
    let mut rendered = 0_usize;
    let mut peak = 0.0_f32;
    for index in 0..track_count {
        if unsafe { cancelled(context) } { unsafe { *peak_out = peak }; return false; }
        let mut track_id = 0_u32;
        let mut name_ptr = std::ptr::null();
        let mut name_size = 0_usize;
        if !unsafe { track_info(context, index, &mut track_id, &mut name_ptr, &mut name_size) }
            || name_ptr.is_null() || name_size > 1024 * 1024
        { continue; }
        let name = unsafe { std::slice::from_raw_parts(name_ptr, name_size) };
        let mut safe_name: Vec<u8> = name.iter().map(|byte| {
            if byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_' { *byte } else { b'_' }
        }).collect();
        if safe_name.is_empty() { safe_name.extend_from_slice(b"track"); }
        let mut output = base.clone();
        output.push(format!(".stem-{track_id}-"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            output.push(std::ffi::OsStr::from_bytes(&safe_name));
            output.push(std::ffi::OsStr::from_bytes(&suffix));
        }
        #[cfg(not(unix))]
        {
            output.push(String::from_utf8_lossy(&safe_name).as_ref());
            output.push(String::from_utf8_lossy(&suffix).as_ref());
        }
        #[cfg(unix)]
        let output_bytes = {
            use std::os::unix::ffi::OsStrExt;
            output.as_os_str().as_bytes()
        };
        #[cfg(not(unix))]
        let output_bytes = {
            let Ok(value) = output.to_str() else { continue };
            value.as_bytes()
        };
        let ids = [track_id];
        let total_samples = hirari_bounce_total_samples(selection_beats, bpm, sample_rate);
        peak = 0.0;
        let _ = unsafe { hirari_bounce_render_stream(
            output_bytes.as_ptr(), output_bytes.len(), sample_rate, bit_depth,
            normalize, dither, total_samples, ids.as_ptr(), ids.len(), context,
            Some(render), Some(cancelled), Some(progress), &mut peak,
        ) };
        rendered += 1;
        unsafe { progress(context, rendered as f32 / track_count as f32) };
    }
    unsafe { *peak_out = peak };
    if rendered > 0 { unsafe { progress(context, 1.0) }; }
    rendered > 0
}
