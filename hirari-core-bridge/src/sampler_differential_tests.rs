use super::{
    hirari_sampler_add_zone, hirari_sampler_create, hirari_sampler_destroy,
    hirari_sampler_note_off, hirari_sampler_note_on_buffer, hirari_sampler_note_on_zone,
    hirari_sampler_pitch_bend, hirari_sampler_process, hirari_sampler_process_midi_events,
    hirari_sampler_set_sustain,
};
use std::ffi::c_void;

unsafe extern "C" {
    fn sampler_reference_create(sample_rate: f64) -> *mut c_void;
    fn sampler_reference_destroy(state: *mut c_void);
    fn sampler_reference_note_on(
        state: *mut c_void,
        note: u8,
        velocity: u8,
        left: *const f32,
        right: *const f32,
        length: u32,
        root: u8,
        source_rate: f64,
    );
    fn sampler_reference_add_zone(
        state: *mut c_void,
        data: *const f32,
        length: u32,
        sample_rate: f64,
        root: u8,
        low_key: u8,
        high_key: u8,
        low_velocity: u8,
        high_velocity: u8,
    );
    fn sampler_reference_note_on_zone(state: *mut c_void, note: u8, velocity: u8, channel: u8);
    fn sampler_reference_note_off(state: *mut c_void, note: u8, channel: u8);
    fn sampler_reference_set_sustain(state: *mut c_void, channel: u8, held: bool);
    fn sampler_reference_pitch_bend(state: *mut c_void, channel: u8, bend: f32);
    fn sampler_reference_process(state: *mut c_void, left: *mut f32, right: *mut f32, frames: u32);
    fn sampler_reference_process_midi_events(
        state: *mut c_void,
        events: *const SamplerMidiEventFixture,
        event_count: usize,
        left: *mut f32,
        right: *mut f32,
        frames: u32,
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SamplerMidiEventFixture {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

fn midi_event(sample_offset: u64, payload: &[u8]) -> SamplerMidiEventFixture {
    let mut event = SamplerMidiEventFixture {
        sample_offset,
        size: payload.len() as u32,
        data: [0; 256],
        articulation_id: 0,
    };
    event.data[..payload.len()].copy_from_slice(payload);
    event
}

struct SampleView {
    left: Vec<f32>,
    right: Vec<f32>,
}

unsafe extern "C" fn read_sample(
    context: *mut c_void,
    left: *mut *const f32,
    right: *mut *const f32,
    frames: *mut u32,
) -> bool {
    if context.is_null() || left.is_null() || right.is_null() || frames.is_null() {
        return false;
    }
    let sample = &*(context.cast::<SampleView>());
    *left = sample.left.as_ptr();
    *right = sample.right.as_ptr();
    *frames = sample.left.len() as u32;
    true
}

fn fixture(kind: usize) -> SampleView {
    let mut left = vec![0.0; 4096];
    let mut right = vec![0.0; 4096];
    for index in 0..left.len() {
        let t = index as f32;
        match kind {
            0 => {
                if index == 0 {
                    left[index] = 1.0;
                    right[index] = -0.5;
                }
            }
            1 => {
                left[index] = 0.7 * (t * 0.071).sin();
                right[index] = 0.35 * (t * 0.043).sin();
            }
            2 => {
                let noise = (((index as u32)
                    .wrapping_mul(747_796_405)
                    .wrapping_add(2_891_336_453)
                    >> 16) as i16) as f32
                    / i16::MAX as f32;
                left[index] = noise * 0.4;
                right[index] = -noise * 0.2;
            }
            _ => {}
        }
    }
    if kind == 2 {
        left[777] = f32::NAN;
        right[1501] = f32::INFINITY;
    }
    SampleView { left, right }
}

fn assert_close(reference: &[f32], actual: &[f32], fixture: usize, block: usize, channel: &str) {
    let mut max_error = 0.0f32;
    let mut max_index = 0;
    for (index, (expected, found)) in reference.iter().zip(actual).enumerate() {
        assert!(
            found.is_finite(),
            "fixture {fixture}, block {block}, {channel}[{index}] is non-finite"
        );
        let error = (expected - found).abs();
        if error > max_error {
            max_error = error;
            max_index = index;
        }
    }
    assert!(
        max_error <= 2.0e-5,
        "fixture {fixture}, block {block}, {channel}[{max_index}] differs by {max_error}"
    );
}

#[test]
fn rust_sampler_matches_frozen_cpp_reference_across_audio_fixtures_and_blocks() {
    const RATE: f64 = 48_000.0;
    const BLOCKS: [usize; 6] = [1, 31, 127, 64, 257, 89];
    for fixture_id in 0..4 {
        let sample = fixture(fixture_id);
        let context = (&sample as *const SampleView).cast_mut().cast::<c_void>();
        let rust = hirari_sampler_create(RATE, Some(read_sample));
        let reference = unsafe { sampler_reference_create(RATE) };
        assert!(!rust.is_null() && !reference.is_null());
        unsafe {
            hirari_sampler_note_on_buffer(rust, 60, 101, context, 60, 44_100.0);
            sampler_reference_note_on(
                reference,
                60,
                101,
                sample.left.as_ptr(),
                sample.right.as_ptr(),
                sample.left.len() as u32,
                60,
                44_100.0,
            );
            hirari_sampler_note_on_buffer(rust, 67, 83, context, 60, 44_100.0);
            sampler_reference_note_on(
                reference,
                67,
                83,
                sample.left.as_ptr(),
                sample.right.as_ptr(),
                sample.left.len() as u32,
                60,
                44_100.0,
            );
        }

        for (block_index, frames) in BLOCKS.into_iter().enumerate() {
            if block_index == 2 {
                unsafe {
                    hirari_sampler_pitch_bend(rust, 0, 0.37);
                    sampler_reference_pitch_bend(reference, 0, 0.37);
                }
            }
            if block_index == 1 {
                unsafe {
                    hirari_sampler_set_sustain(rust, 0, true);
                    sampler_reference_set_sustain(reference, 0, true);
                }
            }
            if block_index == 4 {
                unsafe {
                    hirari_sampler_note_off(rust, 60, 0);
                    sampler_reference_note_off(reference, 60, 0);
                }
            }
            if block_index == 5 {
                unsafe {
                    hirari_sampler_set_sustain(rust, 0, false);
                    sampler_reference_set_sustain(reference, 0, false);
                }
            }
            let mut rust_left = vec![0.0; frames];
            let mut rust_right = vec![0.0; frames];
            let mut cpp_left = vec![0.0; frames];
            let mut cpp_right = vec![0.0; frames];
            unsafe {
                hirari_sampler_process(
                    rust,
                    rust_left.as_mut_ptr(),
                    rust_right.as_mut_ptr(),
                    frames as u32,
                );
                sampler_reference_process(
                    reference,
                    cpp_left.as_mut_ptr(),
                    cpp_right.as_mut_ptr(),
                    frames as u32,
                );
            }
            assert_close(&cpp_left, &rust_left, fixture_id, block_index, "left");
            assert_close(&cpp_right, &rust_right, fixture_id, block_index, "right");
        }
        unsafe {
            hirari_sampler_destroy(rust);
            sampler_reference_destroy(reference);
        }
    }
}

#[test]
fn rust_sampler_zone_priority_and_round_robin_match_frozen_cpp_reference() {
    const RATE: f64 = 48_000.0;
    let fallback = vec![0.1f32; 64];
    let layer = vec![0.2f32; 64];
    let round_robin_a = vec![0.2f32; 64];
    let round_robin_b = vec![0.3f32; 64];
    let rust = hirari_sampler_create(RATE, None);
    let reference = unsafe { sampler_reference_create(RATE) };
    assert!(!rust.is_null() && !reference.is_null());

    for (data, low_key, high_key, low_velocity, high_velocity) in [
        (&fallback, 40, 80, 1, 127),
        (&layer, 50, 70, 80, 127),
        (&round_robin_a, 58, 62, 80, 127),
        (&round_robin_b, 58, 62, 80, 127),
    ] {
        unsafe {
            hirari_sampler_add_zone(
                rust,
                data.as_ptr(),
                data.len() as u64,
                0.0,
                60,
                low_key,
                high_key,
                low_velocity,
                high_velocity,
                0,
                0,
                false,
            );
            sampler_reference_add_zone(
                reference,
                data.as_ptr(),
                data.len() as u32,
                0.0,
                60,
                low_key,
                high_key,
                low_velocity,
                high_velocity,
            );
        }
    }

    unsafe {
        hirari_sampler_note_on_zone(rust, 60, 100, 0);
        hirari_sampler_note_on_zone(rust, 60, 100, 0);
        sampler_reference_note_on_zone(reference, 60, 100, 0);
        sampler_reference_note_on_zone(reference, 60, 100, 0);
    }
    let mut rust_left = [0.0f32; 16];
    let mut rust_right = [0.0f32; 16];
    let mut cpp_left = [0.0f32; 16];
    let mut cpp_right = [0.0f32; 16];
    unsafe {
        hirari_sampler_process(rust, rust_left.as_mut_ptr(), rust_right.as_mut_ptr(), 16);
        sampler_reference_process(reference, cpp_left.as_mut_ptr(), cpp_right.as_mut_ptr(), 16);
    }
    assert_close(&cpp_left, &rust_left, 9, 0, "zone-left");
    assert_close(&cpp_right, &rust_right, 9, 0, "zone-right");
    assert!(
        (0.0007..0.0009).contains(&rust_left[0]),
        "velocity/key priority or round-robin did not select both narrow zones: {}",
        rust_left[0]
    );

    unsafe {
        hirari_sampler_destroy(rust);
        sampler_reference_destroy(reference);
    }
}

#[test]
fn rust_sampler_velocity_layers_and_fallback_match_frozen_cpp_reference() {
    const RATE: f64 = 48_000.0;
    let fallback = vec![0.1f32; 64];
    let high_velocity = vec![0.4f32; 64];
    for (velocity, expected_sample) in [(100u8, 0.4f32), (20u8, 0.1f32)] {
        let rust = hirari_sampler_create(RATE, None);
        let reference = unsafe { sampler_reference_create(RATE) };
        for (data, low_key, high_key, low_velocity, high_velocity) in [
            (&fallback, 40, 80, 1, 127),
            (&high_velocity, 0, 127, 96, 108),
        ] {
            unsafe {
                hirari_sampler_add_zone(
                    rust,
                    data.as_ptr(),
                    data.len() as u64,
                    0.0,
                    60,
                    low_key,
                    high_key,
                    low_velocity,
                    high_velocity,
                    0,
                    0,
                    false,
                );
                sampler_reference_add_zone(
                    reference,
                    data.as_ptr(),
                    data.len() as u32,
                    0.0,
                    60,
                    low_key,
                    high_key,
                    low_velocity,
                    high_velocity,
                );
            }
        }
        unsafe {
            hirari_sampler_note_on_zone(rust, 60, velocity, 0);
            sampler_reference_note_on_zone(reference, 60, velocity, 0);
        }
        let mut rust_left = [0.0f32; 1];
        let mut rust_right = [0.0f32; 1];
        let mut cpp_left = [0.0f32; 1];
        let mut cpp_right = [0.0f32; 1];
        unsafe {
            hirari_sampler_process(rust, rust_left.as_mut_ptr(), rust_right.as_mut_ptr(), 1);
            sampler_reference_process(reference, cpp_left.as_mut_ptr(), cpp_right.as_mut_ptr(), 1);
        }
        assert_close(
            &cpp_left,
            &rust_left,
            10,
            velocity as usize,
            "velocity-layer",
        );
        let expected = 0.002 * (velocity as f32 / 127.0) * expected_sample;
        assert!(
            (rust_left[0] - expected).abs() <= 1.0e-6,
            "velocity {velocity} selected the wrong zone: got {}, expected {expected}",
            rust_left[0]
        );
        unsafe {
            hirari_sampler_destroy(rust);
            sampler_reference_destroy(reference);
        }
    }
}

#[test]
fn rust_sampler_midi_1_and_2_dispatch_matches_frozen_cpp_adapter() {
    const RATE: f64 = 48_000.0;
    const FRAMES: usize = 96;
    let mut sample = vec![0.0f32; 8_192];
    for (index, value) in sample.iter_mut().enumerate() {
        *value = 0.55 * (index as f32 * 0.083).sin();
    }
    let rust = hirari_sampler_create(RATE, None);
    let reference = unsafe { sampler_reference_create(RATE) };
    assert!(!rust.is_null() && !reference.is_null());
    unsafe {
        hirari_sampler_add_zone(
            rust,
            sample.as_ptr(),
            sample.len() as u64,
            RATE,
            60,
            0,
            127,
            1,
            127,
            0,
            0,
            false,
        );
        sampler_reference_add_zone(
            reference,
            sample.as_ptr(),
            sample.len() as u32,
            RATE,
            60,
            0,
            127,
            1,
            127,
        );
    }

    let blocks = [
        vec![midi_event(0, &[0x90, 60, 111])],
        vec![
            midi_event(12, &[0xb0, 64, 127]),
            midi_event(48, &[0x80, 60, 0]),
        ],
        vec![midi_event(0, &[0xe0, 0x00, 0x60])],
        vec![midi_event(71, &[0xb0, 64, 0])],
        vec![midi_event(
            0,
            &[0x40, 0x91, 67, 0, 96, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        )],
        vec![midi_event(
            32,
            &[0x40, 0x81, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        )],
    ];
    assert_eq!(std::mem::size_of::<SamplerMidiEventFixture>(), 272);
    for (block_index, events) in blocks.iter().enumerate() {
        let mut rust_left = [0.0f32; FRAMES];
        let mut rust_right = [0.0f32; FRAMES];
        let mut cpp_left = [0.0f32; FRAMES];
        let mut cpp_right = [0.0f32; FRAMES];
        unsafe {
            hirari_sampler_process_midi_events(
                rust,
                events.as_ptr().cast(),
                events.len(),
                rust_left.as_mut_ptr(),
                rust_right.as_mut_ptr(),
                FRAMES as u32,
            );
            sampler_reference_process_midi_events(
                reference,
                events.as_ptr(),
                events.len(),
                cpp_left.as_mut_ptr(),
                cpp_right.as_mut_ptr(),
                FRAMES as u32,
            );
        }
        assert_close(&cpp_left, &rust_left, 11, block_index, "midi-left");
        assert_close(&cpp_right, &rust_right, 11, block_index, "midi-right");
    }
    unsafe {
        hirari_sampler_destroy(rust);
        sampler_reference_destroy(reference);
    }
}
