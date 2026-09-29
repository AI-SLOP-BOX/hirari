use super::{
    hirari_region_needs_spectral_stretch, hirari_region_render_block, HirariRegionRenderConfig,
};
use crate::audio_note_curve::{HirariAudioNoteAnchor, HirariAudioNoteCurveView};
use crate::region_processing::{HirariRegionBlockConfig, HirariRegionBlockOutput};

unsafe extern "C" {
    fn hirari_region_needs_spectral_stretch_reference(
        source_rate: f64,
        warp_marker_count: usize,
        pitch_semitones: f32,
        note_curves: *const HirariAudioNoteCurveView,
        note_curve_count: usize,
    ) -> bool;
    fn hirari_region_resampler_reference_kernel(output: *mut f32, capacity: usize) -> bool;
    fn hirari_region_read_warped_reference(
        source: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        position: f64,
        reverse: u8,
        allow_preroll: u8,
        resample_step: f64,
        kernel: *const f32,
        output: *mut f32,
    );
    fn region_pitch_correction_reference_frame(
        source_left: *const f32,
        source_right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        warped_position: f64,
        local_source_rate: f64,
        effective_pitch_ratio: f64,
        base_pitch_ratio: f64,
        loop_relative: u64,
        note_correction_seconds: f64,
        sample_rate: f64,
        minimum_delay: f64,
        delay_range: f64,
        reverse: u8,
        kernel: *const f32,
        output: *mut f32,
    ) -> bool;
    fn hirari_region_wsola_frame_reference(
        left: *const f32,
        right: *const f32,
        source_samples: u64,
        source_offset: u64,
        source_span: u64,
        region_length: u64,
        loop_relative: u64,
        sample_rate: f64,
        source_rate: f64,
        base_pitch_ratio: f64,
        effective_pitch_ratio: f64,
        cache_ids: *mut u64,
        cache_starts: *mut f64,
        cache_capacity: usize,
        sync_group: u32,
        reverse: u8,
        kernel: *const f32,
        window: *const f32,
        output: *mut f32,
    );
}

const SOURCE_SAMPLES: usize = 2_048;
const SOURCE_SPAN: u64 = 1_536;
const FRAMES: usize = 12;
const KERNEL_SIZE: usize = 17 * 64 * 8;
const CACHE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug)]
enum RenderPath {
    Resample,
    PitchCorrection,
    Wsola,
    Spectral,
}

fn compare_path(path: RenderPath) {
    let left = (0..SOURCE_SAMPLES)
        .map(|index| ((index as f32 * 0.037).sin() * 0.68) + ((index % 31) as f32 * 0.002))
        .collect::<Vec<_>>();
    let right = (0..SOURCE_SAMPLES)
        .map(|index| ((index as f32 * 0.053 + 0.8).cos() * 0.54) - ((index % 23) as f32 * 0.001))
        .collect::<Vec<_>>();
    let mut kernel = vec![0.0f32; KERNEL_SIZE];
    let mut rust_kernel = crate::region_resampler::hirari_region_resampler_prepare();
    unsafe {
        assert!(hirari_region_resampler_reference_kernel(
            kernel.as_mut_ptr(),
            kernel.len()
        ));
    }
    if rust_kernel.is_null() {
        rust_kernel = kernel.as_ptr();
    }
    let mut window = [0.0f32; 1_024];
    for (index, value) in window.iter_mut().enumerate() {
        let angle = 6.283_185_307_18_f32 * index as f32 / 1_023.0;
        *value = 0.5 * (1.0 - angle.cos());
    }

    let block = HirariRegionBlockConfig {
        region_offset: 250,
        region_length: 1_024,
        timeline_length: 1_024,
        ..HirariRegionBlockConfig::default()
    };
    let mut gains = [0.0f32; FRAMES];
    let mut pitch_cents = [0.0f64; FRAMES];
    let mut formant_cents = [0.0f64; FRAMES];
    let matched_indices = [-1i64; FRAMES];
    let previous_indices = [-1i64; FRAMES];
    let mut positions = [0.0f64; FRAMES];
    let mut source_rates = [1.0f64; FRAMES];
    for frame in 0..FRAMES {
        gains[frame] = 0.35 + frame as f32 * 0.045;
        formant_cents[frame] = -600.0 + frame as f64 * 92.0;
        positions[frame] = 248.375 + frame as f64 * 1.125;
        source_rates[frame] = match path {
            RenderPath::Resample => 1.25,
            RenderPath::PitchCorrection => 1.0,
            RenderPath::Wsola => 1.25,
            RenderPath::Spectral => 1.0,
        };
    }
    let prepared = HirariRegionBlockOutput {
        gain: gains.as_mut_ptr(),
        pitch_cents: pitch_cents.as_mut_ptr(),
        formant_cents: formant_cents.as_mut_ptr(),
        matched_note_indices: matched_indices.as_ptr() as *mut i64,
        previous_note_indices: previous_indices.as_ptr() as *mut i64,
        source_positions: positions.as_mut_ptr(),
        local_source_rates: source_rates.as_mut_ptr(),
    };
    let base_pitch_ratio = if matches!(path, RenderPath::PitchCorrection) {
        1.2
    } else {
        1.0
    };
    let source_rate = if matches!(path, RenderPath::Wsola) {
        1.25
    } else {
        1.0
    };
    let render = HirariRegionRenderConfig {
        source_samples: SOURCE_SAMPLES as u64,
        source_offset: 0,
        source_span: SOURCE_SPAN,
        source_rate,
        sample_rate: 48_000.0,
        base_pitch_ratio,
        minimum_delay: 256.0,
        delay_range: 512.0,
        cache_owner: 0x7265_6e64,
        cache_snapshot: 0x7265_6e64 ^ path as usize,
        cache_source: left.as_ptr() as usize,
        cache_region_id: 73 + path as u32,
        cache_note_segments: 0,
        cache_note_segment_count: 0,
        cache_pitch_semitones: 12.0 * base_pitch_ratio.log2() as f32,
        sync_group: 0,
        reverse: 0,
        pitch_preserve_warp: u8::from(matches!(path, RenderPath::Wsola)),
        spectral_stretch_ready: u8::from(matches!(path, RenderPath::Spectral)),
    };
    let mut spectral_left = [0.0f32; FRAMES];
    let mut spectral_right = [0.0f32; FRAMES];
    for frame in 0..FRAMES {
        spectral_left[frame] = -0.4 + frame as f32 * 0.025;
        spectral_right[frame] = 0.3 - frame as f32 * 0.017;
    }
    let mut rust_destination_left = [0.1f32; FRAMES];
    let mut rust_destination_right = [-0.2f32; FRAMES];
    let mut cpp_destination_left = rust_destination_left;
    let mut cpp_destination_right = rust_destination_right;
    let mut cpp_cache_ids = [u64::MAX; CACHE_CAPACITY];
    let mut cpp_cache_starts = [0.0f64; CACHE_CAPACITY];
    let spectral = matches!(path, RenderPath::Spectral);

    let rendered = unsafe {
        hirari_region_render_block(
            left.as_ptr(),
            right.as_ptr(),
            &block,
            &render,
            &prepared,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            rust_kernel,
            window.as_ptr(),
            if spectral {
                spectral_left.as_ptr()
            } else {
                std::ptr::null()
            },
            if spectral {
                spectral_right.as_ptr()
            } else {
                std::ptr::null()
            },
            rust_destination_left.as_mut_ptr(),
            rust_destination_right.as_mut_ptr(),
            FRAMES as u32,
        )
    };
    assert!(rendered);

    for frame in 0..FRAMES {
        let loop_relative = block.region_offset + frame as u64;
        let mut sample_left = [0.0f32; 2];
        let mut sample_right = [0.0f32; 2];
        if spectral {
            sample_left[0] = spectral_left[frame];
            sample_right[0] = spectral_right[frame];
        } else if matches!(path, RenderPath::Wsola) {
            let mut wsola = [0.0f32; 4];
            unsafe {
                hirari_region_wsola_frame_reference(
                    left.as_ptr(),
                    right.as_ptr(),
                    SOURCE_SAMPLES as u64,
                    0,
                    SOURCE_SPAN,
                    block.region_length,
                    loop_relative,
                    render.sample_rate,
                    render.source_rate,
                    render.base_pitch_ratio,
                    render.base_pitch_ratio,
                    cpp_cache_ids.as_mut_ptr(),
                    cpp_cache_starts.as_mut_ptr(),
                    CACHE_CAPACITY,
                    render.sync_group,
                    render.reverse,
                    kernel.as_ptr(),
                    window.as_ptr(),
                    wsola.as_mut_ptr(),
                );
            }
            sample_left = [wsola[0], wsola[1]];
            sample_right = [wsola[2], wsola[3]];
        } else if matches!(path, RenderPath::PitchCorrection) {
            let mut corrected = [0.0f32; 4];
            let effective_pitch_ratio = render.base_pitch_ratio;
            let ok = unsafe {
                region_pitch_correction_reference_frame(
                    left.as_ptr(),
                    right.as_ptr(),
                    SOURCE_SAMPLES as u64,
                    0,
                    SOURCE_SPAN,
                    positions[frame],
                    source_rates[frame],
                    effective_pitch_ratio,
                    render.base_pitch_ratio,
                    loop_relative,
                    0.0,
                    render.sample_rate,
                    render.minimum_delay,
                    render.delay_range,
                    render.reverse,
                    kernel.as_ptr(),
                    corrected.as_mut_ptr(),
                )
            };
            assert!(ok);
            sample_left = [corrected[0], corrected[1]];
            sample_right = [corrected[2], corrected[3]];
        } else {
            unsafe {
                hirari_region_read_warped_reference(
                    left.as_ptr(),
                    SOURCE_SAMPLES as u64,
                    0,
                    SOURCE_SPAN,
                    positions[frame],
                    render.reverse,
                    0,
                    source_rates[frame],
                    kernel.as_ptr(),
                    sample_left.as_mut_ptr(),
                );
                hirari_region_read_warped_reference(
                    right.as_ptr(),
                    SOURCE_SAMPLES as u64,
                    0,
                    SOURCE_SPAN,
                    positions[frame],
                    render.reverse,
                    0,
                    source_rates[frame],
                    kernel.as_ptr(),
                    sample_right.as_mut_ptr(),
                );
            }
        }
        if !spectral {
            let tilt = (formant_cents[frame].clamp(-2400.0, 2400.0) / 2400.0) as f32;
            sample_left[0] = (sample_left[0] + sample_left[1] * tilt * 0.5).clamp(-2.0, 2.0);
            sample_right[0] = (sample_right[0] + sample_right[1] * tilt * 0.5).clamp(-2.0, 2.0);
        }
        if sample_left[0].is_finite() && sample_right[0].is_finite() {
            cpp_destination_left[frame] += sample_left[0] * gains[frame];
            cpp_destination_right[frame] += sample_right[0] * gains[frame];
        }
    }

    for frame in 0..FRAMES {
        assert!(
            (rust_destination_left[frame] - cpp_destination_left[frame]).abs() <= 2.0e-4,
            "left path={path:?}, frame={frame}: Rust={}, C++={}",
            rust_destination_left[frame],
            cpp_destination_left[frame]
        );
        assert!(
            (rust_destination_right[frame] - cpp_destination_right[frame]).abs() <= 2.0e-4,
            "right path={path:?}, frame={frame}: Rust={}, C++={}",
            rust_destination_right[frame],
            cpp_destination_right[frame]
        );
    }
    if matches!(path, RenderPath::Wsola) {
        let (rust_cache_ids, rust_cache_starts) = unsafe { super::cache_for_render(&render) };
        let rust_cache_ids = unsafe { std::slice::from_raw_parts(rust_cache_ids, CACHE_CAPACITY) };
        let rust_cache_starts =
            unsafe { std::slice::from_raw_parts(rust_cache_starts, CACHE_CAPACITY) };
        assert_eq!(rust_cache_ids, cpp_cache_ids);
        for (rust, cpp) in rust_cache_starts.iter().zip(cpp_cache_starts) {
            assert!((rust - cpp).abs() <= 1.0e-9);
        }
    }
}

#[test]
fn rust_track_region_renderer_matches_frozen_cpp_modes_and_block_edges() {
    assert_eq!(std::mem::size_of::<HirariRegionRenderConfig>(), 128);
    compare_path(RenderPath::Resample);
    compare_path(RenderPath::PitchCorrection);
    compare_path(RenderPath::Wsola);
    compare_path(RenderPath::Spectral);
}

#[test]
fn rust_spectral_stretch_eligibility_matches_frozen_cpp_thresholds() {
    let neutral_anchor = [HirariAudioNoteAnchor {
        position_seconds: 0.0,
        pitch_cents: 1.0e-3,
        formant_cents: -1.0e-3,
    }];
    let changed_pitch_anchor = [HirariAudioNoteAnchor {
        position_seconds: 0.0,
        pitch_cents: 1.001e-3,
        formant_cents: 0.0,
    }];
    let changed_formant_anchor = [HirariAudioNoteAnchor {
        position_seconds: 0.0,
        pitch_cents: 0.0,
        formant_cents: -1.001e-3,
    }];
    let curve = |anchors: &[HirariAudioNoteAnchor], pitch_offset, formant_offset| {
        HirariAudioNoteCurveView {
            anchors: if anchors.is_empty() {
                std::ptr::null()
            } else {
                anchors.as_ptr()
            },
            anchor_count: anchors.len(),
            integral_prefix: std::ptr::null(),
            start_seconds: 0.0,
            end_seconds: 1.0,
            pitch_offset_cents: pitch_offset,
            correction_before_seconds: 0.0,
            formant_offset_cents: formant_offset,
        }
    };
    let fixtures = [
        (1.0, 0, 0.0, curve(&[], 0.0, 0.0)),
        (1.000_009, 0, 0.0, curve(&[], 0.0, 0.0)),
        (1.000_011, 0, 0.0, curve(&[], 0.0, 0.0)),
        (1.0, 1, 0.0, curve(&[], 0.0, 0.0)),
        (1.0, 0, 1.0e-4, curve(&[], 0.0, 0.0)),
        (1.0, 0, 1.001e-4, curve(&[], 0.0, 0.0)),
        (1.0, 0, 0.0, curve(&[], 1.0e-3, -1.0e-3)),
        (1.0, 0, 0.0, curve(&[], 1.001e-3, 0.0)),
        (1.0, 0, 0.0, curve(&[], 0.0, -1.001e-3)),
        (1.0, 0, 0.0, curve(&neutral_anchor, 0.0, 0.0)),
        (1.0, 0, 0.0, curve(&changed_pitch_anchor, 0.0, 0.0)),
        (1.0, 0, 0.0, curve(&changed_formant_anchor, 0.0, 0.0)),
    ];
    for (source_rate, warp_count, semitones, curve) in fixtures {
        let pointer = if curve.anchor_count == 0
            && curve.pitch_offset_cents == 0.0
            && curve.formant_offset_cents == 0.0
        {
            std::ptr::null()
        } else {
            &curve as *const HirariAudioNoteCurveView
        };
        let count = usize::from(!pointer.is_null());
        let rust = unsafe {
            hirari_region_needs_spectral_stretch(source_rate, warp_count, semitones, pointer, count)
        };
        let cpp = unsafe {
            hirari_region_needs_spectral_stretch_reference(
                source_rate,
                warp_count,
                semitones,
                pointer,
                count,
            )
        };
        assert_eq!(
            rust, cpp,
            "rate={source_rate}, warp={warp_count}, semitones={semitones}, curve={curve:?}"
        );
    }
}
