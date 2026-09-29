use memmap2::{Mmap, MmapOptions};
use std::ffi::{c_void, CStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type WaveformSampleCount = unsafe extern "C" fn(*mut c_void, *mut u64) -> bool;
pub type WaveformSample = unsafe extern "C" fn(*mut c_void, u64, *mut f32) -> bool;

#[derive(Clone)]
struct WaveformLod {
    ratio: u32,
    min: Vec<f32>,
    max: Vec<f32>,
}

struct WaveformOverviewData {
    lods: Mutex<Vec<WaveformLod>>,
    error: Mutex<String>,
    ready: Condvar,
    failed: AtomicBool,
    stop: AtomicBool,
}

struct WaveformOverviewState {
    data: Arc<WaveformOverviewData>,
    worker: Option<JoinHandle<()>>,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_create(
    source: *mut c_void,
    sample_count: Option<WaveformSampleCount>,
    sample: Option<WaveformSample>,
) -> *mut c_void {
    let (Some(sample_count), Some(sample)) = (sample_count, sample) else {
        return std::ptr::null_mut();
    };
    let data = Arc::new(WaveformOverviewData {
        lods: Mutex::new(Vec::new()),
        error: Mutex::new(String::new()),
        ready: Condvar::new(),
        failed: AtomicBool::new(false),
        stop: AtomicBool::new(false),
    });
    let worker_data = Arc::clone(&data);
    let source = source as usize;
    let worker = thread::spawn(move || {
        let source = source as *mut c_void;
        let mut total = 0_u64;
        if !unsafe { sample_count(source, &mut total) } {
            if let Ok(mut error) = worker_data.error.lock() {
                *error = "waveform source length query failed".to_owned();
            }
            worker_data.failed.store(true, Ordering::Release);
            worker_data.ready.notify_all();
            return;
        }
        for ratio in [64_u32, 512, 4096] {
            if worker_data.stop.load(Ordering::Acquire) { return; }
            let count = total.saturating_add(ratio as u64 - 1) / ratio as u64;
            if count == 0 || count > u32::MAX as u64 { continue; }
            let mut lod = WaveformLod {
                ratio,
                min: Vec::with_capacity(count as usize),
                max: Vec::with_capacity(count as usize),
            };
            for index in 0..count {
                if worker_data.stop.load(Ordering::Acquire) { return; }
                let start = index * ratio as u64;
                let end = total.min(start.saturating_add(ratio as u64));
                let mut minimum = f32::INFINITY;
                let mut maximum = f32::NEG_INFINITY;
                for frame in start..end {
                    let mut value = 0.0;
                    if !unsafe { sample(source, frame, &mut value) } {
                        if let Ok(mut error) = worker_data.error.lock() {
                            *error = "waveform source sample read failed".to_owned();
                        }
                        worker_data.failed.store(true, Ordering::Release);
                        worker_data.ready.notify_all();
                        return;
                    }
                    if value.is_finite() {
                        minimum = minimum.min(value);
                        maximum = maximum.max(value);
                    }
                }
                if !minimum.is_finite() { minimum = 0.0; }
                if !maximum.is_finite() { maximum = 0.0; }
                lod.min.push(minimum);
                lod.max.push(maximum);
            }
            if worker_data.stop.load(Ordering::Acquire) { return; }
            if let Ok(mut lods) = worker_data.lods.lock() { lods.push(lod); }
            worker_data.ready.notify_all();
        }
    });
    Box::into_raw(Box::new(WaveformOverviewState { data, worker: Some(worker) })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_destroy(state: *mut c_void) {
    if state.is_null() { return; }
    let mut state = unsafe { Box::from_raw(state.cast::<WaveformOverviewState>()) };
    state.data.stop.store(true, Ordering::Release);
    state.data.ready.notify_all();
    if let Some(worker) = state.worker.take() { let _ = worker.join(); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_failed(state: *const c_void) -> bool {
    unsafe { state.cast::<WaveformOverviewState>().as_ref() }
        .map_or(true, |state| state.data.failed.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_wait(state: *const c_void, timeout_ms: u64) -> bool {
    let Some(state) = (unsafe { state.cast::<WaveformOverviewState>().as_ref() }) else { return false; };
    let Ok(lods) = state.data.lods.lock() else { return false; };
    let ready = state.data.ready.wait_timeout_while(lods, Duration::from_millis(timeout_ms), |lods| {
        lods.len() < 3 && !state.data.failed.load(Ordering::Acquire) && !state.data.stop.load(Ordering::Acquire)
    });
    ready.map(|(lods, _)| lods.len() >= 3 && !state.data.failed.load(Ordering::Acquire)).unwrap_or(false)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_lod_count(state: *const c_void) -> u32 {
    let Some(state) = (unsafe { state.cast::<WaveformOverviewState>().as_ref() }) else { return 0; };
    state.data.lods.lock().map(|lods| lods.len() as u32).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_lod_info(
    state: *const c_void, index: u32, ratio: *mut u32, peak_count: *mut usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<WaveformOverviewState>().as_ref() }) else { return false; };
    if ratio.is_null() || peak_count.is_null() { return false; }
    let Ok(lods) = state.data.lods.lock() else { return false; };
    let Some(lod) = lods.get(index as usize) else { return false; };
    unsafe { ratio.write(lod.ratio); peak_count.write(lod.min.len()); }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_copy_lod(
    state: *const c_void, index: u32, min: *mut f32, max: *mut f32, capacity: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<WaveformOverviewState>().as_ref() }) else { return false; };
    let Ok(lods) = state.data.lods.lock() else { return false; };
    let Some(lod) = lods.get(index as usize) else { return false; };
    if lod.min.len() > capacity || (!lod.min.is_empty() && (min.is_null() || max.is_null())) { return false; }
    if lod.min.is_empty() { return true; }
    unsafe {
        std::ptr::copy_nonoverlapping(lod.min.as_ptr(), min, lod.min.len());
        std::ptr::copy_nonoverlapping(lod.max.as_ptr(), max, lod.max.len());
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_overview_copy_error(
    state: *const c_void, destination: *mut u8, capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<WaveformOverviewState>().as_ref() }) else { return 0; };
    let Ok(error) = state.data.error.lock() else { return 0; };
    if capacity == 0 || destination.is_null() { return error.len(); }
    let count = error.len().min(capacity - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(error.as_ptr(), destination, count);
        destination.add(count).write(0);
    }
    error.len()
}

struct MappedWaveformCache {
    file: Option<File>,
    mapping: Option<Mmap>,
    path: PathBuf,
}

impl Drop for MappedWaveformCache {
    fn drop(&mut self) {
        self.mapping.take();
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_cache_map_create(
    path: *const std::ffi::c_char,
    samples: *const f32,
    sample_count: usize,
) -> *mut c_void {
    if path.is_null()
        || samples.is_null()
        || sample_count == 0
        || sample_count % 2 != 0
        || sample_count > (1 << 20)
    {
        return std::ptr::null_mut();
    }
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        return std::ptr::null_mut();
    };
    let path = PathBuf::from(path);
    let result = (|| -> io::Result<MappedWaveformCache> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)?;
        let byte_count = sample_count
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache size overflow"))?;
        let bytes = unsafe {
            std::slice::from_raw_parts(samples.cast::<u8>(), byte_count)
        };
        file.write_all(bytes)?;
        file.sync_all()?;
        let mapping = unsafe { MmapOptions::new().map(&file)? };
        if mapping.len() != byte_count {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "cache mapping length mismatch"));
        }
        Ok(MappedWaveformCache {
            file: Some(file),
            mapping: Some(mapping),
            path: path.clone(),
        })
    })();
    match result {
        Ok(cache) => Box::into_raw(Box::new(cache)).cast(),
        Err(_) => {
            let _ = fs::remove_file(path);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_cache_map_data(
    cache: *const c_void,
) -> *const f32 {
    if cache.is_null() {
        return std::ptr::null();
    }
    unsafe { &*cache.cast::<MappedWaveformCache>() }
        .mapping
        .as_ref()
        .map_or(std::ptr::null(), |mapping| mapping.as_ptr().cast())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_cache_map_len(cache: *const c_void) -> usize {
    if cache.is_null() {
        return 0;
    }
    unsafe { &*cache.cast::<MappedWaveformCache>() }
        .mapping
        .as_ref()
        .map_or(0, |mapping| mapping.len() / std::mem::size_of::<f32>())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_waveform_cache_map_destroy(cache: *mut c_void) {
    if !cache.is_null() {
        drop(unsafe { Box::from_raw(cache.cast::<MappedWaveformCache>()) });
    }
}

pub struct PeakPairRust {
    pub min: f32,
    pub max: f32,
}

pub struct WaveformOrchestrator {
    pub samples_per_pixel: u32,
    pub peaks: Vec<PeakPairRust>,
    pub current_min: f32,
    pub current_max: f32,
    pub sample_counter: u32,
    has_current_sample: bool,
    pub generation: u64,
    pub gate: GenerationGate,
}

/// Build the interleaved max/min envelope used by the region waveform cache.
/// Non-finite source frames are ignored and the legacy zero baseline is kept.
pub fn analysis_waveform_peak_envelope(left: &[f32], right: &[f32], peak_count: u32) -> Vec<f32> {
    let sample_count = left.len().min(right.len());
    if peak_count == 0 || sample_count == 0 {
        return Vec::new();
    }
    let peak_count = peak_count as usize;
    let mut peaks = Vec::with_capacity(peak_count.saturating_mul(2));
    for index in 0..peak_count {
        let begin = ((index as u128 * sample_count as u128) / peak_count as u128) as usize;
        let end = (((index as u128 + 1) * sample_count as u128) / peak_count as u128) as usize;
        let end = end.max(begin.saturating_add(1)).min(sample_count);
        let mut maximum = 0.0f32;
        let mut minimum = 0.0f32;
        for sample in begin..end {
            for value in [left[sample], right[sample]] {
                if value.is_finite() {
                    maximum = maximum.max(value);
                    minimum = minimum.min(value);
                }
            }
        }
        peaks.extend([maximum, minimum]);
    }
    peaks
}

/// Select cached max/min columns at the requested display resolution.
pub fn resample_waveform_peak_envelope(peaks: &[f32], peak_count: u32) -> Vec<f32> {
    let cached_count = peaks.len() / 2;
    if peak_count == 0 || cached_count == 0 || peaks.len() % 2 != 0 {
        return Vec::new();
    }
    let peak_count = peak_count as usize;
    let mut result = Vec::with_capacity(peak_count.saturating_mul(2));
    for index in 0..peak_count {
        let source_index =
            (index as u128 * cached_count as u128 / peak_count as u128) as usize * 2;
        result.extend([peaks[source_index], peaks[source_index + 1]]);
    }
    result
}

impl WaveformOrchestrator {
    pub fn new(samples_per_pixel: u32) -> Self {
        Self {
            samples_per_pixel,
            peaks: Vec::with_capacity(1024),
            current_min: 0.0,
            current_max: 0.0,
            sample_counter: 0,
            has_current_sample: false,
            generation: 0,
            gate: GenerationGate::new(),
        }
    }

    pub fn begin_generation(&mut self) -> u64 {
        self.generation = self.gate.invalidate();
        self.peaks.clear();
        self.sample_counter = 0;
        self.current_min = 0.0;
        self.current_max = 0.0;
        self.has_current_sample = false;
        self.generation
    }

    /// Invalidates a region's cached peaks when its source or lifetime ends.
    /// Returning the new generation lets an in-flight decoder discard stale
    /// work without publishing it after deletion.
    pub fn invalidate(&mut self) -> u64 {
        self.begin_generation()
    }

    pub fn commit_if_current(&mut self, generation: u64, peaks: Vec<PeakPairRust>) -> bool {
        if self.gate.accepts(generation) {
            self.peaks = peaks;
            true
        } else {
            false
        }
    }

    /// INDUSTRIAL: Generates peak data from a raw buffer with SIMD-accelerated precision.
    pub fn generate_for_block(&mut self, data: &[f32]) {
        if self.samples_per_pixel == 0 {
            return;
        }
        for &sample in data {
            if !sample.is_finite() {
                continue;
            }
            if !self.has_current_sample {
                self.current_min = sample;
                self.current_max = sample;
                self.has_current_sample = true;
            } else {
                self.current_min = self.current_min.min(sample);
                self.current_max = self.current_max.max(sample);
            }
            self.sample_counter += 1;

            if self.sample_counter >= self.samples_per_pixel {
                self.peaks.push(PeakPairRust {
                    min: self.current_min,
                    max: self.current_max,
                });
                self.current_min = 0.0;
                self.current_max = 0.0;
                self.sample_counter = 0;
                self.has_current_sample = false;
            }
        }
    }

    /// Flushes a final, partially filled pixel. Callers use this when the
    /// source stream reaches EOF; normal block boundaries remain incremental
    /// and do not manufacture an extra peak.
    pub fn finalize(&mut self) {
        if self.has_current_sample && self.sample_counter > 0 {
            self.peaks.push(PeakPairRust {
                min: self.current_min,
                max: self.current_max,
            });
            self.current_min = 0.0;
            self.current_max = 0.0;
            self.sample_counter = 0;
            self.has_current_sample = false;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project visual assets.
    pub fn audit_waveform_cache(&self) -> bool {
        self.samples_per_pixel > 0
            && self.current_min.is_finite()
            && self.current_max.is_finite()
            && self.current_min <= self.current_max
            && self.peaks.iter().all(|peak| {
                peak.min.is_finite() && peak.max.is_finite() && peak.min <= peak.max
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        analysis_waveform_peak_envelope, resample_waveform_peak_envelope, WaveformOrchestrator,
    };

    #[test]
    fn peak_envelope_uses_stereo_extrema_and_ignores_non_finite_samples() {
        let left = [0.25, 0.75, f32::NAN, -0.5, 0.1];
        let right = [0.5, -0.25, f32::INFINITY, 0.2, -0.1];
        assert_eq!(
            analysis_waveform_peak_envelope(&left, &right, 2),
            vec![0.75, -0.25, 0.2, -0.5]
        );
        assert!(analysis_waveform_peak_envelope(&left, &right, 0).is_empty());
        assert!(analysis_waveform_peak_envelope(&[], &[], 8).is_empty());
    }

    #[test]
    fn peak_envelope_handles_more_pixels_than_frames() {
        let result = analysis_waveform_peak_envelope(&[0.5, -0.25], &[0.1, 0.75], 4);
        assert_eq!(result, vec![0.5, 0.0, 0.5, 0.0, 0.75, -0.25, 0.75, -0.25]);
    }

    #[test]
    fn cached_peak_resampling_preserves_nearest_left_columns() {
        assert_eq!(
            resample_waveform_peak_envelope(
                &[1.0, -1.0, 0.8, -0.8, 0.6, -0.6, 0.4, -0.4],
                2
            ),
            vec![1.0, -1.0, 0.6, -0.6]
        );
        assert!(resample_waveform_peak_envelope(&[1.0], 2).is_empty());
        assert!(resample_waveform_peak_envelope(&[1.0, -1.0], 0).is_empty());
    }

    #[test]
    fn rust_mapped_cache_owns_persistence_and_removes_its_file_on_drop() {
        use std::ffi::CString;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hirari-waveform-map-test-{}-{unique}.bin",
            std::process::id()
        ));
        let path_string = CString::new(path.to_str().unwrap()).unwrap();
        let samples = [0.75f32, -0.5, 0.25, -0.125];
        let cache = unsafe {
            super::hirari_waveform_cache_map_create(
                path_string.as_ptr(),
                samples.as_ptr(),
                samples.len(),
            )
        };
        assert!(!cache.is_null());
        assert!(path.exists());
        let mapped = unsafe { super::hirari_waveform_cache_map_data(cache) };
        let mapped_len = unsafe { super::hirari_waveform_cache_map_len(cache) };
        assert_eq!(mapped_len, samples.len());
        assert_eq!(unsafe { std::slice::from_raw_parts(mapped, mapped_len) }, &samples);
        unsafe { super::hirari_waveform_cache_map_destroy(cache) };
        assert!(!path.exists());
    }

    #[test]
    fn rust_mapped_cache_rejects_invalid_layouts() {
        use std::ffi::CString;

        let path = CString::new("unused-waveform-cache-path").unwrap();
        let samples = [1.0f32, 0.0, 2.0];
        let cache = unsafe {
            super::hirari_waveform_cache_map_create(
                path.as_ptr(),
                samples.as_ptr(),
                samples.len(),
            )
        };
        assert!(cache.is_null());
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn peak_envelope_matches_frozen_cpp_for_boundaries_and_non_finite_audio() {
        unsafe extern "C" {
            fn hirari_waveform_peak_envelope_reference(
                left: *const f32,
                right: *const f32,
                sample_count: usize,
                peak_count: u32,
                output: *mut f32,
                capacity: usize,
            ) -> usize;
        }

        let left = (0..257)
            .map(|index| (index as f32 * 0.17).sin())
            .collect::<Vec<_>>();
        let mut right = (0..257)
            .map(|index| (index as f32 * 0.11).cos() * 0.7)
            .collect::<Vec<_>>();
        right[2] = f32::NAN;
        right[129] = f32::INFINITY;
        for peak_count in [1, 7, 257, 512] {
            let rust = analysis_waveform_peak_envelope(&left, &right, peak_count);
            let mut cpp = vec![0.0; rust.len()];
            let count = unsafe {
                hirari_waveform_peak_envelope_reference(
                    left.as_ptr(),
                    right.as_ptr(),
                    left.len(),
                    peak_count,
                    cpp.as_mut_ptr(),
                    cpp.len(),
                )
            };
            assert_eq!(count, rust.len());
            assert_eq!(rust, cpp);
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn cached_peak_resampling_matches_frozen_cpp_selection() {
        unsafe extern "C" {
            fn hirari_waveform_peak_resample_reference(
                peaks: *const f32,
                cached_count: usize,
                peak_count: u32,
                output: *mut f32,
                capacity: usize,
            ) -> usize;
        }

        let cached = (0..4096)
            .map(|index| (index as f32 * 0.125).sin())
            .collect::<Vec<_>>();
        for peak_count in [1, 37, 2048, 5000] {
            let rust = resample_waveform_peak_envelope(&cached, peak_count);
            let mut cpp = vec![0.0; rust.len()];
            let count = unsafe {
                hirari_waveform_peak_resample_reference(
                    cached.as_ptr(),
                    cached.len() / 2,
                    peak_count,
                    cpp.as_mut_ptr(),
                    cpp.len(),
                )
            };
            assert_eq!(count, rust.len());
            assert_eq!(rust, cpp);
        }
    }

    #[test]
    fn positive_only_waveform_keeps_true_minimum() {
        let mut cache = WaveformOrchestrator::new(3);
        cache.begin_generation();
        cache.generate_for_block(&[0.25, 0.75, 0.5]);
        assert_eq!(cache.peaks.len(), 1);
        assert_eq!(cache.peaks[0].min, 0.25);
        assert_eq!(cache.peaks[0].max, 0.75);
    }

    #[test]
    fn zero_samples_per_pixel_is_safe_and_produces_no_peaks() {
        let mut cache = WaveformOrchestrator::new(0);
        cache.generate_for_block(&[1.0, -1.0]);
        assert!(cache.peaks.is_empty());
    }

    #[test]
    fn stale_waveform_generation_cannot_replace_current_peaks() {
        let mut cache = WaveformOrchestrator::new(1);
        let old = cache.begin_generation();
        let current = cache.begin_generation();
        assert!(!cache.commit_if_current(old, vec![]));
        assert!(cache.commit_if_current(current, vec![]));
    }

    #[test]
    fn invalidation_clears_peaks_and_advances_generation() {
        let mut cache = WaveformOrchestrator::new(1);
        let first = cache.begin_generation();
        cache.generate_for_block(&[0.5]);
        let second = cache.invalidate();
        assert!(second > first);
        assert!(cache.peaks.is_empty());
    }

    #[test]
    fn audit_rejects_corrupt_peak_values() {
        let mut cache = WaveformOrchestrator::new(64);
        cache.peaks.push(super::PeakPairRust {
            min: f32::NAN,
            max: 1.0,
        });
        assert!(!cache.audit_waveform_cache());
    }

    #[test]
    fn finalize_preserves_partial_tail_pixel() {
        let mut cache = WaveformOrchestrator::new(4);
        cache.begin_generation();
        cache.generate_for_block(&[0.2, -0.4, 0.6]);
        assert!(cache.peaks.is_empty());
        cache.finalize();
        assert_eq!(cache.peaks.len(), 1);
        assert_eq!(cache.peaks[0].min, -0.4);
        assert_eq!(cache.peaks[0].max, 0.6);
        assert!(cache.audit_waveform_cache());
    }
}
use crate::generation_gate::GenerationGate;
