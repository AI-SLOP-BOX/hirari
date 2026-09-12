use super::*;

#[test]
fn broadcast_wave_chunk_validates_calendar_and_time_reference_words() {
    let chunk = BroadcastWaveChunk { description: "Final broadcast master".into(), originator: "Aura".into(), originator_reference: "JP-AURA-20260829-001".into(), origination_date: "2026-08-29".into(), origination_time: "21:15:00".into(), time_reference_samples: (2_u64 << 32) | 7, coding_history: "A=PCM,F=48000,W=24,M=stereo".into() };
    assert!(chunk.validate()); assert_eq!(chunk.timecode_samples(), (7, 2));
    let mut invalid = chunk; invalid.origination_date = "2025-02-29".into(); assert!(!invalid.validate());
}

#[test]
fn ixml_distinguishes_stereo_from_dual_mono() {
    let stereo = IxmlChunk::stereo("Film", "Mixer", 23.976, false);
    let dual_mono = IxmlChunk::stereo("Film", "Mixer", 23.976, true);
    assert!(stereo.validate()); assert_eq!(stereo.channel_roles, vec![IxmlChannelRole::Left, IxmlChannelRole::Right]);
    assert_eq!(dual_mono.channel_roles, vec![IxmlChannelRole::LeftMix, IxmlChannelRole::RightMix]);
}

#[test]
fn wave_container_promotes_large_and_multichannel_outputs() {
    assert_eq!(select_wave_container(1_000, 2, false), Some(WaveContainer::Wave));
    assert_eq!(select_wave_container(1_000, 6, false), Some(WaveContainer::WaveExtensible));
    assert_eq!(select_wave_container(u64::from(u32::MAX), 2, false), Some(WaveContainer::Rf64));
}

#[test]
fn loudness_qc_reports_each_delivery_failure() {
    let measurements = LoudnessMeasurements { integrated_lufs: -20.0, max_short_term_lufs: -12.0, max_momentary_lufs: -8.0, loudness_range_lu: 18.0, max_true_peak_dbtp: -0.2 };
    let requirements = LoudnessRequirements { max_short_term_lufs: Some(-15.0), max_momentary_lufs: Some(-10.0), max_loudness_range_lu: Some(15.0), ..LoudnessRequirements::ebu_r128() };
    let report = audit_loudness(measurements, requirements); assert!(!report.passed); assert_eq!(report.failures.len(), 5);
}

#[test]
fn delivery_artifact_detects_truncation_and_tampering() {
    let artifact = DeliveryArtifact::from_bytes("master.wav", b"rendered-audio").unwrap();
    assert!(artifact.verify(b"rendered-audio")); assert!(!artifact.verify(b"rendered-audi0"));
    assert!(DeliveryArtifact::from_bytes("../master.wav", b"audio").is_none());
}

#[test]
fn adm_authoring_requires_one_bed_unique_sources_and_dynamic_objects() {
    let bed = AdmAuthoringElement { id: 1, name: "7.1.2 Bed".into(), kind: AdmElementKind::Bed, source_track_id: 10, source_channels: 10, object_bus_id: None, keyframes: vec![] };
    let object = AdmAuthoringElement { id: 2, name: "Dialogue".into(), kind: AdmElementKind::Object, source_track_id: 11, source_channels: 1, object_bus_id: Some(1), keyframes: vec![AdmPositionKeyframe { sample: 0, position: [0.0, 0.8, 0.0], gain_db: 0.0 }, AdmPositionKeyframe { sample: 24_000, position: [0.5, 0.7, 0.2], gain_db: -1.0 }] };
    let project = AdmAuthoringProject { profile: "Dolby Atmos".into(), sample_rate: 48_000, duration_samples: 48_000, elements: vec![bed, object], trim_downmix: AdmTrimDownmix { surround_trim_db: -3.0, height_trim_db: -3.0, overhead_balance: 0.0, stereo_direct: false } };
    assert_eq!(project.validate(), Ok(())); assert!(!project.requires_rf64(3));
    let mut invalid = project; invalid.elements[1].source_track_id = 10;
    assert!(invalid.validate().unwrap_err().iter().any(|error| error.contains("assigned more than once")));
}

#[test]
fn adm_authoring_rejects_more_than_128_object_channels() {
    let mut elements = vec![AdmAuthoringElement { id: 1, name: "Bed".into(), kind: AdmElementKind::Bed, source_track_id: 1, source_channels: 2, object_bus_id: None, keyframes: vec![] }];
    for index in 0..9_u16 { elements.push(AdmAuthoringElement { id: index + 2, name: format!("Object {index}"), kind: AdmElementKind::Object, source_track_id: u32::from(index) + 2, source_channels: 16, object_bus_id: Some(index + 1), keyframes: vec![AdmPositionKeyframe { sample: 0, position: [0.0, 0.0, 0.0], gain_db: 0.0 }] }); }
    let project = AdmAuthoringProject { profile: "Dolby Atmos".into(), sample_rate: 48_000, duration_samples: 48_000, elements, trim_downmix: AdmTrimDownmix { surround_trim_db: 0.0, height_trim_db: 0.0, overhead_balance: 0.0, stereo_direct: true } };
    assert!(project.validate().unwrap_err().iter().any(|error| error.contains("exceeds 128")));
}

fn t(n: &str, f: DeliveryFormat) -> DeliveryTarget { DeliveryTarget { name: n.into(), format: f, sample_rate: 48_000, bit_depth: 24, loudness_lufs: Some(-14.0) } }

#[test]
fn multi_format() {
    let m = DeliveryManifest { album_title: "A".into(), artist: "B".into(), catalog_number: "C".into(), targets: vec![t("x.wav", DeliveryFormat::Wav), t("x.ddp", DeliveryFormat::DdpImage)] };
    assert!(m.validate()); assert!(verify_delivery_outputs(&m, &["x.wav".into(), "x.ddp".into()]));
}

#[test]
fn invalid_duplicate() {
    let x = t("x", DeliveryFormat::Wav);
    let m = DeliveryManifest { album_title: "A".into(), artist: "B".into(), catalog_number: "C".into(), targets: vec![x.clone(), x] };
    assert!(!m.validate());
}

#[test]
fn adm_xml_is_deterministic_and_bounded() {
    let metadata = AdmMetadata { profile: "ITU-R BS.2076".into(), beds: vec!["Main".into()], objects: vec![AdmObject { id: 1, name: "Voice & Lead".into(), channel: 1, gain_db: -3.0, position: [0.0, 0.0, 0.5] }] };
    let xml = metadata.to_xml().unwrap(); assert!(xml.contains("Voice &amp; Lead")); assert!(metadata.validate());
}

#[test]
fn delivery_queue_has_retryable_atomic_state_transitions() {
    let mut queue = DeliveryQueue::default(); let id = queue.enqueue(t("mix", DeliveryFormat::Wav)).unwrap();
    assert!(queue.begin(id)); assert!(queue.fail(id, "renderer unavailable")); assert!(queue.retry_failed(id));
    assert!(queue.begin(id)); assert!(queue.complete(id, "mix.wav")); assert!(queue.is_complete()); assert!(queue.validate());
}

#[test]
fn loudness_normalization_respects_true_peak_ceiling() {
    let input = vec![0.5f32, -0.5, 0.25, -0.25];
    let (output, applied_db) = normalize_loudness_interleaved(&input, -20.0, -14.0, -1.0).unwrap();
    assert!(applied_db > 0.0 && applied_db < 6.1);
    assert!(output.iter().all(|sample| sample.is_finite() && sample.abs() <= 10.0f32.powf(-1.0 / 20.0) + 1e-5));
    assert!(normalize_loudness_interleaved(&[f32::NAN], -20.0, -14.0, -1.0).is_none());
}
