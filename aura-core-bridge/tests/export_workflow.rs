use aura_core_bridge::export::{
    export_stems_to_wav, prepare_export_buffer, select_frame_range, write_wav_float32,
    write_wav_pcm,
};

#[test]
fn selection_normalization_and_dither_feed_a_real_export() {
    let samples = [0.25_f32, -0.5, 0.75, -1.0, 0.5, 0.0];
    let selected = select_frame_range(&samples, 2, 1, 3).expect("selection must be valid");
    assert_eq!(selected, vec![0.75, -1.0, 0.5, 0.0]);

    let normalized = prepare_export_buffer(&selected, 24, true, false)
        .expect("normalization must produce an export buffer");
    let peak = normalized
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!((peak - 1.0).abs() < 0.0001);

    let dithered_a = prepare_export_buffer(&selected, 16, false, true)
        .expect("dither must produce an export buffer");
    let dithered_b =
        prepare_export_buffer(&selected, 16, false, true).expect("dither must be repeatable");
    assert_eq!(dithered_a, dithered_b);

    let root = std::env::temp_dir().join(format!(
        "aura-export-workflow-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("export directory must be creatable");
    let output = root.join("selection-24bit.wav");
    write_wav_pcm(&output, &normalized, 96_000, 2, 24).expect("24-bit WAV export must succeed");
    let bytes = std::fs::read(&output).expect("export must exist");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        96_000
    );
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 24);
    assert_eq!(bytes.len(), 44 + selected.len() * 3);
    std::fs::remove_dir_all(root).expect("export cleanup must succeed");
}

#[test]
fn float32_wav_export_uses_ieee_float_header_and_payload() {
    let root = std::env::temp_dir().join(format!(
        "aura-float-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("export directory must be creatable");
    let output = root.join("float32.wav");
    write_wav_float32(&output, &[0.25, -0.5], 48_000, 1).expect("float32 export must succeed");
    let bytes = std::fs::read(&output).expect("float32 output must exist");
    assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
    assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 32);
    assert_eq!(
        f32::from_le_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]),
        0.25
    );
    assert_eq!(
        f32::from_le_bytes([bytes[48], bytes[49], bytes[50], bytes[51]]),
        -0.5
    );
    std::fs::remove_dir_all(root).expect("float32 cleanup must succeed");
}

#[test]
fn stem_export_sanitizes_names_and_publishes_each_stem() {
    let root = std::env::temp_dir().join(format!(
        "aura-stems-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("stem directory must be creatable");
    let paths = export_stems_to_wav(
        &root,
        &[
            ("Lead Vocal/Comp".to_owned(), vec![0.2, -0.2]),
            ("Lead_Vocal_Comp".to_owned(), vec![0.4, -0.4]),
        ],
        48_000,
        1,
        16,
        true,
        true,
    )
    .expect("stem export must succeed");
    assert_eq!(paths.len(), 2);
    assert!(root.join("Lead_Vocal_Comp.wav").is_file());
    assert!(root.join("Lead_Vocal_Comp_2.wav").is_file());
    std::fs::remove_dir_all(root).expect("stem cleanup must succeed");
}
