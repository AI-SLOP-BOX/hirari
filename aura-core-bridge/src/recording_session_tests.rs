use super::{
    RecordingLifecycle, RecordingSession, RecordingSessionError, RecordingSessionState,
    NEXT_CAPTURE_ID,
};
use std::sync::atomic::Ordering;

#[test]
fn lifecycle_requires_a_real_recording_session() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    assert_eq!(session.state(), RecordingSessionState::Idle);
    assert_eq!(session.lifecycle(), RecordingLifecycle::Idle);
    assert!(session.arm());
    assert_eq!(session.lifecycle(), RecordingLifecycle::Armed);
    assert_eq!(session.stop(), Err(RecordingSessionError::NotRecording));
    session.start(512).unwrap();
    assert_eq!(session.state(), RecordingSessionState::Recording);
    assert_eq!(session.lifecycle(), RecordingLifecycle::Recording);
    assert_eq!(session.start(513), Err(RecordingSessionError::AlreadyRecording));
    session.append_interleaved(&[0.0, 0.0]).unwrap();
    let region = session.stop().unwrap();
    assert_eq!(region.start_sample, 512);
    assert_eq!(session.state(), RecordingSessionState::Stopped);
    assert_eq!(session.lifecycle(), RecordingLifecycle::Stopped);
    assert!(session.mark_committed());
    assert_eq!(session.lifecycle(), RecordingLifecycle::Committed);
    assert!(!session.mark_committed());
    assert!(session.audit());
}

#[test]
fn published_take_path_can_be_rebased_without_breaking_audit() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    session.start(0).unwrap();
    session.append_interleaved(&[0.25, -0.25]).unwrap();
    let original = session.stop().unwrap();
    let old_path = session.last_spool_path().unwrap().to_path_buf();
    assert!(old_path.is_file());
    let published = std::env::temp_dir().join(format!(
        "aura-recording-published-{}-{}.wav",
        std::process::id(),
        NEXT_CAPTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::rename(&old_path, &published).unwrap();
    assert!(session.rebase_last_spool_path(published.clone()));
    assert_eq!(session.last_spool_path(), Some(published.as_path()));
    assert_eq!(session.last_region(), Some(&original));
    assert!(session.audit());
    let _ = std::fs::remove_file(published);
}

#[test]
fn disk_capture_continues_after_bounded_preview_is_full() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 1).unwrap();
    session.start(0).unwrap();
    session.append_interleaved(&[0.1, -0.1]).unwrap();
    session.append_interleaved(&[0.2, -0.2]).unwrap();
    let region = session.stop().unwrap();
    assert_eq!(session.captured_frame_count(), 2);
    assert_eq!(region.frame_count(), 1);
    assert!(session.last_spool_path().unwrap().is_file());
    let _ = std::fs::remove_file(session.last_spool_path().unwrap());
}

#[test]
fn invalid_audio_is_rejected_without_state_corruption() {
    let mut session = RecordingSession::try_new(44_100.0, 2, 2).unwrap();
    session.start(0).unwrap();
    let result = session.append_interleaved(&[f32::NAN, 0.0]);
    assert!(matches!(result, Err(RecordingSessionError::Preview(_))));
    assert_eq!(session.state(), RecordingSessionState::Recording);
    assert!(session.audit());
}

#[test]
fn completed_takes_are_kept_and_can_be_selected() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    session.start(0).unwrap();
    session.append_interleaved(&[0.1, 0.1]).unwrap();
    session.stop().unwrap();
    session.start(16).unwrap();
    session.append_interleaved(&[0.2, 0.2]).unwrap();
    session.stop().unwrap();
    assert_eq!(session.take_count(), 2);
    assert_eq!(session.active_take(), 1);
    assert!(session.select_take(0));
    assert_eq!(session.active_take(), 0);
    assert!(!session.select_take(9));
}

#[test]
fn empty_recording_is_rejected_before_commit() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    session.start(0).unwrap();
    assert_eq!(session.stop(), Err(RecordingSessionError::EmptyRecording));
    assert_eq!(session.state(), RecordingSessionState::Idle);
    assert_eq!(session.take_count(), 0);
    assert!(session.audit());
}

#[test]
fn configuration_changes_are_not_silently_reused() {
    let session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    assert!(session.configuration_matches(48_000.0, 2, 8));
    assert!(!session.configuration_matches(44_100.0, 2, 8));
    assert!(!session.configuration_matches(48_000.0, 1, 8));
    assert!(!session.configuration_matches(48_000.0, 2, 16));
}

#[test]
fn take_history_is_bounded_without_invalidating_active_take() {
    let mut session = RecordingSession::try_new(48_000.0, 1, 2).unwrap();
    for take in 0..20u32 {
        session.start(u64::from(take) * 2).unwrap();
        session.append_interleaved(&[0.1 + take as f32 / 100.0]).unwrap();
        session.stop().unwrap();
    }
    assert_eq!(session.take_count(), 16);
    assert!(session.active_take() < session.take_count());
    assert!(session.audit());
    assert!(session.select_take(0));
    assert_eq!(session.active_take(), 0);
    assert!(session.audit());
}

#[test]
fn waveform_snapshot_is_bounded_and_tracks_peak_values() {
    let mut session = RecordingSession::try_new(48_000.0, 2, 8).unwrap();
    session.start(0).unwrap();
    session
        .append_interleaved(&[0.1, -0.2, 0.7, 0.3, -0.4, 0.9])
        .unwrap();
    let points = session.waveform_points(32);
    assert_eq!(points.len(), 3);
    assert!((points[0] - 0.2).abs() < f32::EPSILON);
    assert!((points[1] - 0.7).abs() < f32::EPSILON);
    assert!((points[2] - 0.9).abs() < f32::EPSILON);
    assert!(points.iter().all(|point| (0.0..=1.0).contains(point)));
    session.stop().unwrap();
    assert_eq!(session.waveform_points(0), Vec::<f32>::new());
}
