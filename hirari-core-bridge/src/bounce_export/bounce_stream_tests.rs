//! Tests split out of export_wav_stream.rs.
    use super::*;

    /// A distinct output path per call. Tests run in parallel threads and the
    /// stream refuses to share a path with a live publication lock, so a clock
    /// read that two threads share would fail one of them for no reason.
    fn temp_path(extension: &str) -> std::path::PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "hirari-bounce-stream-{}-{}.{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            extension
        ))
    }

    #[test]
    fn every_bounce_stream_format_requires_all_frames_before_publishing() {
        let left = [0.25_f32; 16];
        let right = [-0.5_f32; 16];
        let channels = [left.as_ptr(), right.as_ptr()];
        // format, extension, WAVE64 container.
        let cases = [
            (0_u8, "wav", false),
            (1, "wav", false),
            (2, "wav", false),
            (3, "w64", true),
        ];
        for (format, extension, wave64) in cases {
            let path = temp_path(extension);
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_bounce_stream_create(bytes.as_ptr(), bytes.len(), 16, 48_000, format)
            };
            assert!(!stream.is_null(), "format {format} could not be opened");
            assert_eq!(
                unsafe { hirari_bounce_stream_write(stream, channels.as_ptr(), 17) },
                0,
                "format {format} accepted more frames than declared"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_finish(stream) },
                0,
                "format {format} published an incomplete stream"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_write(stream, channels.as_ptr(), 16) },
                1,
                "format {format} rejected a valid block"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_finish(stream) },
                1,
                "format {format} could not publish"
            );
            unsafe { hirari_bounce_stream_free(stream) };
            let data = std::fs::read(&path).unwrap();
            assert!(data.starts_with(b"RIFF"), "format {format} container mismatch");
            if wave64 {
                assert_eq!(&data[4..8], &[0x2e, 0x91, 0xcf, 0x11]);
            } else {
                assert_eq!(&data[8..12], b"WAVE");
            }
            assert_eq!(data.len() % 2, 0);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn unsupported_bounce_stream_format_is_rejected() {
        let path = temp_path("wav");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe {
            hirari_bounce_stream_create(bytes.as_ptr(), bytes.len(), 16, 48_000, 4)
        };
        assert!(stream.is_null());
    }

    fn write_planar(path: &Path, planes: &[Vec<f32>], frames: u64, format: u8) -> u8 {
        let pointers: Vec<*const f32> = planes.iter().map(|plane| plane.as_ptr()).collect();
        let bytes = path.to_str().unwrap().as_bytes();
        unsafe {
            hirari_wav_write_planar(
                bytes.as_ptr(),
                bytes.len(),
                pointers.as_ptr(),
                pointers.len(),
                frames,
                48_000,
                format,
            )
        }
    }

    fn decode(path: &Path) -> (u32, f64, Vec<f32>) {
        let bytes = path.to_str().unwrap().as_bytes();
        let decoded = crate::wav_decoder_compat::decode_wav_file(bytes).expect("decode");
        let samples = unsafe {
            std::slice::from_raw_parts(decoded.samples, decoded.sample_count).to_vec()
        };
        unsafe { crate::preview_audio_runtime::hirari_free_decoded_audio(decoded.samples, decoded.sample_count) };
        (decoded.channels, decoded.sample_rate, samples)
    }

    #[test]
    fn planar_writers_emit_expected_headers_and_replace_destinations() {
        let left = vec![0.0_f32, 0.25];
        let right = vec![-0.25_f32, 0.5];
        let stereo = [left, right];

        let pcm16 = temp_path("wav");
        assert_eq!(write_planar(&pcm16, &stereo, 2, 1), 0);
        let data = std::fs::read(&pcm16).unwrap();
        assert!(data.starts_with(b"RIFF"));
        assert_eq!(&data[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([data[34], data[35]]), 16);
        let (channels, rate, samples) = decode(&pcm16);
        assert_eq!((channels, rate), (2, 48_000.0));
        assert!(samples.iter().all(|value| value.is_finite()));

        let pcm24 = temp_path("wav");
        assert_eq!(write_planar(&pcm24, &stereo, 2, 2), 0);
        let data = std::fs::read(&pcm24).unwrap();
        assert_eq!(u16::from_le_bytes([data[34], data[35]]), 24);

        let multichannel = [
            vec![0.0_f32, 0.25],
            vec![-0.25_f32, 0.5],
            vec![0.75_f32, -0.5],
        ];
        let multi_path = temp_path("wav");
        assert_eq!(write_planar(&multi_path, &multichannel, 2, 1), 0);
        let data = std::fs::read(&multi_path).unwrap();
        assert_eq!(u16::from_le_bytes([data[22], data[23]]), 3);
        assert_eq!(decode(&multi_path).0, 3);

        // Re-exporting over an existing destination replaces it wholesale.
        let replaced = [
            vec![0.25_f32, -0.25, 0.75, -0.5, 0.125, -0.125],
            vec![-0.25_f32, 0.25, 0.5, -0.75, 0.25, -0.25],
        ];
        assert_eq!(write_planar(&multi_path, &replaced, 6, 0), 0);
        let (channels, _, samples) = decode(&multi_path);
        assert_eq!(channels, 2);
        assert_eq!(samples.len(), 12);
        assert_eq!(&samples[..], &[0.25, -0.25, -0.25, 0.25, 0.75, 0.5, -0.5, -0.75, 0.125, 0.25, -0.125, -0.25]);

        for path in [pcm16, pcm24, multi_path] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn pcm_stream_writers_publish_complete_container_sizes() {
        let left = [0.0_f32, 0.25, 0.5, 0.75];
        let right = [-0.25_f32, 0.5, -0.75, 1.0];
        let channels = [left.as_ptr(), right.as_ptr()];
        for (bit_depth, expected_bytes) in [(16_u16, 60_u64), (24, 68)] {
            let path = temp_path("wav");
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_wav_pcm_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 2, bit_depth, 0)
            };
            assert!(!stream.is_null());
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, channels.as_ptr(), 2) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, channels.as_ptr(), 2) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_finish(stream) }, 1);
            unsafe { hirari_wav_pcm_stream_free(stream) };
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected_bytes);
            let data = std::fs::read(&path).unwrap();
            assert_eq!(u16::from_le_bytes([data[34], data[35]]), bit_depth);
            let _ = std::fs::remove_file(path);
        }
        for (bit_depth, expected_bytes) in [(16_u16, 52_u64), (24, 56)] {
            let path = temp_path("wav");
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_wav_pcm_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 1, bit_depth, 0)
            };
            assert!(!stream.is_null());
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, [left.as_ptr()].as_ptr(), 4) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_finish(stream) }, 1);
            unsafe { hirari_wav_pcm_stream_free(stream) };
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected_bytes);
            let data = std::fs::read(&path).unwrap();
            assert_eq!(u16::from_le_bytes([data[22], data[23]]), 1);
            assert_eq!(u16::from_le_bytes([data[34], data[35]]), bit_depth);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn float_stream_sanitises_non_finite_samples() {
        let path = temp_path("wav");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe { hirari_wav_float32_stream_create(bytes.as_ptr().cast(), bytes.len(), 48_000, 2) };
        assert!(!stream.is_null());
        let left = [0.25_f32, f32::NAN];
        let right = [-0.5_f32, 0.75];
        let channels = [left.as_ptr(), right.as_ptr()];
        assert_eq!(unsafe { hirari_wav_float32_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_float32_stream_frames(stream) }, 2);
        assert_eq!(unsafe { hirari_wav_float32_stream_finish(stream) }, 1);
        unsafe { hirari_wav_float32_stream_free(stream) };
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 96);
        let (channels, _, samples) = decode(&path);
        assert_eq!(channels, 2);
        assert!(samples.iter().all(|value| value.is_finite()));
        assert_eq!(&samples[..], &[0.25, -0.5, 0.0, 0.75]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wave64_stream_reports_frames_and_round_trips() {
        let path = temp_path("w64");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe {
            hirari_wav_wave64_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 2)
        };
        assert!(!stream.is_null());
        let left = [0.0_f32, 0.25, 0.5, 0.75];
        let right = [-0.25_f32, 0.5, -0.75, 1.0];
        let channels = [left.as_ptr(), right.as_ptr()];
        assert_eq!(unsafe { hirari_wav_wave64_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_wave64_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_wave64_stream_frames(stream) }, 4);
        assert_eq!(unsafe { hirari_wav_wave64_stream_finish(stream) }, 1);
        unsafe { hirari_wav_wave64_stream_free(stream) };
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 136);
        let (rate, channels, samples) =
            crate::export::read_wave64_float32(&path).expect("wave64 round trip");
        assert_eq!((rate, channels), (48_000, 2));
        assert_eq!(samples.len(), 8);
        assert_eq!(samples[3], 0.5);
        let _ = std::fs::remove_file(path);
    }
