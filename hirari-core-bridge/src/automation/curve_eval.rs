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
