use hirari_core_bridge::engine_edit_ext::TrackEditExt;
use hirari_core_bridge::ffi;

mod support {
    include!("recording_support.rs");
}
use support::*;

const ERROR_NONE: u8 = 0;
const ERROR_INVALID_TARGET: u8 = 1;
const ERROR_STALE_PROJECT: u8 = 2;
const ERROR_STALE_AUDIO: u8 = 3;
const ERROR_QUEUE_FULL: u8 = 4;

#[test]
fn command_admission_reports_target_and_generation_failures_in_order() {
    let _guard = native_engine_test_guard();
    let engine = ffi::new_audio_engine_offline();
    let engine = engine.as_ref().expect("offline engine must be created");
    let track = engine.add_track(0);
    assert_ne!(track, 0);
    let project = engine.get_project_generation();
    let audio = engine.get_audio_config_generation();

    assert!(ffi::push_command(
        engine,
        ffi::HirariCommand::SetVolume,
        track,
        0.5,
        0,
        project,
        audio
    ));
    assert_eq!(engine.get_last_command_error(), ERROR_NONE);

    assert!(!ffi::push_command(
        engine,
        ffi::HirariCommand::SetVolume,
        900,
        0.5,
        0,
        project,
        audio
    ));
    assert_eq!(engine.get_last_command_error(), ERROR_INVALID_TARGET);

    assert!(!ffi::push_command(
        engine,
        ffi::HirariCommand::SetVolume,
        track,
        0.5,
        0,
        project + 1,
        audio
    ));
    assert_eq!(engine.get_last_command_error(), ERROR_STALE_PROJECT);

    assert!(!ffi::push_command(
        engine,
        ffi::HirariCommand::SetVolume,
        track,
        0.5,
        0,
        project,
        audio + 1
    ));
    assert_eq!(engine.get_last_command_error(), ERROR_STALE_AUDIO);
}

#[test]
fn a_full_command_queue_is_reported_as_a_failure() {
    let _guard = native_engine_test_guard();
    let engine = ffi::new_audio_engine_offline();
    let engine = engine.as_ref().expect("offline engine must be created");
    let track = engine.add_track(0);
    let project = engine.get_project_generation();
    let audio = engine.get_audio_config_generation();

    let mut accepted = 0u32;
    for _ in 0..4_096 {
        if ffi::push_command(
            engine,
            ffi::HirariCommand::SetPan,
            track,
            0.25,
            0,
            project,
            audio,
        ) {
            accepted += 1;
        } else {
            break;
        }
    }
    assert!(accepted > 0);
    assert_eq!(engine.get_last_command_error(), ERROR_QUEUE_FULL);
}

#[test]
fn spatial_and_hrtf_validation_runs_before_the_track_lookup() {
    let _guard = native_engine_test_guard();
    let engine = ffi::new_audio_engine_offline();
    let engine = engine.as_ref().expect("offline engine must be created");
    let track = engine.add_track(0);

    assert!(!engine.set_spatial_position(track, f32::NAN, 0.0, 0.0));
    assert!(engine.set_spatial_position(track, 0.25, 0.0, -0.5));
    assert!(!engine.set_spatial_position(900, 0.25, 0.0, -0.5));

    assert!(!engine.set_hrtf_kernel(track, Vec::new(), Vec::new()));
    assert!(!engine.set_hrtf_kernel(track, vec![0.5, 0.25], vec![0.5]));
    assert!(!engine.set_hrtf_kernel(track, vec![0.1; 129], vec![0.1; 129]));
    assert!(engine.set_hrtf_kernel(track, vec![0.5, 0.25], vec![0.75, 0.125]));
    assert!(engine.clear_hrtf_kernel(track));
    assert!(!engine.clear_hrtf_kernel(900));
}
