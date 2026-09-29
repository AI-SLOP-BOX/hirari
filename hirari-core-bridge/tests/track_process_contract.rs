use hirari_core_bridge::track_process::{hirari_track_process_prepare, TrackProcessRequest};
use std::ffi::c_void;

struct MidiStage {
    left: *mut f32,
    right: *mut f32,
    calls: usize,
}

unsafe extern "C" fn prepare_midi(context: *mut c_void, _playhead: u64, frames: u32) {
    let stage = unsafe { &mut *context.cast::<MidiStage>() };
    stage.calls += 1;
    for frame in 0..frames as usize {
        unsafe {
            *stage.left.add(frame) = 0.25;
            *stage.right.add(frame) = -0.5;
        }
    }
}

#[test]
fn midi_preparation_runs_before_the_pre_insert_snapshot() {
    let mut left = [0.0_f32; 4];
    let mut right = [0.0_f32; 4];
    let mut pre_insert_left = [9.0_f32; 4];
    let mut pre_insert_right = [9.0_f32; 4];
    let channels = [left.as_mut_ptr(), right.as_mut_ptr()];
    let pre_insert = [pre_insert_left.as_mut_ptr(), pre_insert_right.as_mut_ptr()];
    let mut stage = MidiStage {
        left: left.as_mut_ptr(),
        right: right.as_mut_ptr(),
        calls: 0,
    };
    let mut event_count = usize::MAX;
    let mut request: TrackProcessRequest = unsafe { std::mem::zeroed() };
    request.channels = channels.as_ptr();
    request.channel_count = 2;
    request.buffer_capacity = 4;
    request.frames = 4;
    request.region_snapshot_available = 1;
    request.plugin_event_count_out = &mut event_count;
    request.pre_insert_channels = pre_insert.as_ptr();
    request.pre_insert_channel_count = 2;
    request.pre_insert_capacity = 4;
    request.midi_prepare = Some(prepare_midi);
    request.midi_context = (&mut stage as *mut MidiStage).cast();

    assert!(unsafe { hirari_track_process_prepare(&request) });
    assert_eq!(stage.calls, 1);
    assert_eq!(event_count, 0);
    assert_eq!(pre_insert_left, [0.25; 4]);
    assert_eq!(pre_insert_right, [-0.5; 4]);
}

#[test]
fn muted_track_clears_audio_and_skips_midi_preparation() {
    let mut left = [1.0_f32; 4];
    let mut right = [-1.0_f32; 4];
    let channels = [left.as_mut_ptr(), right.as_mut_ptr()];
    let mut stage = MidiStage {
        left: left.as_mut_ptr(),
        right: right.as_mut_ptr(),
        calls: 0,
    };
    let mut event_count = usize::MAX;
    let mut request: TrackProcessRequest = unsafe { std::mem::zeroed() };
    request.channels = channels.as_ptr();
    request.channel_count = 2;
    request.buffer_capacity = 4;
    request.frames = 4;
    request.muted = 1;
    request.plugin_event_count_out = &mut event_count;
    request.midi_prepare = Some(prepare_midi);
    request.midi_context = (&mut stage as *mut MidiStage).cast();

    assert!(!unsafe { hirari_track_process_prepare(&request) });
    assert_eq!(left, [0.0; 4]);
    assert_eq!(right, [0.0; 4]);
    assert_eq!(stage.calls, 0);
    assert_eq!(event_count, 0);
}
