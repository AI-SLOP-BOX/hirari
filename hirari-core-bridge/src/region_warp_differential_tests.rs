use super::{hirari_region_source_block, hirari_region_source_position_at, HirariWarpMarker};

unsafe extern "C" {
    fn hirari_region_source_position_reference(
        markers: *const HirariWarpMarker,
        marker_count: usize,
        timeline_sample: u64,
        source_rate: f64,
        source_span: u64,
    ) -> f64;
}

fn compare(markers: &[HirariWarpMarker], rate: f64, span: u64) {
    let times = [
        0, 1, 2, 99, 100, 101, 249, 250, 251, 500, 749, 750, 999, 1_000, 1_200,
    ];
    let marker_pointer = if markers.is_empty() {
        std::ptr::null()
    } else {
        markers.as_ptr()
    };
    for timeline in times {
        let rust = unsafe {
            hirari_region_source_position_at(marker_pointer, markers.len(), timeline, rate, span)
        };
        let cpp = unsafe {
            hirari_region_source_position_reference(
                marker_pointer,
                markers.len(),
                timeline,
                rate,
                span,
            )
        };
        assert!(
            (rust - cpp).abs() <= 1.0e-12,
            "timeline={timeline}, markers={markers:?}, rate={rate}, Rust={rust}, C++={cpp}"
        );
    }
}

#[test]
fn rust_warp_position_matches_frozen_cpp_for_fallback_and_marker_edges() {
    assert_eq!(std::mem::size_of::<HirariWarpMarker>(), 24);
    for rate in [0.25, 0.5, 1.0, 1.75, 4.0] {
        compare(&[], rate, 1_000);
        compare(
            &[HirariWarpMarker {
                source_sample: 150,
                timeline_sample: 200,
                transient: 1,
            }],
            rate,
            1_000,
        );
    }

    for markers in [
        vec![
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 300,
                timeline_sample: 250,
                transient: 1,
            },
            HirariWarpMarker {
                source_sample: 700,
                timeline_sample: 750,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 1_000,
                timeline_sample: 1_000,
                transient: 1,
            },
        ],
        vec![
            HirariWarpMarker {
                source_sample: 20,
                timeline_sample: 100,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 20,
                timeline_sample: 100,
                transient: 1,
            },
        ],
        vec![
            HirariWarpMarker {
                source_sample: 100,
                timeline_sample: 200,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 900,
                timeline_sample: 800,
                transient: 0,
            },
        ],
    ] {
        compare(&markers, 1.25, 1_000);
    }
}

#[test]
fn rust_track_source_block_matches_frozen_cpp_positions_and_local_rates() {
    let fixtures = [
        vec![],
        vec![HirariWarpMarker {
            source_sample: 3,
            timeline_sample: 2,
            transient: 1,
        }],
        vec![
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 9,
                timeline_sample: 3,
                transient: 1,
            },
            HirariWarpMarker {
                source_sample: 18,
                timeline_sample: 7,
                transient: 0,
            },
        ],
    ];
    const REGION_LENGTH: u64 = 8;
    const FRAME_COUNT: usize = 15;
    for markers in fixtures {
        let pointer = if markers.is_empty() {
            std::ptr::null()
        } else {
            markers.as_ptr()
        };
        for first_sample in [0, 5, 7, 12] {
            for rate in [0.5, 1.0, 1.75] {
                let mut positions = [0.0; FRAME_COUNT];
                let mut rates = [0.0; FRAME_COUNT];
                unsafe {
                    hirari_region_source_block(
                        pointer,
                        markers.len(),
                        first_sample,
                        REGION_LENGTH,
                        rate,
                        20,
                        FRAME_COUNT as u32,
                        positions.as_mut_ptr(),
                        rates.as_mut_ptr(),
                    );
                }
                for frame in 0..FRAME_COUNT {
                    let timeline = (first_sample + frame as u64) % REGION_LENGTH;
                    let expected_position = unsafe {
                        hirari_region_source_position_reference(
                            pointer,
                            markers.len(),
                            timeline,
                            rate,
                            20,
                        )
                    };
                    let expected_rate = if timeline + 1 < REGION_LENGTH {
                        unsafe {
                            hirari_region_source_position_reference(
                                pointer,
                                markers.len(),
                                timeline + 1,
                                rate,
                                20,
                            ) - expected_position
                        }
                    } else if timeline > 0 {
                        expected_position
                            - unsafe {
                                hirari_region_source_position_reference(
                                    pointer,
                                    markers.len(),
                                    timeline - 1,
                                    rate,
                                    20,
                                )
                            }
                    } else {
                        1.0
                    };
                    assert!((positions[frame] - expected_position).abs() <= 1.0e-12);
                    assert!((rates[frame] - expected_rate).abs() <= 1.0e-12);
                }
            }
        }
    }
}
