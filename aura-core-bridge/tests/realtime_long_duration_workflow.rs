//! Bounded long-duration callback soak used by the release gate.
use aura_core_bridge::AuraCore;
use std::time::{Duration, Instant};

mod support {
    include!("recording_support.rs");
}
use support::native_engine_test_guard;

#[test]
#[ignore = "release-mode long-duration realtime soak"]
fn callback_soak_has_no_nonfinite_output_or_deadline_misses() {
    let _guard = native_engine_test_guard();
    let seconds = std::env::var("AURA_REALTIME_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
        .max(1);
    let frames = 256usize;
    let deadline = Duration::from_micros((frames as u64 * 1_000_000) / 48_000);
    let core = AuraCore::new().expect("core must initialize");
    let track = core.add_track(0);
    assert!(core.apply_audio_config(48_000, frames as u32));
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let stop = Instant::now() + Duration::from_secs(seconds);
    let mut blocks = 0u64;
    let mut misses = 0u64;
    let mut maximum = Duration::ZERO;
    while Instant::now() < stop {
        let started = Instant::now();
        assert!(core.process_audio_block(&mut left, &mut right));
        let elapsed = started.elapsed();
        maximum = maximum.max(elapsed);
        misses += u64::from(elapsed > deadline);
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        blocks += 1;
        if blocks.is_multiple_of(256) {
            assert!(core.set_volume(track, 0.75));
            assert!(core.set_pan(track, -0.1));
        }
    }
    assert!(blocks > 0);
    assert_eq!(
        misses, 0,
        "realtime deadline misses={misses}, maximum={maximum:?}"
    );
    println!("AURA_REALTIME_SOAK_EVIDENCE={{\"seconds\":{seconds},\"blocks\":{blocks},\"maximum_us\":{}}}", maximum.as_micros());
}
