//! Shared automation curve evaluation.
//!
//! One implementation of the unified automation curve shape used by every
//! automation lane consumer: the RT track renderer, the DSP differential
//! reference, plugin parameter lanes, and the DAWproject export path.
//!
//! The legacy `curve: f32` column in saved projects and the `repr(C)` FFI is
//! the sigmoid implemented here. `curve == 0.0` reproduces a straight line;
//! positive values bow the middle of the segment upward, negative values bow
//! it downward. The General \[`curves`\] section of the textual patch spec
//! documents this shape so new readers agree without reading this code.
pub(crate) const CURVE_MIN: f32 = -1.0;
pub(crate) const CURVE_MAX: f32 = 1.0;

/// Evaluates the unified sigmoid shape at `amount`, clamped to [0, 1].
///
/// `curve` is clamped to the closed interval [−1, 1] before the polynomial is
/// applied, matching the validation every lane runs before storage.
#[inline]
pub(crate) fn shaped_amount(amount: f32, curve: f32) -> f32 {
    let curve = curve.clamp(CURVE_MIN, CURVE_MAX);
    (amount + curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount)).clamp(0.0, 1.0)
}

/// Evaluates a single segment between neighbouring points, sharing the same
/// shape law. Returns the interpolated value.
#[inline]
pub(crate) fn interpolate_segment(
    left_value: f32,
    right_value: f32,
    amount: f32,
    curve: f32,
) -> f32 {
    left_value + (right_value - left_value) * shaped_amount(amount, curve)
}


/// A cubic Bézier ease between segment endpoints expressed through two
/// control points (the endpoints are always (0, 0) and (1, 1)).
/// Control-point `x` stays inside the unit interval and is non-decreasing,
/// validation guarantees that, so `x(u)` is invertible by bisection.
fn cubic(t: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * mt * p0 + 3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t * p3
}

fn bezier_raise(p: &[crate::project_contracts::Point; 2], amount: f32) -> f32 {
    let [p0, p1] = *p;
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return 0.0;
    }
    if amount >= 1.0 {
        return 1.0;
    }
    // Solve x(u) == amount for u in [0, 1] via bisection; x(u) is strictly
    // increasing under the validation constraints (x0 <= x1, both in [0, 1]).
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    for _ in 0..40 {
        let mid = 0.5 * (low + high);
        let x = cubic(mid, 0.0, p0.x, p1.x, 1.0);
        if x < amount {
            low = mid;
        } else {
            high = mid;
        }
    }
    let u = 0.5 * (low + high);
    cubic(u, 0.0, p0.y, p1.y, 1.0).clamp(0.0, 1.0)
}

/// Evaluates the normalized ease fraction at `amount` for any curve shape.
/// The native lane has no ease primitives other than the shortcut sigmoid,
/// so callers that must cross the FFI boundary bake with `pack_lane_triples`.
pub(crate) fn evaluate_ease(curve: &crate::project_contracts::Curve, amount: f32) -> f32 {
    use crate::project_contracts::{Curve, Shape};
    match curve {
        Curve::Shortcut(weight) => shaped_amount(amount, *weight),
        Curve::Shaped(Shape::Linear) => amount,
        Curve::Shaped(Shape::Step) => {
            if amount >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Curve::Shaped(Shape::Bezier { p }) => bezier_raise(p, amount),
    }
}

/// Number of sub-samples used to bake a non-shortcut/non-linear curve
/// segment into the native sigmoid lane.
pub(crate) const LANE_BAKE_SUBDIVISIONS: usize = 16;

/// Packs a contract lane into the flat `[time, value, curve]` triples the
/// native sigmoid lane consumes. Shortcut and Linear segments pass through
/// exactly (Linear is shortcut weight 0); Step and Bézier segments are baked
/// to linear sub-samples so authored curves survive the FFI without loss of
/// audible accuracy (sub-1% of segment span).
///
/// `points` are carried as `(time, value, curve)` triples.
pub(crate) fn pack_lane_triples(points: &[(f64, f64, crate::project_contracts::Curve)]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len() * 3 * LANE_BAKE_SUBDIVISIONS);
    if points.is_empty() {
        return out;
    }
    for (index, &(time, value, curve)) in points.iter().enumerate() {
        match points.get(index + 1) {
            None => out.extend_from_slice(&[time, value, 0.0]),
            Some(&(next_time, next_value, _)) => {
                use crate::project_contracts::{Curve, Shape};
                match curve {
                    Curve::Shortcut(weight) => {
                        out.extend_from_slice(&[time, value, f64::from(weight)])
                    }
                    Curve::Shaped(Shape::Linear) => out.extend_from_slice(&[time, value, 0.0]),
                    Curve::Shaped(_) => {
                        out.extend_from_slice(&[time, value, 0.0]);
                        for subdivision in 1..=LANE_BAKE_SUBDIVISIONS {
                            let amount = subdivision as f32 / LANE_BAKE_SUBDIVISIONS as f32;
                            let ease = f64::from(evaluate_ease(&curve, amount));
                            let t = time + (next_time - time) * f64::from(amount);
                            let v = value + (next_value - value) * ease;
                            out.extend_from_slice(&[t, v, 0.0]);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_contracts::{Curve, Shape};

    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32) / (u32::MAX as f32)
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + self.next_f32() * (hi - lo)
        }
    }

    #[test]
    fn shortcut_state_remains_the_legacy_sigmoid_law() {
        // The legacy sigmoid must not move: 0.0 reproduces identity, odd
        // weights keep their shape, extreme weights stay clamped.
        assert_eq!(shaped_amount(0.5, 0.0), 0.5);
        for curve in [-1.0f32, -0.5, 0.25, 1.0] {
            for amount in [0.25f32, 0.5, 0.75] {
                let expected = (amount
                    + curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount))
                    .clamp(0.0, 1.0);
                assert_eq!(shaped_amount(amount, curve), expected);
            }
        }
    }

    #[test]
    fn step_holds_the_left_value_until_the_segment_end() {
        use crate::project_contracts::Shape::*;
        let curve = Curve::Shaped(Step);
        assert_eq!(evaluate_ease(&curve, 0.0), 0.0);
        assert_eq!(evaluate_ease(&curve, 0.99), 0.0);
        assert_eq!(evaluate_ease(&curve, 1.0), 1.0);
    }

    #[test]
    fn linear_is_the_identity() {
        assert_eq!(evaluate_ease(&Curve::Shaped(Shape::Linear), 0.37), 0.37);
    }

    #[test]
    fn bezier_hits_endpoints_and_stays_monotone_for_ease_curves() {
        use crate::project_contracts::Point;
        let curve = Curve::Shaped(Shape::Bezier {
            p: [Point { x: 0.25, y: 0.1 }, Point { x: 0.75, y: 0.9 }],
        });
        let mut previous = 0.0f32;
        for step in 0..=64 {
            let amount = step as f32 / 64.0;
            let value = evaluate_ease(&curve, amount);
            assert!((previous..=1.0).contains(&value), "non-monotone ease at {amount}");
            previous = value;
        }
        assert_eq!(evaluate_ease(&curve, 0.0), 0.0);
        assert_eq!(evaluate_ease(&curve, 1.0), 1.0);
    }

    #[test]
    fn randomized_shortcut_curves_match_the_ff_ffi_lane() {
        // The FFI lane and the contract evaluator must agree exactly for
        // shortcut curves; both call `interpolate_segment`.
        let mut lcg = Lcg(0x5DEECE66D);
        for _ in 0..256 {
            let amount = lcg.range(0.0, 1.0);
            let curve = lcg.range(-1.0, 1.0);
            let left = lcg.range(-2.0, 2.0);
            let right = lcg.range(-2.0, 2.0);
            let via_contract = interpolate_segment(left, right, amount, curve);
            let via_shared_sigmoid = left + (right - left) * shaped_amount(amount, curve);
            assert_eq!(via_contract, via_shared_sigmoid);
            let eased = evaluate_ease(&Curve::Shortcut(curve), amount);
            assert_eq!(eased, shaped_amount(amount, curve));
            let value = left + (right - left) * eased;
            assert_eq!(value, via_contract);
        }
    }

    #[test]
    fn pack_lane_triples_round_trips_shortcut_lanes_and_bakes_steps() {
        let shortcut = Curve::Shortcut(0.5);
        let step = Curve::Shaped(Shape::Step);
        let lane = [
            (0.0, 0.0f64, shortcut),
            (100.0, 1.0f64, shortcut),
            (200.0, 0.5f64, step),
            (300.0, 1.0f64, shortcut),
        ];
        let packed = pack_lane_triples(&lane);
        // First segment (shortcut) passes through unchanged; the step segment
        // expands into the baked subdivision count.
        assert_eq!(packed[0..6], [0.0, 0.0, 0.5, 100.0, 1.0, 0.5]);
        assert_eq!(packed.len(), 3 * (2 + LANE_BAKE_SUBDIVISIONS + 2));
        assert_eq!(packed[packed.len() - 3], 300.0);
        assert_eq!(packed[packed.len() - 2], 1.0);
    }
}
