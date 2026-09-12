    use super::*;
    use std::io::Write;

    #[test]
    fn registers_decodes_and_assigns_pcm16_wav() {
        let path = std::env::temp_dir().join(format!("aura-preview-{}.wav", std::process::id()));
        let samples = [0i16, 16384i16, -16384i16];
        let data_len = samples.len() * 2;
        let riff_len = 36 + data_len;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&88200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&bytes).unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        assert_eq!(runtime.register_file(&path).unwrap(), id);
        assert_eq!(runtime.asset_count(), 1);
        assert!(runtime.assign_pad(3, Some(id)));
        assert_eq!(runtime.pad_asset(3), Some(id));
        assert_eq!(runtime.pad_samples(3).unwrap().len(), 3);
        assert_eq!(runtime.asset_audio(id).unwrap().1, 44100.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preserves_stereo_96khz_metadata_on_unicode_path() {
        let directory = std::env::temp_dir().join(format!(
            "aura-preview-unicode-{}-音声",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ステレオ素材.wav");
        let interleaved = [16_384i16, -16_384i16, 8_192i16, -8_192i16];
        let data_len = interleaved.len() * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&(96_000u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in interleaved {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        let catalog = runtime.catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].sample_rate, 96_000.0);
        assert_eq!(catalog[0].frames, 2);
        assert_eq!(runtime.assets.get(&id).unwrap().channels, 2);
        let (decoded, sample_rate) = runtime.asset_audio(id).unwrap();
        assert_eq!(sample_rate, 96_000.0);
        assert_eq!(decoded.len(), 2);
        assert!(decoded.iter().all(|sample| sample.abs() < 1.0e-6));

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn decodes_float32_wav_samples() {
        let path =
            std::env::temp_dir().join(format!("aura-preview-float-{}.wav", std::process::id()));
        let samples = [0.25f32, -0.5f32];
        let data_len = samples.len() * 4;
        let riff_len = 36 + data_len;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48000u32.to_le_bytes());
        bytes.extend_from_slice(&192000u32.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        let (decoded, rate) = runtime.asset_audio(id).unwrap();
        assert_eq!(rate, 48000.0);
        assert!((decoded[0] - 0.25).abs() < 1.0e-6);
        assert!((decoded[1] + 0.5).abs() < 1.0e-6);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decodes_rf64_pcm16_using_ds64_data_size() {
        let path = std::env::temp_dir().join(format!(
            "aura-preview-rf64-{}-{}.wav",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let samples = [0i16, 16384i16];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RF64");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"ds64");
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&76u64.to_le_bytes());
        bytes.extend_from_slice(&4u64.to_le_bytes());
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48000u32.to_le_bytes());
        bytes.extend_from_slice(&96000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        let (decoded, rate) = runtime.asset_audio(id).unwrap();
        assert_eq!(rate, 48000.0);
        assert_eq!(decoded.len(), 2);
        assert!((decoded[1] - 0.5).abs() < 1.0e-3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decodes_pcm24_extremes() {
        let path =
            std::env::temp_dir().join(format!("aura-preview-pcm24-{}.wav", std::process::id()));
        let raw_samples = [0x7f_ffffu32, 0x80_0000u32];
        let data_len = raw_samples.len() * 3;
        let riff_len = 36 + data_len;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&132300u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for raw in raw_samples {
            bytes.push((raw & 0xff) as u8);
            bytes.push(((raw >> 8) & 0xff) as u8);
            bytes.push(((raw >> 16) & 0xff) as u8);
        }
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        let (decoded, _) = runtime.asset_audio(id).unwrap();
        assert!((decoded[0] - 0.9999999).abs() < 1.0e-5);
        assert!((decoded[1] + 1.0).abs() < 1.0e-6);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decodes_pcm32_extremes() {
        let path =
            std::env::temp_dir().join(format!("aura-preview-pcm32-{}.wav", std::process::id()));
        let samples = [0i32, 1_073_741_824i32, i32::MIN, i32::MAX];
        let data_len = samples.len() * 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&88_200u32.to_le_bytes());
        bytes.extend_from_slice(&(88_200u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        let id = runtime.register_file(&path).unwrap();
        let (decoded, rate) = runtime.asset_audio(id).unwrap();
        assert_eq!(rate, 88_200.0);
        assert_eq!(decoded.len(), 4);
        assert!(decoded[0].abs() < 1.0e-7);
        assert!((decoded[1] - 0.5).abs() < 1.0e-7);
        assert!((decoded[2] + 1.0).abs() < 1.0e-7);
        assert!((decoded[3] - 1.0).abs() < 1.0e-6);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_partial_pcm_frame() {
        let path =
            std::env::temp_dir().join(format!("aura-preview-partial-{}.wav", std::process::id()));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&37u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&88200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(0);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        assert!(runtime.register_file(&path).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn library_scan_skips_corrupt_and_empty_wavs() {
        let directory = std::env::temp_dir().join(format!(
            "aura-preview-scan-invalid-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("empty.wav"), []).unwrap();
        std::fs::write(directory.join("corrupt.wav"), b"not a wave file").unwrap();

        let mut runtime = PreviewAudioRuntime::new();
        assert_eq!(runtime.scan(&directory).unwrap(), 0);
        assert!(runtime.catalog().is_empty());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preview_can_follow_project_tempo_without_mutating_source() {
        let path = std::env::temp_dir().join(format!("aura-preview-tempo-{}.wav", std::process::id()));
        let samples = [0i16, 8_192i16, 16_384i16, 24_576i16];
        let data_len = samples.len() * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&(36u32 + data_len as u32).to_le_bytes()); bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&1u16.to_le_bytes()); bytes.extend_from_slice(&48_000u32.to_le_bytes()); bytes.extend_from_slice(&96_000u32.to_le_bytes()); bytes.extend_from_slice(&2u16.to_le_bytes()); bytes.extend_from_slice(&16u16.to_le_bytes()); bytes.extend_from_slice(b"data"); bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples { bytes.extend_from_slice(&sample.to_le_bytes()); }
        std::fs::write(&path, bytes).unwrap();
        let mut runtime = PreviewAudioRuntime::new(); let id = runtime.register_file(&path).unwrap();
        let synced = runtime.tempo_synced_audio(id, 120.0, 60.0).unwrap();
        assert_eq!(synced.len(), 8); assert_eq!(runtime.asset_samples(id).unwrap().len(), 4);
        std::fs::remove_file(path).unwrap();
    }
