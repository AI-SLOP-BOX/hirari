//! Tests split out of audio_buffer_ops.rs.

    use super::hirari_audio_buffer_render_frozen;

    #[test]
    fn copies_bounded_audio_and_leaves_missing_channels_silent() {
        let source_left = [1.0_f32, 2.0, 3.0, 4.0];
        let source_right = [-1.0_f32, -2.0, -3.0, -4.0];
        let sources = [source_left.as_ptr(), source_right.as_ptr()];
        let mut left = [9.0_f32; 6];
        let mut right = [9.0_f32; 6];
        let mut extra = [9.0_f32; 6];
        let mut destinations = [left.as_mut_ptr(), right.as_mut_ptr(), extra.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                3,
                6,
                sources.as_ptr(),
                2,
                4,
                3,
                1,
                4,
            )
        };
        assert_eq!(copied, 2);
        assert_eq!(left, [2.0, 3.0, 0.0, 0.0, 9.0, 9.0]);
        assert_eq!(right, [-2.0, -3.0, 0.0, 0.0, 9.0, 9.0]);
        assert_eq!(extra, [0.0, 0.0, 0.0, 0.0, 9.0, 9.0]);
    }

    #[test]
    fn clears_requested_output_when_source_is_missing_or_exhausted() {
        let mut left = [7.0_f32; 4];
        let mut destinations = [left.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                1,
                4,
                std::ptr::null(),
                0,
                0,
                8,
                8,
                4,
            )
        };
        assert_eq!(copied, 0);
        assert_eq!(left, [0.0; 4]);
    }

    #[test]
    fn bounds_copy_by_source_capacity_even_when_timeline_is_longer() {
        let source = [3.0_f32, 4.0];
        let sources = [source.as_ptr()];
        let mut output = [8.0_f32; 4];
        let mut destinations = [output.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                1,
                4,
                sources.as_ptr(),
                1,
                2,
                20,
                1,
                4,
            )
        };
        assert_eq!(copied, 1);
        assert_eq!(output, [4.0, 0.0, 0.0, 0.0]);
    }
