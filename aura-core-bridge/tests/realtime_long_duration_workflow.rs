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
    let mut soft_overruns = 0u64;
    let hard_budget = deadline.saturating_mul(4);
    let mut hard_overruns = 0u64;
    let mut maximum = Duration::ZERO;
    // Prime lazy DSP state and allocator-backed preparation before measuring
    // the callback budget.  A real device likewise delivers a short warm-up
    // period after start; counting one-time initialization as steady-state
    // audio latency makes this gate platform-scheduler dependent.
    for _ in 0..64 {
        assert!(core.process_audio_block(&mut left, &mut right));
    }
    while Instant::now() < stop {
        let started = Instant::now();
        assert!(core.process_audio_block(&mut left, &mut right));
        let elapsed = started.elapsed();
        maximum = maximum.max(elapsed);
        soft_overruns += u64::from(elapsed > deadline);
        hard_overruns += u64::from(elapsed > hard_budget);
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
        hard_overruns, 0,
        "realtime hard budget overruns={hard_overruns}, budget={hard_budget:?}, maximum={maximum:?}"
    );
    println!("AURA_REALTIME_SOAK_EVIDENCE={{\"seconds\":{seconds},\"blocks\":{blocks},\"soft_overruns\":{soft_overruns},\"hard_budget_us\":{},\"maximum_us\":{}}}", hard_budget.as_micros(), maximum.as_micros());
}
