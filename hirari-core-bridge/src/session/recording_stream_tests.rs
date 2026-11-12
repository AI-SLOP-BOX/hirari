//! Tests split out of recording_stream.rs.

    use super::{
        recording_wav_metadata, recoverable_spools, write_header, RecordingStreamError,
        StreamingRecordingWriter,
    };
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hirari-stream-{}-{}.wav",
            std::process::id(),
            TEST_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn unique_test_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hirari-stream-{}-{}-{suffix}.wav",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn streams_blocks_and_publishes_a_valid_wav() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.0, 0.5, -0.5, 1.0]).unwrap();
        assert_eq!(writer.frames(), 2);
        let published = writer.finalize().unwrap();
        let bytes = std::fs::read(&published).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[72..76], b"data");
        assert_eq!(u32::from_le_bytes(bytes[76..80].try_into().unwrap()), 8);
        assert!(!path.with_extension("wav.part").exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn finalize_never_replaces_an_existing_take() {
        let path = unique_test_path("no-replace");
        std::fs::write(&path, b"newer take").unwrap();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 1).unwrap();
        writer.append_interleaved(&[0.25]).unwrap();
        let error = writer.finalize().unwrap_err();
        assert!(
            matches!(error, RecordingStreamError::Io(message) if message.contains("already exists"))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"newer take");
        // A failed publication keeps the spool available for retry/recovery.
        let spool = path.with_extension("wav.part");
        assert!(spool.exists());
        let _ = std::fs::remove_file(spool);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn metadata_parser_accepts_standard_riff_and_reserved_rf64_headers() {
        let riff_path = temp_path();
        let mut riff = StreamingRecordingWriter::create(&riff_path, 48_000, 2).unwrap();
        riff.append_interleaved(&[0.1, -0.1]).unwrap();
        let riff_path = riff.finalize().unwrap();
        let riff_bytes = std::fs::read(&riff_path).unwrap();
        assert_eq!(recording_wav_metadata(&riff_bytes), Some((2, 4)));
        let _ = std::fs::remove_file(riff_path);

        let rf64_path = unique_test_path("rf64");
        let mut rf64_file = std::fs::File::create(&rf64_path).unwrap();
        write_header(&mut rf64_file, 48_000, 2, 4).unwrap();
        rf64_file.write_all(&[0, 0, 0, 0]).unwrap();
        rf64_file.sync_all().unwrap();
        let mut rf64_bytes = std::fs::read(&rf64_path).unwrap();
        rf64_bytes[0..4].copy_from_slice(b"RF64");
        rf64_bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        rf64_bytes[12..16].copy_from_slice(b"ds64");
        rf64_bytes[20..28].copy_from_slice(&u64::from(80u32).to_le_bytes());
        rf64_bytes[28..36].copy_from_slice(&4u64.to_le_bytes());
        rf64_bytes[76..80].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&rf64_path, &rf64_bytes).unwrap();
        assert_eq!(recording_wav_metadata(&rf64_bytes), Some((2, 4)));
        let _ = std::fs::remove_file(rf64_path);
    }

    #[test]
    fn metadata_parser_rejects_truncation_and_header_mutations_without_panicking() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.1, -0.1, 0.2, -0.2]).unwrap();
        let published = writer.finalize().unwrap();
        let valid = std::fs::read(&published).unwrap();

        for end in 0..valid.len() {
            assert!(recording_wav_metadata(&valid[..end]).is_none());
        }
        for offset in [0usize, 4, 12, 20, 28, 36, 40, 76] {
            if offset < valid.len() {
                let mut mutated = valid.clone();
                mutated[offset] ^= 0xff;
                let _ = recording_wav_metadata(&mutated);
            }
        }
        let _ = std::fs::remove_file(published);
    }

    #[test]
    fn metadata_parser_handles_unknown_odd_chunks_and_rejects_missing_padding() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.1, -0.1]).unwrap();
        let published = writer.finalize().unwrap();
        let valid = std::fs::read(&published).unwrap();

        let mut with_unknown = valid[..12].to_vec();
        with_unknown.extend_from_slice(b"JUNK");
        with_unknown.extend_from_slice(&1u32.to_le_bytes());
        with_unknown.push(0x7f);
        with_unknown.push(0); // RIFF padding for the odd-sized payload.
        with_unknown.extend_from_slice(&valid[12..]);
        assert_eq!(recording_wav_metadata(&with_unknown), Some((2, 4)));

        let mut missing_padding = valid[..12].to_vec();
        missing_padding.extend_from_slice(b"JUNK");
        missing_padding.extend_from_slice(&1u32.to_le_bytes());
        missing_padding.push(0x7f);
        missing_padding.extend_from_slice(&valid[12..]);
        assert!(recording_wav_metadata(&missing_padding).is_none());

        let _ = std::fs::remove_file(published);
    }

    #[test]
    fn metadata_parser_fuzz_corpus_never_panics() {
        // Deterministic mutation corpus: this exercises truncation, random
        // chunk lengths, odd alignment, and bogus RF64 markers without
        // requiring an external fuzzing tool in every developer checkout.
        let mut state = 0x9e37_79b9_u32;
        for length in 0..4096usize {
            let mut bytes = vec![0u8; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let _ = recording_wav_metadata(&bytes);
        }
    }
    #[test]
    fn finalize_does_not_overwrite_an_existing_destination() {
        let path = temp_path();
        std::fs::write(&path, b"newer-take").unwrap();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 1).unwrap();
        writer.append_interleaved(&[0.25]).unwrap();
        assert!(matches!(
            writer.finalize(),
            Err(RecordingStreamError::Io(message)) if message.contains("already exists")
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"newer-take");
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn rejects_bad_blocks_without_writing_partial_audio() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        assert_eq!(
            writer.append_interleaved(&[0.0]),
            Err(RecordingStreamError::InvalidBlock)
        );
        assert_eq!(
            writer.append_interleaved(&[f32::NAN, 0.0]),
            Err(RecordingStreamError::NonFiniteSample)
        );
        assert_eq!(writer.frames(), 0);
        drop(writer);
        assert!(!path.with_extension("wav.part").exists());
    }
    #[test]
    fn crash_surviving_spools_are_detected_and_invalid_files_are_ignored() {
        let path = std::env::temp_dir().join(format!(
            "hirari-capture-test-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let part = path.with_extension("wav.part");
        let _ = std::fs::remove_file(&part);
        let mut file = std::fs::File::create(&part).unwrap();
        write_header(&mut file, 48_000, 1, 2).unwrap();
        file.write_all(&[0x7f, 0x00]).unwrap();
        file.sync_all().unwrap();
        let invalid = part.with_file_name("hirari-capture-invalid.wav.part");
        std::fs::write(&invalid, b"partial").unwrap();

        let candidates = recoverable_spools(std::env::temp_dir());
        assert!(candidates.iter().any(|candidate| candidate == &part));
        assert!(!candidates.iter().any(|candidate| candidate == &invalid));
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(invalid);
    }
