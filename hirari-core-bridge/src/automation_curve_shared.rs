use arc_swap::ArcSwap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeAutomationPoint {
    pub time: f64,
    pub value: f32,
    pub interpolation: i32,
    pub curvature: f32,
}

struct SharedAutomationCurve {
    writer: Mutex<()>,
    points: ArcSwap<Vec<NativeAutomationPoint>>,
}

impl SharedAutomationCurve {
    fn new() -> Self {
        Self {
            writer: Mutex::new(()),
            points: ArcSwap::from_pointee(Vec::new()),
        }
    }

    fn add_point(&self, point: NativeAutomationPoint) {
        if !point.time.is_finite() || !point.value.is_finite() {
            return;
        }
        let Ok(_guard) = self.writer.lock() else {
            return;
        };
        let mut next = self.points.load_full().as_ref().clone();
        next.push(point);
        // Stable sorting makes same-time points deterministic: the later point
        // is the one selected by the upper-bound lookup.
        next.sort_by(|left, right| left.time.total_cmp(&right.time));
        self.points.store(Arc::new(next));
    }

    fn value_at(&self, time: f64) -> f32 {
        if !time.is_finite() {
            return 0.0;
        }
        let points = self.points.load();
        let points = points.as_slice();
        if points.is_empty() {
            return 0.0;
        }
        if time <= points[0].time {
            return points[0].value;
        }
        if time >= points[points.len() - 1].time {
            return points[points.len() - 1].value;
        }
        let upper = points.partition_point(|point| point.time <= time);
        let a = points[upper - 1];
        let b = points[upper];
        if a.interpolation == 0 || b.time <= a.time {
            return a.value;
        }
        let amount = ((time - a.time) / (b.time - a.time)) as f32;
        let curvature = a.curvature.clamp(-1.0, 1.0);
        let shaped = if a.interpolation == 1 {
            amount
        } else {
            (amount + curvature * amount * (1.0 - amount) * (1.0 - 2.0 * amount)).clamp(0.0, 1.0)
        };
        a.value + (b.value - a.value) * shaped
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_curve_create() -> *mut c_void {
    Box::into_raw(Box::new(SharedAutomationCurve::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_curve_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: handle comes from the matching create function and is destroyed once.
        unsafe { drop(Box::from_raw(state.cast::<SharedAutomationCurve>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_curve_add_point(
    state: *mut c_void,
    time: f64,
    value: f32,
    interpolation: i32,
) {
    if let Some(state) = unsafe { state.cast::<SharedAutomationCurve>().as_ref() } {
        state.add_point(NativeAutomationPoint {
            time,
            value,
            interpolation,
            curvature: 0.5,
        });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_curve_value_at(state: *const c_void, time: f64) -> f32 {
    unsafe { state.cast::<SharedAutomationCurve>().as_ref() }
        .map_or(0.0, |state| state.value_at(time))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_curve_copy_points(
    state: *const c_void,
    output: *mut NativeAutomationPoint,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<SharedAutomationCurve>().as_ref() }) else {
        return 0;
    };
    let points = state.points.load();
    let count = points.len();
    if output.is_null() || capacity == 0 {
        return count;
    }
    let copy_count = count.min(capacity);
    // SAFETY: caller provides `capacity` writable entries and ArcSwap keeps
    // this immutable point generation alive for the duration of the copy.
    unsafe {
        std::ptr::copy_nonoverlapping(points.as_ptr(), output, copy_count);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::{NativeAutomationPoint, SharedAutomationCurve};

    #[test]
    fn point_snapshot_is_sorted_and_evaluation_matches_interpolation_modes() {
        let curve = SharedAutomationCurve::new();
        curve.add_point(NativeAutomationPoint {
            time: 2.0,
            value: 1.0,
            interpolation: 1,
            curvature: 0.5,
        });
        curve.add_point(NativeAutomationPoint {
            time: 0.0,
            value: 0.0,
            interpolation: 1,
            curvature: 0.5,
        });
        assert_eq!(curve.value_at(1.0), 0.5);
        assert_eq!(curve.value_at(-1.0), 0.0);
        assert_eq!(curve.value_at(3.0), 1.0);

        curve.add_point(NativeAutomationPoint {
            time: 1.0,
            value: 0.25,
            interpolation: 0,
            curvature: 0.5,
        });
        assert_eq!(curve.value_at(1.5), 0.25);
        let points = curve.points.load();
        assert!(points.windows(2).all(|pair| pair[0].time <= pair[1].time));
    }

    #[test]
    fn concurrent_readers_keep_a_coherent_published_generation() {
        let curve = SharedAutomationCurve::new();
        curve.add_point(NativeAutomationPoint {
            time: 0.0,
            value: 0.0,
            interpolation: 1,
            curvature: 0.5,
        });
        curve.add_point(NativeAutomationPoint {
            time: 1.0,
            value: 1.0,
            interpolation: 1,
            curvature: 0.5,
        });
        let writer = std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                for index in 2..100 {
                    curve.add_point(NativeAutomationPoint {
                        time: index as f64,
                        value: index as f32,
                        interpolation: 1,
                        curvature: 0.5,
                    });
                }
            });
            for _ in 0..1000 {
                assert!(curve.value_at(0.5).is_finite());
                let snapshot = curve.points.load();
                assert!(snapshot.windows(2).all(|pair| pair[0].time <= pair[1].time));
            }
            writer.join().unwrap();
        });
        let _ = writer;
        assert_eq!(curve.points.load().len(), 100);
    }

    #[test]
    fn bezier_and_exponential_types_use_their_curvature_shape() {
        for interpolation in [2, 3] {
            let curve = SharedAutomationCurve::new();
            curve.add_point(NativeAutomationPoint {
                time: 0.0,
                value: 0.0,
                interpolation,
                curvature: 0.5,
            });
            curve.add_point(NativeAutomationPoint {
                time: 1.0,
                value: 1.0,
                interpolation: 1,
                curvature: 0.5,
            });
            assert!((curve.value_at(0.25) - 0.296875).abs() < 1e-6);
        }
    }
}
