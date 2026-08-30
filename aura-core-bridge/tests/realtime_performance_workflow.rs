use aura_core_bridge::AuraCore;
use std::time::Instant;

mod support {
    include!("recording_support.rs");
}
use support::native_engine_test_guard;

#[test]
#[ignore = "release-mode realtime performance evidence gate"]
fn hundred_track_callback_matrix_meets_device_deadlines() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let track_ids: Vec<u32> = (0..100).map(|_| core.add_track(0)).collect();

    let mut evidence = Vec::new();
    for frames in [128usize, 256, 512, 1024] {
        assert!(core.apply_audio_config(48_000, frames as u32));
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];

        for _ in 0..64 {
            assert!(core.process_audio_block(&mut left, &mut right));
        }

        let mut timings_us = Vec::with_capacity(2_000);
        for block in 0..2_000usize {
            left[block % frames] = 0.125;
            right[(block * 17) % frames] = -0.125;
            let started = Instant::now();
            assert!(core.process_audio_block(&mut left, &mut right));
            timings_us.push(started.elapsed().as_micros() as u64);

            // Exercise control-plane publication between callbacks without
            // moving locks or allocations into the measured callback itself.
            if block % 100 == 0 {
                let track = track_ids[(block / 100) % track_ids.len()];
                assert!(core.set_volume(track, 0.5 + (block % 5) as f32 * 0.1));
                assert!(core.set_pan(track, ((block % 3) as f32 - 1.0) * 0.25));
            }
        }

        timings_us.sort_unstable();
        let deadline_us = (frames as u64 * 1_000_000) / 48_000;
        let maximum_us = *timings_us.last().unwrap();
        let p99_us = timings_us[(timings_us.len() * 99) / 100];
        let deadline_misses = timings_us
            .iter()
            .filter(|duration| **duration > deadline_us)
            .count();
        evidence.push(serde_json::json!({
            "frames": frames,
            "sample_rate": 48_000,
            "tracks": 100,
            "blocks": timings_us.len(),
            "deadline_us": deadline_us,
            "maximum_us": maximum_us,
            "p99_us": p99_us,
            "deadline_misses": deadline_misses,
        }));
        assert_eq!(
            deadline_misses,
            0,
            "callback deadline missed: {}",
            evidence.last().unwrap()
        );
    }
    println!(
        "AURA_REALTIME_EVIDENCE={}",
        serde_json::Value::Array(evidence)
    );
}
