//! Tests split out of region_processing.rs.

    use super::{hirari_region_render_chunks, HirariRegionBlockConfig, HirariRegionBlockOutput};
    use crate::audio_note_curve::{
        HirariAudioNoteAnchor, HirariAudioNoteCurveView, HirariAudioNoteSegmentRange,
    };
    use crate::region_render::{hirari_region_render_block, HirariRegionRenderConfig};

    #[test]
    fn chunk_scheduler_matches_manual_chunk_rendering_across_tail_chunk() {
        const SOURCE_FRAMES: usize = 512;
        const OUTPUT_FRAMES: usize = 300;
        let source_left =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.017).sin() * 0.8);
        let source_right =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.023).cos() * 0.6);
        let mut scheduled_left = [0.0; OUTPUT_FRAMES];
        let mut scheduled_right = [0.0; OUTPUT_FRAMES];
        let mut manual_left = [0.0; OUTPUT_FRAMES];
        let mut manual_right = [0.0; OUTPUT_FRAMES];
        let mut window = [0.0f32; 1024];
        window.fill(1.0);
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let block_template = HirariRegionBlockConfig {
            region_length: SOURCE_FRAMES as u64,
            timeline_length: SOURCE_FRAMES as u64,
            clip_gain: 0.75,
            sample_rate: 48_000.0,
            source_rate: 1.0,
            source_span: SOURCE_FRAMES as u64,
            ..Default::default()
        };
        let render_config = HirariRegionRenderConfig {
            source_samples: SOURCE_FRAMES as u64,
            source_span: SOURCE_FRAMES as u64,
            source_rate: 1.0,
            sample_rate: 48_000.0,
            base_pitch_ratio: 1.0,
            minimum_delay: 256.0,
            delay_range: 256.0,
            ..Default::default()
        };

        unsafe {
            hirari_region_render_chunks(
                source_left.as_ptr(),
                source_right.as_ptr(),
                &block_template,
                &render_config,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                kernel,
                window.as_ptr(),
                std::ptr::null_mut(),
                scheduled_left.as_mut_ptr(),
                scheduled_right.as_mut_ptr(),
                OUTPUT_FRAMES as u32,
            );

            let mut chunk_start = 0;
            while chunk_start < OUTPUT_FRAMES {
                let frames = (OUTPUT_FRAMES - chunk_start).min(256);
                let mut config = block_template;
                config.region_offset = chunk_start as u64;
                let mut gain = [0.0f32; 256];
                let mut pitch = [0.0f64; 256];
                let mut formant = [0.0f64; 256];
                let mut matched = [-1i64; 256];
                let mut previous = [-1i64; 256];
                let mut positions = [0.0f64; 256];
                let mut rates = [0.0f64; 256];
                let output = HirariRegionBlockOutput {
                    gain: gain.as_mut_ptr(),
                    pitch_cents: pitch.as_mut_ptr(),
                    formant_cents: formant.as_mut_ptr(),
                    matched_note_indices: matched.as_mut_ptr(),
                    previous_note_indices: previous.as_mut_ptr(),
                    source_positions: positions.as_mut_ptr(),
                    local_source_rates: rates.as_mut_ptr(),
                };
                assert!(super::hirari_region_prepare_block(
                    &config,
                    frames as u32,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    &output,
                ));
                assert!(hirari_region_render_block(
                    source_left.as_ptr(),
                    source_right.as_ptr(),
                    &config,
                    &render_config,
                    &output,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    kernel,
                    window.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    manual_left.as_mut_ptr().add(chunk_start),
                    manual_right.as_mut_ptr().add(chunk_start),
                    frames as u32,
                ));
                chunk_start += frames;
            }
        }

        assert_eq!(scheduled_left, manual_left);
        assert_eq!(scheduled_right, manual_right);
    }

    #[test]
    fn spectral_stretch_scheduler_renders_note_controlled_slices_in_rust() {
        const SOURCE_FRAMES: usize = 8192;
        const OUTPUT_FRAMES: usize = 8192;
        const SAMPLE_RATE: f64 = 48_000.0;
        let source_left =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.031).sin() * 0.7);
        let source_right =
            std::array::from_fn::<_, SOURCE_FRAMES, _>(|index| (index as f32 * 0.027).cos() * 0.5);
        let mut destination_left = [0.0f32; OUTPUT_FRAMES];
        let mut destination_right = [0.0f32; OUTPUT_FRAMES];
        let mut window = [1.0f32; 1024];
        window[0] = 0.0;
        let kernel = crate::region_resampler::hirari_region_resampler_prepare();
        let block_template = HirariRegionBlockConfig {
            region_length: SOURCE_FRAMES as u64,
            timeline_length: SOURCE_FRAMES as u64,
            clip_gain: 1.0,
            sample_rate: SAMPLE_RATE,
            source_rate: 1.0,
            source_span: SOURCE_FRAMES as u64,
            ..Default::default()
        };
        let note_range = [HirariAudioNoteSegmentRange {
            start_seconds: 0.0,
            end_seconds: SOURCE_FRAMES as f64 / SAMPLE_RATE,
            detected_pitch_cents: 6900.0,
        }];
        let anchors = [HirariAudioNoteAnchor {
            position_seconds: 0.0,
            pitch_cents: 125.0,
            formant_cents: -40.0,
        }];
        let note_curve = [HirariAudioNoteCurveView {
            anchors: anchors.as_ptr(),
            anchor_count: anchors.len(),
            integral_prefix: std::ptr::null(),
            start_seconds: 0.0,
            end_seconds: SOURCE_FRAMES as f64 / SAMPLE_RATE,
            pitch_offset_cents: 0.0,
            correction_before_seconds: 0.0,
            formant_offset_cents: 0.0,
        }];
        let render_config = HirariRegionRenderConfig {
            source_samples: SOURCE_FRAMES as u64,
            source_span: SOURCE_FRAMES as u64,
            source_rate: 1.0,
            sample_rate: SAMPLE_RATE,
            base_pitch_ratio: 1.0,
            minimum_delay: 256.0,
            delay_range: 256.0,
            cache_pitch_semitones: 0.0,
            spectral_stretch_ready: 1,
            ..Default::default()
        };
        let stretch = crate::region_time_stretch::hirari_region_stretch_create();
        assert!(!stretch.is_null());
        unsafe {
            assert!(crate::region_time_stretch::hirari_region_stretch_prepare(
                stretch,
                SAMPLE_RATE,
                OUTPUT_FRAMES as u32,
            ));
            hirari_region_render_chunks(
                source_left.as_ptr(),
                source_right.as_ptr(),
                &block_template,
                &render_config,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                note_range.as_ptr(),
                note_range.len(),
                note_curve.as_ptr(),
                note_curve.len(),
                std::ptr::null(),
                0,
                kernel,
                window.as_ptr(),
                stretch,
                destination_left.as_mut_ptr(),
                destination_right.as_mut_ptr(),
                OUTPUT_FRAMES as u32,
            );
            assert!(
                !crate::region_time_stretch::hirari_region_stretch_output_left(stretch).is_null()
            );
            crate::region_time_stretch::hirari_region_stretch_destroy(stretch);
        }
        assert!(destination_left.iter().all(|sample| sample.is_finite()));
        assert!(destination_right.iter().all(|sample| sample.is_finite()));
        assert!(destination_left.iter().any(|sample| sample.abs() > 1.0e-6));
        assert!(destination_right.iter().any(|sample| sample.abs() > 1.0e-6));
    }
