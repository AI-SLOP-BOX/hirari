use crate::signalsmith_fft::SignalsmithFft;
use rustfft::num_complex::Complex32;

const FFT_SIZE: usize = 512;
const BIN_COUNT: usize = FFT_SIZE / 2;
const CLASH_THRESHOLD: f32 = 0.02;

/// Computes the dominant spectral overlaps for mono track snapshots.
/// `other_mono` is a packed array of `other_count` frames, each `frame_count`
/// samples long. The caller owns track-buffer access; this Rust kernel owns
/// the transform, magnitude, and clash-selection work.
#[no_mangle]
pub unsafe extern "C" fn hirari_masking_analysis(
    target_mono: *const f32,
    other_mono: *const f32,
    other_track_ids: *const u32,
    other_count: usize,
    frame_count: usize,
    output_bins: *mut u32,
    output_intensities: *mut f32,
    output_track_ids: *mut u32,
    output_capacity: usize,
) -> usize {
    if target_mono.is_null()
        || frame_count != FFT_SIZE
        || output_bins.is_null()
        || output_intensities.is_null()
        || output_track_ids.is_null()
        || output_capacity == 0
        || (other_count > 0 && (other_mono.is_null() || other_track_ids.is_null()))
        || other_count.checked_mul(frame_count).is_none()
    {
        return 0;
    }

    let Some(mut plan) = SignalsmithFft::new(FFT_SIZE) else {
        return 0;
    };
    let target = std::slice::from_raw_parts(target_mono, FFT_SIZE);
    let others = if other_count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(other_mono, other_count * FFT_SIZE)
    };
    let track_ids = if other_count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(other_track_ids, other_count)
    };
    let bins_out = std::slice::from_raw_parts_mut(output_bins, output_capacity);
    let intensities_out = std::slice::from_raw_parts_mut(output_intensities, output_capacity);
    let track_ids_out = std::slice::from_raw_parts_mut(output_track_ids, output_capacity);

    let mut target_spectrum = target
        .iter()
        .map(|&sample| Complex32::new(sample, 0.0))
        .collect::<Vec<_>>();
    if !plan.forward(&mut target_spectrum) {
        return 0;
    }
    let target_magnitudes = (0..BIN_COUNT)
        .map(|bin| target_spectrum[bin].norm())
        .collect::<Vec<_>>();

    let mut spectrum = vec![Complex32::new(0.0, 0.0); FFT_SIZE];
    let mut written = 0;
    'tracks: for track_index in 0..other_count {
        let start = track_index * FFT_SIZE;
        for (value, &sample) in spectrum.iter_mut().zip(&others[start..start + FFT_SIZE]) {
            *value = Complex32::new(sample, 0.0);
        }
        if !plan.forward(&mut spectrum) {
            return 0;
        }

        for bin in 0..BIN_COUNT {
            let intensity = target_magnitudes[bin] * spectrum[bin].norm();
            if intensity > CLASH_THRESHOLD {
                bins_out[written] = bin as u32;
                intensities_out[written] = intensity;
                track_ids_out[written] = track_ids[track_index];
                written += 1;
                if written == output_capacity {
                    break 'tracks;
                }
            }
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tracks_produces_no_clashes() {
        let target = [0.0f32; FFT_SIZE];
        let mut bins = [0u32; 4];
        let mut intensities = [0.0f32; 4];
        let mut ids = [0u32; 4];
        let written = unsafe {
            hirari_masking_analysis(
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                FFT_SIZE,
                bins.as_mut_ptr(),
                intensities.as_mut_ptr(),
                ids.as_mut_ptr(),
                bins.len(),
            )
        };
        assert_eq!(written, 0);
    }

    #[test]
    fn matching_sines_report_the_shared_bin_and_track() {
        let target = (0..FFT_SIZE)
            .map(|index| (2.0 * std::f32::consts::PI * 23.0 * index as f32 / FFT_SIZE as f32).sin())
            .collect::<Vec<_>>();
        let other = target.clone();
        let track_ids = [73u32];
        let mut bins = [0u32; 50];
        let mut intensities = [0.0f32; 50];
        let mut ids = [0u32; 50];
        let written = unsafe {
            hirari_masking_analysis(
                target.as_ptr(),
                other.as_ptr(),
                track_ids.as_ptr(),
                1,
                FFT_SIZE,
                bins.as_mut_ptr(),
                intensities.as_mut_ptr(),
                ids.as_mut_ptr(),
                bins.len(),
            )
        };
        assert!(written > 0);
        assert!(bins[..written].contains(&23));
        assert!(ids[..written].iter().all(|&id| id == 73));
        assert!(intensities[..written]
            .iter()
            .all(|value| *value > CLASH_THRESHOLD));
    }

    #[test]
    fn invalid_frame_size_is_rejected_without_writing_outputs() {
        let target = [0.0f32; FFT_SIZE];
        let mut bins = [u32::MAX; 2];
        let mut intensities = [f32::MAX; 2];
        let mut ids = [u32::MAX; 2];
        let written = unsafe {
            hirari_masking_analysis(
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                FFT_SIZE - 1,
                bins.as_mut_ptr(),
                intensities.as_mut_ptr(),
                ids.as_mut_ptr(),
                bins.len(),
            )
        };
        assert_eq!(written, 0);
        assert_eq!(bins, [u32::MAX; 2]);
        assert_eq!(intensities, [f32::MAX; 2]);
        assert_eq!(ids, [u32::MAX; 2]);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_masking_fft_and_clash_selection_match_frozen_cpp() {
        unsafe extern "C" {
            fn hirari_masking_analysis_reference(
                target_mono: *const f32,
                other_mono: *const f32,
                other_track_ids: *const u32,
                other_count: usize,
                frame_count: usize,
                output_bins: *mut u32,
                output_intensities: *mut f32,
                output_track_ids: *mut u32,
                output_capacity: usize,
            ) -> usize;
        }

        let target = (0..FFT_SIZE)
            .map(|index| {
                let x = index as f32;
                (x * 0.037).sin() * 0.65 + (x * 0.119).cos() * 0.27
            })
            .collect::<Vec<_>>();
        let others = (0..3)
            .flat_map(|track| {
                (0..FFT_SIZE).map(move |index| {
                    let x = index as f32;
                    (x * (0.021 + track as f32 * 0.009)).sin() * (0.72 - track as f32 * 0.1)
                        + (x * (0.083 + track as f32 * 0.017)).cos() * 0.23
                })
            })
            .collect::<Vec<_>>();
        let track_ids = [17u32, 23, 41];
        let mut rust_bins = [0u32; 50];
        let mut rust_intensities = [0.0f32; 50];
        let mut rust_ids = [0u32; 50];
        let mut cpp_bins = [0u32; 50];
        let mut cpp_intensities = [0.0f32; 50];
        let mut cpp_ids = [0u32; 50];

        let rust_count = unsafe {
            hirari_masking_analysis(
                target.as_ptr(),
                others.as_ptr(),
                track_ids.as_ptr(),
                track_ids.len(),
                FFT_SIZE,
                rust_bins.as_mut_ptr(),
                rust_intensities.as_mut_ptr(),
                rust_ids.as_mut_ptr(),
                rust_bins.len(),
            )
        };
        let cpp_count = unsafe {
            hirari_masking_analysis_reference(
                target.as_ptr(),
                others.as_ptr(),
                track_ids.as_ptr(),
                track_ids.len(),
                FFT_SIZE,
                cpp_bins.as_mut_ptr(),
                cpp_intensities.as_mut_ptr(),
                cpp_ids.as_mut_ptr(),
                cpp_bins.len(),
            )
        };
        assert_eq!(rust_count, cpp_count);
        assert_eq!(&rust_bins[..rust_count], &cpp_bins[..cpp_count]);
        assert_eq!(&rust_ids[..rust_count], &cpp_ids[..cpp_count]);
        for index in 0..rust_count {
            let delta = (rust_intensities[index] - cpp_intensities[index]).abs();
            assert!(
                delta <= 2.0e-4 * cpp_intensities[index].abs().max(1.0),
                "clash={index} rust={} cpp={} delta={delta}",
                rust_intensities[index],
                cpp_intensities[index]
            );
        }
    }
}
