//! Pure Rust presentation transforms for the engine's fixed 512-bin spectrum.
//! Sampling the live engine stays in the native adapter; display math lives here.

const FFT_BINS: usize = 512;
const DISPLAY_BANDS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrangementSection {
    pub start_sample: u64,
    pub end_sample: u64,
    pub section_type: u8,
    pub energy_level: f32,
    pub motivic_id: u32,
    pub narrative_flow_score: f32,
}

/// Replaces the native NeuralArrangementKernel. Region starts remain an input
/// for API compatibility with the engine snapshot; the prior algorithm only
/// used the maximum project length and harmonic context.
pub fn analysis_structure_sections(
    _region_starts: &[u64],
    total_length: u64,
    bpm: f64,
    sample_rate: f64,
    tension: f32,
    valence: f32,
) -> Vec<ArrangementSection> {
    if total_length == 0
        || !bpm.is_finite()
        || bpm <= 0.0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !tension.is_finite()
        || !valence.is_finite()
    {
        return Vec::new();
    }
    let samples_per_beat = (60.0 / bpm) * sample_rate;
    let bar_block_f64 = samples_per_beat * 4.0 * 8.0;
    if !bar_block_f64.is_finite() || bar_block_f64 < 1.0 || bar_block_f64 > u64::MAX as f64 {
        return Vec::new();
    }
    let bar_block = bar_block_f64 as u64;
    let mut sections = Vec::new();
    let mut start = 0u64;
    while start < total_length {
        let end = start.saturating_add(bar_block).min(total_length);
        let energy_level = tension;
        sections.push(ArrangementSection {
            start_sample: start,
            end_sample: end,
            section_type: if energy_level > 0.8 {
                2 // Chorus
            } else if energy_level < 0.2 {
                1 // Verse
            } else {
                3 // Bridge
            },
            energy_level,
            motivic_id: (energy_level * 1000.0) as u32,
            narrative_flow_score: energy_level * valence,
        });
        if end == total_length {
            break;
        }
        start = end;
    }

    // Preserve NeuralArrangementKernel::OptimizeStructure's chronological
    // ordering and half-step smoothing of adjacent energy changes.
    sections.sort_by_key(|section| section.start_sample);
    for index in 1..sections.len() {
        let previous = sections[index - 1].energy_level;
        let current = sections[index].energy_level;
        if (current - previous).abs() > 0.5 {
            sections[index].energy_level = previous + (current - previous) * 0.5;
        }
    }
    sections
}

#[inline]
fn sample(spectrum: &[f32], index: usize) -> f32 {
    spectrum
        .get(index)
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn valid_spectrum(spectrum: &[f32]) -> bool {
    spectrum.len() == FFT_BINS
}

pub fn analysis_spectral_display(spectrum: &[f32], sample_rate: f64) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    let sample_rate = if sample_rate.is_finite() {
        sample_rate.clamp(8_000.0, 384_000.0)
    } else {
        48_000.0
    };
    let nyquist = sample_rate * 0.5;
    (0..FFT_BINS)
        .map(|index| {
            let normalized = index as f64 / (FFT_BINS - 1) as f64;
            let frequency = 20.0 * (nyquist / 20.0).powf(normalized);
            let bin = frequency / nyquist * (FFT_BINS - 1) as f64;
            let low = (bin as usize).min(FFT_BINS - 1);
            let high = (low + 1).min(FFT_BINS - 1);
            let a = sample(spectrum, low);
            let b = if spectrum.get(high).is_some_and(|value| value.is_finite()) {
                sample(spectrum, high)
            } else {
                a
            };
            (a + (b - a) * (bin - low as f64) as f32).clamp(0.0, 1.0)
        })
        .collect()
}

pub fn analysis_phase_heatmap(spectrum: &[f32]) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    (0..DISPLAY_BANDS)
        .map(|band| (sample(spectrum, band * 4 + 1) - sample(spectrum, band * 4)).clamp(-1.0, 1.0))
        .collect()
}

pub fn analysis_partials(spectrum: &[f32]) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(128);
    for bin in 1..FFT_BINS - 1 {
        let value = sample(spectrum, bin);
        if value >= sample(spectrum, bin - 1) && value >= sample(spectrum, bin + 1) && value > 0.01
        {
            result.extend([bin as f32, value]);
        }
    }
    result
}

pub fn analysis_mel_spectrogram(spectrum: &[f32]) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    let width = (FFT_BINS - 1) as f32 / (DISPLAY_BANDS - 1) as f32;
    (0..DISPLAY_BANDS)
        .map(|band| {
            let center = band as f32 / (DISPLAY_BANDS - 1) as f32 * (FFT_BINS - 1) as f32;
            let first = (center - width).floor().max(0.0) as usize;
            let last = (center + width).ceil().min((FFT_BINS - 1) as f32) as usize;
            let (mut weighted, mut weight_sum) = (0.0, 0.0);
            for bin in first..=last {
                let weight = (1.0 - (bin as f32 - center).abs() / width).max(0.0);
                weighted += sample(spectrum, bin).max(0.0) * weight;
                weight_sum += weight;
            }
            if weight_sum > 0.0 {
                (weighted / weight_sum).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect()
}

pub fn analysis_motion_vectors(spectrum: &[f32]) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(DISPLAY_BANDS * 2);
    for band in 0..DISPLAY_BANDS {
        let left = sample(spectrum, band * 4);
        let right = sample(spectrum, (band * 4 + 3).min(FFT_BINS - 1));
        let magnitude = (0.5 * (left.abs() + right.abs())).clamp(0.0, 1.0);
        result.extend([
            (right - left).clamp(-1.0, 1.0) * (0.25 + 0.75 * magnitude),
            magnitude,
        ]);
    }
    result
}

pub fn analysis_motion_energy(spectrum: &[f32]) -> f32 {
    if !valid_spectrum(spectrum) {
        return 0.0;
    }
    let total: f32 = (1..FFT_BINS)
        .map(|index| (sample(spectrum, index) - sample(spectrum, index - 1)).abs())
        .sum();
    total / (FFT_BINS - 1) as f32
}

pub fn analysis_synesthesia_colors(spectrum: &[f32]) -> Vec<f32> {
    if !valid_spectrum(spectrum) {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(DISPLAY_BANDS * 3);
    for band in 0..DISPLAY_BANDS {
        let value = sample(spectrum, band * 4).abs().clamp(0.0, 1.0);
        let h6 = (band % 12) as f32 / 12.0 * 6.0;
        let sector = h6 as usize % 6;
        let fraction = h6 - h6.floor();
        let q = value * (1.0 - 0.65 * fraction);
        let t = value * (1.0 - 0.65 * (1.0 - fraction));
        let rgb = match sector {
            0 => [value, t, 0.0],
            1 => [q, value, 0.0],
            2 => [0.0, value, t],
            3 => [0.0, q, value],
            4 => [t, 0.0, value],
            _ => [value, 0.0, q],
        };
        result.extend(rgb);
    }
    result
}

pub fn analysis_creative_advice(true_peak_l: f32, true_peak_r: f32, correlation: f32) -> String {
    if true_peak_l > -0.1 || true_peak_r > -0.1 {
        "Reduce master gain: true peak is close to clipping.".into()
    } else if correlation < 0.0 {
        "Check stereo phase: the master correlation is negative.".into()
    } else {
        "Master headroom and stereo correlation are currently healthy.".into()
    }
}

pub fn analysis_mixing_advice(true_peak_l: f32, true_peak_r: f32, correlation: f32) -> Vec<f32> {
    let mut result = Vec::with_capacity(6);
    if true_peak_l.max(true_peak_r) > -0.1 {
        result.extend([1001.0, 3.0, 0.0]);
    }
    if correlation < 0.0 {
        result.extend([1002.0, 2.0, 0.0]);
    }
    result
}

pub fn analysis_arrangement_advice(_track_id: u32) -> String {
    "Arrangement balanced. Suggest spectral lift at 16k.".into()
}

pub fn analysis_spectral_clashes(true_peak_l: f32, true_peak_r: f32, correlation: f32) -> Vec<f32> {
    analysis_mixing_advice(true_peak_l, true_peak_r, correlation)
        .chunks_exact(3)
        .filter(|advice| advice[0] >= 1000.0)
        .flat_map(|advice| [advice[0], advice[1]])
        .collect()
}

pub fn analysis_song_structure_json(
    kinds: &[u8],
    starts: &[u64],
    ends: &[u64],
    energies: &[f32],
    flows: &[f32],
) -> String {
    let count = [
        kinds.len(),
        starts.len(),
        ends.len(),
        energies.len(),
        flows.len(),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);
    let mut json = String::from("{\"available\":true,\"sections\":[");
    for index in 0..count {
        if index > 0 {
            json.push(',');
        }
        let name = match kinds[index] {
            0 => "Intro",
            1 => "Verse",
            2 => "Chorus",
            3 => "Bridge",
            4 => "Outro",
            _ => "Unknown",
        };
        let energy = if energies[index].is_finite() {
            energies[index]
        } else {
            0.0
        };
        let flow = if flows[index].is_finite() {
            flows[index]
        } else {
            0.0
        };
        use std::fmt::Write as _;
        let _ = write!(
            json,
            "{{\"name\":\"{name}\",\"start_sample\":{},\"end_sample\":{},\"energy\":{energy},\"flow\":{flow}}}",
            starts[index], ends[index]
        );
    }
    json.push_str("]}");
    json
}

pub fn analysis_dashboard_json(
    lufs_short: f32,
    lufs_integrated: f32,
    true_peak_l: f32,
    true_peak_r: f32,
    correlation: f32,
) -> String {
    let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
    let lufs_short = finite(lufs_short);
    let lufs_integrated = finite(lufs_integrated);
    let true_peak_l = finite(true_peak_l);
    let true_peak_r = finite(true_peak_r);
    let correlation = finite(correlation);
    let health = (1.0 - true_peak_l.abs().max(true_peak_r.abs()) * 0.1).clamp(0.0, 1.0);
    format!(
        "{{\"available\":true,\"health\":{health:.3},\"lufs_short\":{lufs_short:.2},\"lufs_integrated\":{lufs_integrated:.2},\"true_peak_l\":{true_peak_l:.2},\"true_peak_r\":{true_peak_r:.2},\"correlation\":{correlation:.3}}}"
    )
}

#[cfg(test)]
mod arrangement_tests {
    use super::*;

    #[test]
    fn arrangement_sections_match_eight_bar_boundaries_and_labels() {
        let sections =
            analysis_structure_sections(&[32, 900_000], 1_600_000, 120.0, 48_000.0, 0.9, 0.5);
        assert_eq!(sections.len(), 3);
        assert_eq!(
            (sections[0].start_sample, sections[0].end_sample),
            (0, 768_000)
        );
        assert_eq!(
            (sections[1].start_sample, sections[1].end_sample),
            (768_000, 1_536_000)
        );
        assert_eq!(
            (sections[2].start_sample, sections[2].end_sample),
            (1_536_000, 1_600_000)
        );
        assert!(sections.iter().all(|section| section.section_type == 2));
        assert!(sections.iter().all(|section| section.motivic_id == 900));
        assert!(sections
            .iter()
            .all(|section| section.narrative_flow_score == 0.45));
    }

    #[test]
    fn arrangement_sections_reject_invalid_timing_without_looping() {
        assert!(analysis_structure_sections(&[], 1000, 0.0, 48_000.0, 0.5, 0.5).is_empty());
        assert!(analysis_structure_sections(&[], 1000, 120.0, f64::NAN, 0.5, 0.5).is_empty());
        assert!(analysis_structure_sections(&[], 0, 120.0, 48_000.0, 0.5, 0.5).is_empty());
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn arrangement_sections_match_frozen_cpp_kernel() {
        unsafe extern "C" {
            fn hirari_analysis_structure_reference(
                region_starts: *const u64,
                region_count: usize,
                total_length: u64,
                bpm: f64,
                sample_rate: f64,
                tension: f32,
                valence: f32,
                starts: *mut u64,
                ends: *mut u64,
                kinds: *mut u8,
                energies: *mut f32,
                motivic_ids: *mut u32,
                flows: *mut f32,
                capacity: usize,
            ) -> usize;
        }

        for (bpm, length, tension, valence) in [
            (120.0_f64, 1_600_000_u64, 0.9_f32, 0.5_f32),
            (90.0_f64, 2_400_000_u64, 0.1_f32, 0.25_f32),
            (127.5_f64, 1_100_000_u64, 0.45_f32, 0.8_f32),
        ] {
            let regions = [0, 12_345, length.saturating_sub(1)];
            let rust =
                analysis_structure_sections(&regions, length, bpm, 48_000.0, tension, valence);
            let capacity = rust.len().max(1);
            let mut starts = vec![0; capacity];
            let mut ends = vec![0; capacity];
            let mut kinds = vec![0; capacity];
            let mut energies = vec![0.0; capacity];
            let mut motivic_ids = vec![0; capacity];
            let mut flows = vec![0.0; capacity];
            let count = unsafe {
                hirari_analysis_structure_reference(
                    regions.as_ptr(),
                    regions.len(),
                    length,
                    bpm,
                    48_000.0,
                    tension,
                    valence,
                    starts.as_mut_ptr(),
                    ends.as_mut_ptr(),
                    kinds.as_mut_ptr(),
                    energies.as_mut_ptr(),
                    motivic_ids.as_mut_ptr(),
                    flows.as_mut_ptr(),
                    capacity,
                )
            };
            assert_eq!(count, rust.len());
            for (index, section) in rust.iter().enumerate() {
                assert_eq!(section.start_sample, starts[index]);
                assert_eq!(section.end_sample, ends[index]);
                assert_eq!(section.section_type, kinds[index]);
                assert_eq!(section.energy_level, energies[index]);
                assert_eq!(section.motivic_id, motivic_ids[index]);
                assert_eq!(section.narrative_flow_score, flows[index]);
            }
        }
    }
}
