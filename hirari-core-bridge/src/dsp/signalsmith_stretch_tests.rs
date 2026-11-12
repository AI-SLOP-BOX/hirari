//! Tests split out of signalsmith_stretch.rs.

    use super::*;

    #[test]
    fn rng_matches_libcpp_default_random_engine() {
        // Reference values from libc++ std::default_random_engine(17) with
        // std::uniform_real_distribution<float>(-18.5f, 22.5f).
        let mut rng = MinstdRand0::new(17);
        let expected = [
            -18.484333, -0.232383013, -9.357234, -12.0469398, 0.16512394, -9.28622913,
            12.4381495, 12.9118471,
        ];
        for (index, want) in expected.iter().enumerate() {
            let got = uniform_real(-18.5, 22.5, &mut rng);
            assert!(
                (got - want).abs() < 1.0e-6,
                "draw {index}: got {got} want {want}"
            );
        }
    }
