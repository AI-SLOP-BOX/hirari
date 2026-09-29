use hirari_core_bridge::bus_audio::{
    hirari_bus_audio_accumulate, hirari_bus_audio_create, hirari_bus_audio_destroy,
    hirari_bus_audio_replace_post,
};
use hirari_core_bridge::bus_track::{
    hirari_bus_track_create, hirari_bus_track_destroy, hirari_bus_track_fetch_audio,
    hirari_bus_track_set_input_gain, hirari_bus_track_set_phase_inverted,
    hirari_bus_track_set_read_post_fx,
};

#[test]
fn bus_track_rust_state_selects_stage_and_applies_gain_and_polarity() {
    let bus = hirari_bus_audio_create();
    let track = hirari_bus_track_create();
    let pre_left = [1.0_f32, 2.0];
    let pre_right = [-1.0_f32, -2.0];
    let post_left = [3.0_f32, 4.0];
    let post_right = [5.0_f32, 6.0];
    assert!(unsafe {
        hirari_bus_audio_accumulate(bus, pre_left.as_ptr(), pre_right.as_ptr(), 2, 1.0)
    });
    assert!(unsafe {
        hirari_bus_audio_replace_post(bus, post_left.as_ptr(), post_right.as_ptr(), 2)
    });

    unsafe {
        hirari_bus_track_set_input_gain(track, 2.0);
        hirari_bus_track_set_phase_inverted(track, true);
    }
    let mut left = [0.0; 2];
    let mut right = [0.0; 2];
    assert!(unsafe {
        hirari_bus_track_fetch_audio(track, bus, left.as_mut_ptr(), right.as_mut_ptr(), 2)
    });
    assert_eq!(left, [-6.0, -8.0]);
    assert_eq!(right, [-10.0, -12.0]);

    unsafe { hirari_bus_track_set_read_post_fx(track, false) };
    assert!(unsafe {
        hirari_bus_track_fetch_audio(track, bus, left.as_mut_ptr(), right.as_mut_ptr(), 2)
    });
    assert_eq!(left, [-2.0, -4.0]);
    assert_eq!(right, [2.0, 4.0]);

    unsafe {
        hirari_bus_track_set_input_gain(track, f32::NAN);
        hirari_bus_track_set_phase_inverted(track, false);
    }
    assert!(unsafe {
        hirari_bus_track_fetch_audio(track, bus, left.as_mut_ptr(), right.as_mut_ptr(), 2)
    });
    assert_eq!(left, pre_left);
    assert_eq!(right, pre_right);

    unsafe {
        hirari_bus_track_destroy(track);
        hirari_bus_audio_destroy(bus);
    }
}
