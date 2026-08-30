// Export writer and format contract tests.

use super::*;
mod tests {
    use super::*;

    #[test]
    fn writes_valid_pcm16_wav_atomically() {
        let path = std::env::temp_dir().join(format!("aura-export-{}.wav", std::process::id()));
        write_wav_pcm16(&path, &[0.0, 0.5, -0.5, 1.0], 48_000, 2).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(bytes.len(), 52);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn writes_valid_wave64_float_with_64_bit_chunk_sizes() {
        let path = std::env::temp_dir().join(format!("aura-export-{}.w64", std::process::id()));
        write_wave64_float32(&path, &[0.0, 0.5, -0.5, 1.0], 48_000, 2).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[24..28], b"WAVE");
        assert_eq!(u64::from_le_bytes(bytes[16..24].try_into().unwrap()), bytes.len() as u64);
        assert_eq!(&bytes[40..44], b"fmt ");
        assert_eq!(&bytes[80..84], b"data");
        assert_eq!(u64::from_le_bytes(bytes[96..104].try_into().unwrap()), 40);
        assert_eq!(bytes.len(), 120);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wave64_float_writer_round_trips_through_reader() {
        let path = std::env::temp_dir().join(format!("aura-export-roundtrip-{}.w64", std::process::id()));
        let source = vec![0.0, 0.5, -0.5, 1.0];
        write_wave64_float32(&path, &source, 96_000, 2).unwrap();
        let (rate, channels, decoded) = read_wave64_float32(&path).unwrap();
        assert_eq!(rate, 96_000);
        assert_eq!(channels, 2);
        assert_eq!(decoded, source);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn writes_pcm24_and_pcm32_headers_and_payloads() {
        for bit_depth in [24, 32] {
            let path = std::env::temp_dir().join(format!(
                "aura-export-{}-{}.wav",
                std::process::id(),
                bit_depth
            ));
            write_wav_pcm(&path, &[0.0, 0.5, -0.5, 1.0], 48_000, 2, bit_depth).unwrap();
            let bytes = fs::read(&path).unwrap();
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), bit_depth as u16);
            assert_eq!(bytes.len(), 44 + 4 * (bit_depth as usize / 8));
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn preserves_interleaved_channel_order_for_multichannel_export() {
        let path = std::env::temp_dir().join(format!(
            "aura-export-multichannel-{}-{}.wav",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        // Two frames in explicit L/R/C order. The payload must remain
        // interleaved in that order rather than being silently downmixed.
        write_wav_pcm16(&path, &[0.25, -0.25, 0.5, -0.5, 0.75, -0.75], 48_000, 3)
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 3);
        assert_eq!(u16::from_le_bytes([bytes[32], bytes[33]]), 6);
        let payload = &bytes[44..];
        let samples: Vec<i16> = payload
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        assert_eq!(samples, vec![8192, -8192, 16384, -16384, 24575, -24575]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_invalid_export_input() {
        let path = std::env::temp_dir().join("aura-invalid-export.wav");
        assert_eq!(
            write_wav_pcm16(&path, &[0.0, f32::NAN], 48_000, 1),
            Err(WavExportError::NonFiniteSample)
        );
        assert_eq!(
            write_wav_pcm16(&path, &[0.0], 48_000, 2),
            Err(WavExportError::IncompleteFrame)
        );
        assert_eq!(
            write_wav_pcm16(&path, &[], 48_000, 2),
            Err(WavExportError::EmptyBuffer)
        );
        assert_eq!(
            export_interleaved_buffer_to_wav(&path, &[0.0, 0.0], 48_000, 2, false),
            Err(WavExportError::RendererNotConnected)
        );
        assert_eq!(
            export_interleaved_buffer_to_wave64(&path, &[0.0, 0.0], 48_000, 2, false),
            Err(WavExportError::RendererNotConnected)
        );
    }

    #[test]
    fn supports_unicode_path_and_rejects_empty_or_corrupt_wav() {
        let dir = std::env::temp_dir().join(format!("aura-音声-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("テイク-96k.wav");
        write_wav_pcm(&path, &[0.0, 0.25, -0.25, 0.5], 96_000, 2, 32).unwrap();
        assert!(path.is_file());
        assert!(read_wave64_float32(&path).is_err());
        let empty = dir.join("empty.wav");
        fs::write(&empty, []).unwrap();
        assert!(read_wave64_float32(&empty).is_err());
        let corrupt = dir.join("corrupt.wav");
        fs::write(&corrupt, b"not-a-wave").unwrap();
        assert!(read_wave64_float32(&corrupt).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exports_multiple_sample_rates_without_cross_contamination() {
        let root = std::env::temp_dir().join(format!("aura-rates-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let rates = [44_100_u32, 48_000, 96_000];
        let mut paths = Vec::new();
        for rate in rates {
            let path = root.join(format!("{rate}.wav"));
            write_wav_pcm(&path, &[0.0, 0.25, -0.25, 0.5], rate, 1, 24).unwrap();
            paths.push(path);
        }
        for (path, rate) in paths.iter().zip(rates) {
            let bytes = fs::read(path).unwrap();
            let stored = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
            assert_eq!(stored, rate);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn orchestrator_exposes_explicit_wave64_export() {
        let path = std::env::temp_dir().join(format!("aura-wave64-orchestrator-{}.w64", std::process::id()));
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.export_interleaved_buffer_to_wave64(&path, &[0.0, 0.25], 48_000, 2, true).unwrap();
        let (_, channels, samples) = read_wave64_float32(&path).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(samples, vec![0.0, 0.25]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn orchestrator_exports_connected_interleaved_buffer() {
        let path = std::env::temp_dir().join(format!(
            "aura-orchestrated-export-{}-{}.wav",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut orchestrator = ExportOrchestrator::new();
        assert!(orchestrator
            .export_interleaved_buffer_to_wav(&path, &[0.0, 0.25], 48_000, 2, true)
            .is_ok());
        assert!(orchestrator.last_error.is_none());
        assert_eq!(&fs::read(&path).unwrap()[0..4], b"RIFF");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn orchestrator_keeps_pcm_and_float_encoding_explicit() {
        let root = std::env::temp_dir().join(format!(
            "aura-orchestrated-format-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        let pcm = root.join("pcm24.wav");
        let float = root.join("float32.wav");
        assert!(orchestrator
            .export_interleaved_buffer_to_wav_with_format(
                &pcm,
                &[0.25, -0.25],
                48_000,
                1,
                24,
                false,
                true,
            )
            .is_ok());
        assert!(orchestrator
            .export_interleaved_buffer_to_wav_with_format(
                &float,
                &[0.25, -0.25],
                48_000,
                1,
                32,
                true,
                true,
            )
            .is_ok());
        let pcm_bytes = fs::read(&pcm).unwrap();
        let float_bytes = fs::read(&float).unwrap();
        assert_eq!(u16::from_le_bytes([pcm_bytes[20], pcm_bytes[21]]), 1);
        assert_eq!(u16::from_le_bytes([pcm_bytes[34], pcm_bytes[35]]), 24);
        assert_eq!(u16::from_le_bytes([float_bytes[20], float_bytes[21]]), 3);
        assert_eq!(u16::from_le_bytes([float_bytes[34], float_bytes[35]]), 32);
        assert_eq!(
            orchestrator.export_interleaved_buffer_to_wav_with_format(
                &root.join("invalid.wav"),
                &[0.0],
                48_000,
                1,
                24,
                true,
                true,
            ),
            Err(WavExportError::UnsupportedFormat)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accepts_bit_depths_supported_by_connected_writer() {
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.add_job(ExportJob {
            name: "24-bit delivery".into(),
            codec: Codec::WAV,
            bit_depth: 24,
            sample_rate: 48_000,
            normalize: false,
            lufs_target: -14.0,
        });
        assert_eq!(orchestrator.jobs.len(), 1);
        assert!(orchestrator.last_error.is_none());
    }

    #[test]
    fn selection_normalization_and_dither_are_deterministic() {
        let source = [0.25_f32, -0.5, 0.75, -1.0, 0.5, 0.0];
        assert_eq!(select_frame_range(&source, 2, 1, 2).unwrap(), vec![0.75, -1.0]);
        assert_eq!(select_frame_range(&source, 2, 2, 4), Err(WavExportError::InvalidTask));
        let normalized = prepare_export_buffer(&source, 16, true, false).unwrap();
        assert!((normalized.iter().fold(0.0_f32, |peak, value| peak.max(value.abs())) - 1.0).abs() < 0.001);
        let first = prepare_export_buffer(&source, 16, false, true).unwrap();
        let second = prepare_export_buffer(&source, 16, false, true).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn stem_export_writes_each_atomic_output_with_requested_depth() {
        let directory = std::env::temp_dir().join(format!(
            "aura-stems-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let paths = export_stems_to_wav(
            &directory,
            &[("Drums".into(), vec![0.2, -0.2]), ("Bass Bus".into(), vec![0.4, -0.4])],
            48_000,
            1,
            24,
            true,
            true,
        )
        .unwrap();
        assert_eq!(paths.len(), 2);
        for path in paths {
            let bytes = fs::read(&path).unwrap();
            assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 24);
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn aiff_writer_emits_big_endian_pcm16_and_atomic_output() {
        let path = std::env::temp_dir().join(format!("aura-aiff-{}-{}.aiff", std::process::id(), 1));
        let _ = fs::remove_file(&path);
        write_aiff_pcm16(&path, &[1.0, -1.0], 44_100, 1).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"FORM");
        assert_eq!(&bytes[8..12], b"AIFF");
        assert_eq!(&bytes[12..16], b"COMM");
        let ssnd = bytes.windows(4).position(|chunk| chunk == b"SSND").unwrap();
        let data = ssnd + 16;
        assert_eq!(&bytes[data..data + 2], &[0x7f, 0xff]);
        assert_eq!(&bytes[data + 2..data + 4], &[0x80, 0x01]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn orchestrator_exports_aiff_jobs_with_aiff_extension() {
        let directory = std::env::temp_dir().join(format!("aura-aiff-batch-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.add_job(ExportJob { name: "Vocal Mix".into(), codec: Codec::AIFF, bit_depth: 16, sample_rate: 44_100, normalize: false, lufs_target: -14.0 });
        let paths = orchestrator.execute_jobs_with_buffers(&directory, &[vec![0.0, 0.25, -0.25, 0.0]], 1).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().and_then(|value| value.to_str()), Some("aiff"));
        assert_eq!(&fs::read(&paths[0]).unwrap()[0..4], b"FORM");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn flac_export_publishes_only_after_ffmpeg_succeeds() {
        let path = std::env::temp_dir().join(format!("aura-flac-{}.flac", std::process::id()));
        let _ = fs::remove_file(&path);
        let samples = vec![0.0_f32; 4096];
        export_interleaved_buffer_to_flac(&path, &samples, 44_100, 1, true).unwrap();
        assert_eq!(&fs::read(&path).unwrap()[0..4], b"fLaC");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn orchestrator_exports_flac_jobs_when_ffmpeg_is_available() {
        let directory = std::env::temp_dir().join(format!("aura-flac-batch-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.add_job(ExportJob { name: "Master Mix".into(), codec: Codec::FLAC, bit_depth: 16, sample_rate: 44_100, normalize: false, lufs_target: -14.0 });
        let paths = orchestrator.execute_jobs_with_buffers(&directory, &[vec![0.0_f32; 4096]], 1).unwrap();
        assert_eq!(paths[0].extension().and_then(|value| value.to_str()), Some("flac"));
        assert_eq!(&fs::read(&paths[0]).unwrap()[0..4], b"fLaC");
        let _ = fs::remove_dir_all(directory);
    }
}
