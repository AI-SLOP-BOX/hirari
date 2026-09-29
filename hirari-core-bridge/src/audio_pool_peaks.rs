//! Hierarchical waveform peak generation for the memory-mapped audio pool.

use crate::mapped_audio_file::{audio_file, MappedAudioFile};
use std::ffi::c_void;

const PEAK_STEPS: [u32; 2] = [256, 4096];

struct PeakLevel {
    step: u32,
    min: Vec<f32>,
    max: Vec<f32>,
}

struct PeakHierarchy {
    levels: Vec<PeakLevel>,
}

impl PeakHierarchy {
    fn build(source: &MappedAudioFile) -> Option<Self> {
        let frames = usize::try_from(source.frame_count()).ok()?;
        let mut levels = Vec::with_capacity(PEAK_STEPS.len());
        for step in PEAK_STEPS {
            // Retain the existing public bucket shape: floor(frames / step) +
            // 1, including the trailing empty bucket on exact multiples.
            let bucket_count = frames.checked_div(step as usize)?.checked_add(1)?;
            let mut min = Vec::new();
            let mut max = Vec::new();
            min.try_reserve_exact(bucket_count).ok()?;
            max.try_reserve_exact(bucket_count).ok()?;
            min.resize(bucket_count, f32::INFINITY);
            max.resize(bucket_count, f32::NEG_INFINITY);
            levels.push(PeakLevel { step, min, max });
        }

        // Read each mapped frame once, then update both resolutions. The work
        // stays on the caller's bounded background worker; unlike the former
        // nested C++ pool, this cannot deadlock workers waiting on themselves.
        for frame in 0..frames {
            let mut sample = source.sample(0, frame as u64);
            if !sample.is_finite() {
                sample = 0.0;
            }
            for level in &mut levels {
                let bucket = frame / level.step as usize;
                level.min[bucket] = level.min[bucket].min(sample);
                level.max[bucket] = level.max[bucket].max(sample);
            }
        }

        for level in &mut levels {
            for (minimum, maximum) in level.min.iter_mut().zip(level.max.iter_mut()) {
                if !minimum.is_finite() {
                    *minimum = 0.0;
                    *maximum = 0.0;
                }
            }
        }
        Some(Self { levels })
    }
}

unsafe fn hierarchy<'a>(handle: *const c_void) -> Option<&'a PeakHierarchy> {
    handle.cast::<PeakHierarchy>().as_ref()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_create(mapped_file: *const c_void) -> *mut c_void {
    let Some(source) = audio_file(mapped_file) else {
        return std::ptr::null_mut();
    };
    PeakHierarchy::build(source)
        .map(Box::new)
        .map(Box::into_raw)
        .map_or(std::ptr::null_mut(), |handle| handle.cast())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<PeakHierarchy>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_level_count(handle: *const c_void) -> usize {
    hierarchy(handle).map_or(0, |result| result.levels.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_level_step(
    handle: *const c_void,
    level: usize,
) -> u32 {
    hierarchy(handle)
        .and_then(|result| result.levels.get(level))
        .map_or(0, |level| level.step)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_level_len(
    handle: *const c_void,
    level: usize,
) -> usize {
    hierarchy(handle)
        .and_then(|result| result.levels.get(level))
        .map_or(0, |level| level.min.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_level_min(
    handle: *const c_void,
    level: usize,
) -> *const f32 {
    hierarchy(handle)
        .and_then(|result| result.levels.get(level))
        .map_or(std::ptr::null(), |level| level.min.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pool_peaks_level_max(
    handle: *const c_void,
    level: usize,
) -> *const f32 {
    hierarchy(handle)
        .and_then(|result| result.levels.get(level))
        .map_or(std::ptr::null(), |level| level.max.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn wave_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hirari-pool-peaks-{}-{nonce}.wav",
            std::process::id()
        ))
    }

    fn write_pcm16(path: &std::path::Path, samples: &[i16]) {
        let data_size = (samples.len() * 2) as u32;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&48_000_u32.to_le_bytes()).unwrap();
        file.write_all(&96_000_u32.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    fn write_float32(path: &std::path::Path, samples: &[f32]) {
        let data_size = (samples.len() * 4) as u32;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&3_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&48_000_u32.to_le_bytes()).unwrap();
        file.write_all(&(48_000_u32 * 4).to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap();
        file.write_all(&32_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn builds_the_pool_resolutions_from_the_rust_mapped_reader() {
        let path = wave_path();
        let mut samples = vec![0_i16; 300];
        samples[0] = 16_384;
        samples[255] = -16_384;
        samples[256] = 8_192;
        samples[299] = -8_192;
        write_pcm16(&path, &samples);

        let mapped = MappedAudioFile::open(&path).unwrap();
        let hierarchy = PeakHierarchy::build(&mapped).unwrap();
        assert_eq!(hierarchy.levels.len(), 2);
        assert_eq!(hierarchy.levels[0].step, 256);
        assert_eq!(hierarchy.levels[0].min, [-0.5, -0.25]);
        assert_eq!(hierarchy.levels[0].max, [0.5, 0.25]);
        assert_eq!(hierarchy.levels[1].step, 4096);
        assert_eq!(hierarchy.levels[1].min, [-0.5]);
        assert_eq!(hierarchy.levels[1].max, [0.5]);

        drop(mapped);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_bucket_multiples_keep_a_zeroed_trailing_compatibility_bucket() {
        let path = wave_path();
        write_pcm16(&path, &vec![0; 256]);
        let mapped = MappedAudioFile::open(&path).unwrap();
        let hierarchy = PeakHierarchy::build(&mapped).unwrap();
        assert_eq!(hierarchy.levels[0].min, [0.0, 0.0]);
        assert_eq!(hierarchy.levels[0].max, [0.0, 0.0]);
        drop(mapped);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sanitizes_non_finite_samples_before_building_peak_ranges() {
        let path = wave_path();
        let mut samples = vec![0.0_f32; 257];
        samples[0] = 0.75;
        samples[1] = f32::NAN;
        samples[2] = f32::NEG_INFINITY;
        samples[256] = -0.25;
        write_float32(&path, &samples);
        let mapped = MappedAudioFile::open(&path).unwrap();
        let hierarchy = PeakHierarchy::build(&mapped).unwrap();
        assert_eq!(hierarchy.levels[0].min, [0.0, -0.25]);
        assert_eq!(hierarchy.levels[0].max, [0.75, -0.25]);
        assert_eq!(hierarchy.levels[1].min, [-0.25]);
        assert_eq!(hierarchy.levels[1].max, [0.75]);
        drop(mapped);
        std::fs::remove_file(path).unwrap();
    }
}
