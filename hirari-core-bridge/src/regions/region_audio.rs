//! Region source-audio access and the waveform peak cache.
//!
//! The native engine owns decoding and region storage; this module owns the
//! validation, the interleaving policy, and the peak cache. Nothing here runs
//! on the audio callback: cache probes only touch the Rust registry, and cache
//! builds run on detached control-plane threads.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::thread;

use crate::region_edit_host::{host_ops, HirariRegionHostOps};
use crate::waveform_cache::{analysis_waveform_peak_envelope, resample_waveform_peak_envelope};

/// Matches the native `get_region_audio_interleaved` guard.
#[cfg(test)]
const MAX_INTERLEAVED_FRAMES: u32 = 16 * 1024 * 1024;
/// Matches the native ARA2 range-read guard.
const MAX_RANGE_SAMPLES: u32 = 1 << 20;
const MAX_CHANNELS: u32 = 32;
const MAX_PATH_BYTES: usize = 1 << 20;
/// The cache stores one fixed-resolution envelope per region.
const CACHED_PEAK_COUNT: u32 = 2048;

/// Owned `Vec<f32>` returned to the native caller, which copies it into a
/// `rust::Vec` and destroys the handle.
struct FloatBuffer(Vec<f32>);

fn float_buffer(values: Vec<f32>) -> *mut c_void {
    Box::into_raw(Box::new(FloatBuffer(values))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_float_buffer_len(buffer: *const c_void) -> usize {
    unsafe { buffer.cast::<FloatBuffer>().as_ref() }.map_or(0, |buffer| buffer.0.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_float_buffer_data(buffer: *const c_void) -> *const f32 {
    unsafe { buffer.cast::<FloatBuffer>().as_ref() }
        .map_or(ptr::null(), |buffer| buffer.0.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_float_buffer_destroy(buffer: *mut c_void) {
    if !buffer.is_null() {
        drop(unsafe { Box::from_raw(buffer.cast::<FloatBuffer>()) });
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// A native audio buffer retained for the duration of one call or one cache
/// build. `audio_view` hands out channel pointers that stay valid until the
/// handle is released.
struct RetainedAudio {
    ops: HirariRegionHostOps,
    handle: *mut c_void,
}

impl RetainedAudio {
    fn summary(&self) -> Option<(u32, u64)> {
        let view = self.ops.audio_view?;
        let mut samples = 0u64;
        let channels = unsafe { view(self.handle, ptr::null_mut(), 0, &mut samples) };
        if channels == 0 || channels > MAX_CHANNELS {
            return None;
        }
        Some((channels, samples))
    }

    /// Returns the channel count, the frame count, and one pointer per channel.
    fn view(&self) -> Option<(u32, u64, Vec<*const f32>)> {
        let (channels, samples) = self.summary()?;
        if samples == 0 {
            return None;
        }
        let view = self.ops.audio_view?;
        let mut pointers = vec![ptr::null(); channels as usize];
        let mut written_samples = 0u64;
        if unsafe { view(self.handle, pointers.as_mut_ptr(), pointers.len(), &mut written_samples) }
            != channels
        {
            return None;
        }
        if pointers.iter().any(|pointer| pointer.is_null()) {
            return None;
        }
        Some((channels, samples, pointers))
    }
}

impl Drop for RetainedAudio {
    fn drop(&mut self) {
        if let Some(release) = self.ops.release_audio {
            unsafe { release(self.handle) };
        }
    }
}

// The handle is only read through immutable channel pointers after the native
// decoder returns it, and releasing it only drops a shared reference.
unsafe impl Send for RetainedAudio {}

fn region_path(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<Vec<u8>> {
    let path = ops.region_path?;
    let length = unsafe { path(host, track_id, region_id, ptr::null_mut(), 0) };
    if length == 0 || length > MAX_PATH_BYTES {
        return None;
    }
    let mut bytes = vec![0u8; length];
    if unsafe { path(host, track_id, region_id, bytes.as_mut_ptr(), bytes.len()) } != length {
        return None;
    }
    Some(bytes)
}

fn decode(ops: &HirariRegionHostOps, path: &[u8]) -> Option<(RetainedAudio, f64)> {
    // The import cache owns decoding and the planar build; this only wraps the
    // owned handle so the region can read it through the host callbacks.
    let (handle, sample_rate, _channels, _frames) =
        crate::audio_import_cache::import_audio_file(path)?;
    Some((
        RetainedAudio {
            ops: *ops,
            handle,
        },
        sample_rate,
    ))
}

fn opened_region(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> Option<(RetainedAudio, f64)> {
    let path = region_path(ops, host, track_id, region_id)?;
    decode(ops, &path)
}

fn channel_slices(pointers: &[*const f32], samples: u64) -> Vec<&[f32]> {
    pointers
        .iter()
        .map(|pointer| unsafe { std::slice::from_raw_parts(*pointer, samples as usize) })
        .collect()
}

#[cfg(test)]
fn interleaved_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    max_frames: u32,
) -> Vec<f32> {
    if host.is_null() || max_frames == 0 || max_frames > MAX_INTERLEAVED_FRAMES {
        return Vec::new();
    }
    let Some((audio, _)) = opened_region(ops, host, track_id, region_id) else {
        return Vec::new();
    };
    let Some((channels, samples, pointers)) = audio.view() else {
        return Vec::new();
    };
    if channels != 2 || samples > u64::from(max_frames) {
        return Vec::new();
    }
    let frames = samples as usize;
    let left = unsafe { std::slice::from_raw_parts(pointers[0], frames) };
    let right = unsafe { std::slice::from_raw_parts(pointers[1], frames) };
    let mut out = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        out.push(finite(left[frame]));
        out.push(finite(right[frame]));
    }
    out
}

fn range_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_frame: u64,
    frames: u32,
    channels: u32,
) -> Vec<f32> {
    if track_id == 0
        || region_id == 0
        || frames == 0
        || channels == 0
        || channels > MAX_CHANNELS
        || frames > MAX_RANGE_SAMPLES / channels
    {
        return Vec::new();
    }
    let Some((audio, _)) = opened_region(ops, host, track_id, region_id) else {
        return Vec::new();
    };
    let Some((available_channels, samples, pointers)) = audio.view() else {
        return Vec::new();
    };
    if channels > available_channels || start_frame >= samples {
        return Vec::new();
    }
    let read_frames = u64::from(frames).min(samples - start_frame) as usize;
    let start = start_frame as usize;
    let sources = channel_slices(&pointers[..channels as usize], samples);
    let mut out = Vec::with_capacity(read_frames * channels as usize);
    for frame in 0..read_frames {
        for source in &sources {
            out.push(finite(source[start + frame]));
        }
    }
    out
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_audio_interleaved_range(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start_frame: u64,
    frames: u32,
    channels: u32,
) -> *mut c_void {
    let Some(ops) = host_ops() else {
        return float_buffer(Vec::new());
    };
    float_buffer(range_with(
        &ops,
        host,
        track_id,
        region_id,
        start_frame,
        frames,
        channels,
    ))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_source_frame_count(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> u64 {
    if host.is_null() || track_id == 0 || region_id == 0 {
        return 0;
    }
    let Some(ops) = host_ops() else { return 0 };
    opened_region(&ops, host, track_id, region_id)
        .and_then(|(audio, _)| audio.summary().map(|(_, samples)| samples))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_source_sample_rate(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> f64 {
    if host.is_null() || track_id == 0 || region_id == 0 {
        return 0.0;
    }
    let Some(ops) = host_ops() else { return 0.0 };
    let Some((_, sample_rate)) = opened_region(&ops, host, track_id, region_id) else {
        return 0.0;
    };
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        0.0
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_audio_channel_count(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> u32 {
    if host.is_null() {
        return 0;
    }
    let Some(ops) = host_ops() else { return 0 };
    opened_region(&ops, host, track_id, region_id)
        .and_then(|(audio, _)| audio.summary().map(|(channels, _)| channels))
        .unwrap_or(0)
}

#[derive(Default)]
struct PeakEntry {
    peaks: Option<Vec<f32>>,
    /// Bumped by invalidation so an in-flight build cannot republish.
    generation: u64,
    building: bool,
}

static PEAK_CACHE: OnceLock<Mutex<HashMap<(usize, u32), PeakEntry>>> = OnceLock::new();

fn peak_cache() -> &'static Mutex<HashMap<(usize, u32), PeakEntry>> {
    PEAK_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_resampled(key: (usize, u32), num_peaks: u32) -> Option<Vec<f32>> {
    let cache = peak_cache().lock().ok()?;
    let peaks = cache.get(&key)?.peaks.as_ref()?;
    Some(resample_waveform_peak_envelope(peaks, num_peaks))
}

/// Registers a build for the current entry generation. Returns the generation
/// a worker must publish under, or `None` when a build is already running.
fn register_build(key: (usize, u32)) -> Option<u64> {
    let mut cache = peak_cache().lock().ok()?;
    let entry = cache.entry(key).or_default();
    if entry.building {
        return None;
    }
    entry.building = true;
    Some(entry.generation)
}

fn publish_build(key: (usize, u32), generation: u64, peaks: Option<Vec<f32>>) {
    let Ok(mut cache) = peak_cache().lock() else {
        return;
    };
    let Some(entry) = cache.get_mut(&key) else {
        return;
    };
    entry.building = false;
    if entry.generation == generation {
        entry.peaks = peaks;
    }
    if entry.peaks.is_none() {
        cache.remove(&key);
    }
}

fn build_peaks(audio: &RetainedAudio) -> Option<Vec<f32>> {
    let (channels, samples, pointers) = audio.view()?;
    let slices = channel_slices(&pointers, samples);
    let right = if channels > 1 { slices[1] } else { slices[0] };
    let peaks = analysis_waveform_peak_envelope(slices[0], right, CACHED_PEAK_COUNT);
    (peaks.len() == CACHED_PEAK_COUNT as usize * 2).then_some(peaks)
}

fn spawn_build(key: (usize, u32), generation: u64, audio: RetainedAudio) {
    let spawned = thread::Builder::new()
        .name("hirari-region-waveform".to_owned())
        .spawn(move || {
            let peaks = build_peaks(&audio);
            publish_build(key, generation, peaks);
        });
    if spawned.is_err() {
        publish_build(key, generation, None);
    }
}

fn waveform_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    num_peaks: u32,
) -> Vec<f32> {
    if host.is_null() || region_id == 0 || num_peaks == 0 {
        return Vec::new();
    }
    let key = (host as usize, region_id);
    if let Some(peaks) = cached_resampled(key, num_peaks) {
        return peaks;
    }
    let Some((audio, _)) = opened_region(ops, host, track_id, region_id) else {
        return Vec::new();
    };
    let Some((channels, samples, pointers)) = audio.view() else {
        return Vec::new();
    };
    let slices = channel_slices(&pointers, samples);
    let right = if channels > 1 { slices[1] } else { slices[0] };
    let peaks = analysis_waveform_peak_envelope(slices[0], right, num_peaks);
    if let Some(generation) = register_build(key) {
        spawn_build(key, generation, audio);
    }
    peaks
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_waveform(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    num_peaks: u32,
) -> *mut c_void {
    let Some(ops) = host_ops() else {
        return float_buffer(Vec::new());
    };
    float_buffer(waveform_with(&ops, host, track_id, region_id, num_peaks))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_waveform_forget(host: *mut c_void, region_id: u32) {
    if host.is_null() || region_id == 0 {
        return;
    }
    let Ok(mut cache) = peak_cache().lock() else {
        return;
    };
    let key = (host as usize, region_id);
    let Some(entry) = cache.get_mut(&key) else {
        return;
    };
    entry.generation = entry.generation.wrapping_add(1);
    entry.peaks = None;
    if !entry.building {
        cache.remove(&key);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_waveform_clear(host: *mut c_void) {
    if host.is_null() {
        return;
    }
    let Ok(mut cache) = peak_cache().lock() else {
        return;
    };
    let owner = host as usize;
    cache.retain(|(entry_owner, _), _| *entry_owner != owner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct FakeHost {
        audio: Vec<Vec<f32>>,
        sample_rate: f64,
        decodes: AtomicUsize,
    }

    unsafe extern "C" fn fake_region_path(
        _host: *mut c_void,
        _track_id: u32,
        _region_id: u32,
        out: *mut u8,
        capacity: usize,
    ) -> usize {
        let path = b"fake.wav";
        if !out.is_null() && capacity >= path.len() {
            unsafe { ptr::copy_nonoverlapping(path.as_ptr(), out, path.len()) };
        }
        path.len()
    }

    unsafe extern "C" fn fake_view(
        handle: *mut c_void,
        out: *mut *const f32,
        capacity: usize,
        samples: *mut u64,
    ) -> u32 {
        let channels = unsafe { &*handle.cast::<Vec<Vec<f32>>>() };
        if let Some(samples) = unsafe { samples.as_mut() } {
            *samples = channels.first().map_or(0, |channel| channel.len() as u64);
        }
        if !out.is_null() && capacity >= channels.len() {
            for (index, channel) in channels.iter().enumerate() {
                unsafe { out.add(index).write(channel.as_ptr()) };
            }
        }
        channels.len() as u32
    }

    unsafe extern "C" fn fake_release(handle: *mut c_void) {
        drop(unsafe { Box::from_raw(handle.cast::<Vec<Vec<f32>>>()) });
    }

    // The import callbacks are keyed by media path, not by host, so the
    // fixture publishes the host the current thread should read.
    thread_local! {
        static CURRENT_HOST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn fake_import_acquire(
        _path: *const u8,
        _length: usize,
        rate: *mut f64,
        handle: *mut *mut c_void,
    ) -> i32 {
        let host = CURRENT_HOST.with(|entry| entry.get());
        let host = unsafe { &*(host as *const FakeHost) };
        host.decodes.fetch_add(1, Ordering::AcqRel);
        let channels = host.audio.clone();
        unsafe {
            *rate = host.sample_rate;
            *handle = Box::into_raw(Box::new(channels)).cast();
        }
        crate::audio_import_cache::HIRARI_IMPORT_CACHE_HIT
    }

    fn fake_import_decode(
        _path: *const u8,
        _length: usize,
    ) -> crate::preview_audio_runtime::NativeDecodedAudio {
        crate::preview_audio_runtime::NativeDecodedAudio {
            samples: ptr::null_mut(),
            sample_count: 0,
            channels: 0,
            sample_rate: 0.0,
            bits_per_sample: 0,
            status: 3,
        }
    }

    /// Every import the region audio path makes is served as a cache hit from
    /// the host fixture; the planar data is still owned by Rust.
    fn fake_import() -> crate::audio_import_cache::ImportOps {
        crate::audio_import_cache::ImportOps {
            acquire: fake_import_acquire,
            store: |_, _, _, _| true,
            create: |_| ptr::null_mut(),
            storage: |handle| handle,
            channels: |handle| unsafe { &*handle.cast::<Vec<Vec<f32>>>() }.len() as u32,
            frames: |handle| {
                unsafe { &*handle.cast::<Vec<Vec<f32>>>() }
                    .first()
                    .map_or(0, |channel| channel.len() as u64)
            },
            weak: |_| ptr::null_mut(),
            release: |_| {},
            decode: fake_import_decode,
        }
    }

    fn fake_ops() -> HirariRegionHostOps {
        HirariRegionHostOps {
            region_path: Some(fake_region_path),
            audio_view: Some(fake_view),
            release_audio: Some(fake_release),
            ..Default::default()
        }
    }

    // The peak cache is keyed by host address; leak the fixture so a later
    // test cannot allocate a host at the same address and inherit its entries.
    fn fake_host(audio: Vec<Vec<f32>>, sample_rate: f64) -> *mut c_void {
        let host = Box::into_raw(Box::new(FakeHost {
            audio,
            sample_rate,
            decodes: AtomicUsize::new(0),
        }))
        .cast();
        CURRENT_HOST.with(|entry| entry.set(host as usize));
        crate::audio_import_cache::set_import_ops(Some(fake_import()));
        host
    }

    fn decode_count(host: *mut c_void) -> usize {
        unsafe { &*host.cast::<FakeHost>() }
            .decodes
            .load(Ordering::Acquire)
    }

    fn cached_peaks(host: *mut c_void, region_id: u32) -> Option<Vec<f32>> {
        peak_cache()
            .lock()
            .ok()?
            .get(&(host as usize, region_id))?
            .peaks
            .clone()
    }

    fn wait_for_cache(host: *mut c_void, region_id: u32) -> Vec<f32> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(peaks) = cached_peaks(host, region_id) {
                return peaks;
            }
            assert!(Instant::now() < deadline, "waveform build did not finish");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn interleaved_requires_stereo_and_sanitizes_non_finite() {
        let ops = fake_ops();
        let mono = fake_host(vec![vec![0.5, 0.25]], 48_000.0);
        assert!(interleaved_with(&ops, mono, 1, 1, 16).is_empty());

        let stereo = fake_host(
            vec![vec![0.5, f32::NAN, -0.25], vec![f32::INFINITY, 0.75, -1.0]],
            44_100.0,
        );
        assert_eq!(
            interleaved_with(&ops, stereo, 1, 1, 16),
            vec![0.5, 0.0, 0.0, 0.75, -0.25, -1.0]
        );
        assert!(interleaved_with(&ops, stereo, 1, 1, 2).is_empty());
    }

    #[test]
    fn range_reads_requested_channels_and_clamps_to_end() {
        let ops = fake_ops();
        let host = fake_host(
            vec![vec![0.0, 1.0, 2.0, 3.0], vec![10.0, 11.0, 12.0, 13.0]],
            48_000.0,
        );
        assert_eq!(
            range_with(&ops, host, 1, 1, 1, 10, 2),
            vec![1.0, 11.0, 2.0, 12.0, 3.0, 13.0]
        );
        assert_eq!(range_with(&ops, host, 1, 1, 1, 1, 1), vec![1.0]);
        assert!(range_with(&ops, host, 1, 1, 4, 1, 1).is_empty());
        assert!(range_with(&ops, host, 1, 1, 0, 1, 3).is_empty());
        assert!(range_with(&ops, host, 0, 1, 0, 1, 1).is_empty());
    }

    #[test]
    fn waveform_builds_cached_peaks_and_reuses_them() {
        let ops = fake_ops();
        let host = fake_host(
            vec![
                (0..4096).map(|index| (index as f32 * 0.01).sin()).collect(),
                (0..4096).map(|index| (index as f32 * 0.02).cos()).collect(),
            ],
            48_000.0,
        );
        let first = waveform_with(&ops, host, 1, 7, 64);
        assert_eq!(first.len(), 128);
        let cached = wait_for_cache(host, 7);
        assert_eq!(cached.len(), CACHED_PEAK_COUNT as usize * 2);

        let decodes_before = decode_count(host);
        let second = waveform_with(&ops, host, 1, 7, 32);
        assert_eq!(second.len(), 64);
        assert_eq!(decode_count(host), decodes_before);
        assert_eq!(second, resample_waveform_peak_envelope(&cached, 32));
        unsafe { hirari_region_waveform_clear(host) };
    }

    #[test]
    fn forget_invalidates_cache_and_drops_stale_builds() {
        let ops = fake_ops();
        let host = fake_host(vec![vec![0.5; 1024], vec![-0.5; 1024]], 48_000.0);
        let key = (host as usize, 3);
        peak_cache().lock().unwrap().insert(
            key,
            PeakEntry {
                peaks: Some(vec![0.25; 8]),
                generation: 0,
                building: false,
            },
        );
        assert_eq!(cached_resampled(key, 4), Some(vec![0.25; 8]));

        let generation = register_build(key).unwrap();
        unsafe { hirari_region_waveform_forget(host, 3) };
        assert!(cached_resampled(key, 4).is_none());
        publish_build(key, generation, Some(vec![1.0; 8]));
        assert!(peak_cache().lock().unwrap().get(&key).is_none());

        let rebuilt = waveform_with(&ops, host, 1, 3, 8);
        assert_eq!(rebuilt.len(), 16);
        unsafe { hirari_region_waveform_clear(host) };
    }

    #[test]
    fn accessors_report_source_metadata() {
        let ops = fake_ops();
        let host = fake_host(vec![vec![0.0; 256], vec![0.0; 256]], 44_100.0);
        let (_, sample_rate) = opened_region(&ops, host, 1, 1).unwrap();
        assert_eq!(sample_rate, 44_100.0);
        assert_eq!(
            opened_region(&ops, host, 1, 1)
                .and_then(|(audio, _)| audio.summary())
                .unwrap(),
            (2, 256)
        );
        unsafe { hirari_region_waveform_clear(host) };
    }
}
