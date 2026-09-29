//! Runtime VCA group ownership and lock-free resolved gain lookup for Tracks.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

const MAX_TRACKS: usize = 2048;

struct Group {
    gain: f32,
    tracks: Vec<u32>,
}

struct VcaManager {
    groups: Mutex<HashMap<u32, Group>>,
    resolved_gain_bits: [AtomicU32; MAX_TRACKS],
}

impl VcaManager {
    fn new() -> Self {
        Self {
            groups: Mutex::new(HashMap::new()),
            resolved_gain_bits: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
        }
    }

    fn add_group(&self, id: u32, gain: f32) {
        if id == 0 {
            return;
        }
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 8.0)
        } else {
            1.0
        };
        self.groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id)
            .or_insert_with(|| Group {
                gain: 1.0,
                tracks: Vec::new(),
            })
            .gain = gain;
    }

    fn assign_track(&self, track_id: u32, group_id: u32) -> bool {
        if track_id as usize >= MAX_TRACKS || group_id == 0 {
            return false;
        }
        let mut groups = self
            .groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(group) = groups.get_mut(&group_id) else {
            return false;
        };
        if !group.tracks.contains(&track_id) {
            group.tracks.push(track_id);
        }
        Self::resolve_locked(&groups, &self.resolved_gain_bits);
        true
    }

    fn resolve(&self) {
        let groups = self
            .groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::resolve_locked(&groups, &self.resolved_gain_bits);
    }

    fn resolve_locked(groups: &HashMap<u32, Group>, resolved: &[AtomicU32; MAX_TRACKS]) {
        for gain in resolved {
            gain.store(1.0f32.to_bits(), Ordering::Relaxed);
        }
        for group in groups.values() {
            let gain = if group.gain.is_finite() {
                group.gain.clamp(0.0, 8.0)
            } else {
                1.0
            };
            for &track_id in &group.tracks {
                let Some(current) = resolved.get(track_id as usize) else {
                    continue;
                };
                let current_gain = f32::from_bits(current.load(Ordering::Relaxed));
                current.store(
                    (current_gain * gain).clamp(0.0, 8.0).to_bits(),
                    Ordering::Release,
                );
            }
        }
    }

    fn cumulative_gain(&self, track_id: u32) -> f32 {
        self.resolved_gain_bits
            .get(track_id as usize)
            .map(|gain| f32::from_bits(gain.load(Ordering::Relaxed)))
            .unwrap_or(1.0)
    }

    fn clear(&self) {
        let mut groups = self
            .groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        groups.clear();
        Self::resolve_locked(&groups, &self.resolved_gain_bits);
    }

    fn snapshot_json(&self) -> String {
        #[derive(Serialize)]
        struct Snapshot<'a> {
            id: u32,
            gain: f32,
            track_ids: &'a [u32],
        }
        let groups = self
            .groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ids: Vec<_> = groups.keys().copied().collect();
        ids.sort_unstable();
        let snapshots: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                groups.get(&id).map(|group| Snapshot {
                    id,
                    gain: if group.gain.is_finite() {
                        group.gain
                    } else {
                        1.0
                    },
                    track_ids: &group.tracks,
                })
            })
            .collect();
        serde_json::to_string(&snapshots).unwrap_or_else(|_| "[]".to_owned())
    }
}

fn global_manager() -> &'static VcaManager {
    static MANAGER: OnceLock<VcaManager> = OnceLock::new();
    MANAGER.get_or_init(VcaManager::new)
}

fn copy_snapshot_json(json: &str, output: *mut u8, capacity: usize) -> usize {
    let required = json.len().saturating_add(1);
    if output.is_null() || capacity < required {
        return required;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(json.as_ptr(), output, json.len());
        *output.add(json.len()) = 0;
    }
    required
}

fn apply_stereo_gain(left: &mut [f32], right: &mut [f32], gain: f32) {
    if !gain.is_finite() || gain == 1.0 {
        return;
    }
    for (left, right) in left.iter_mut().zip(right) {
        let scaled_left = *left * gain;
        let scaled_right = *right * gain;
        *left = if scaled_left.is_finite() {
            scaled_left
        } else {
            0.0
        };
        *right = if scaled_right.is_finite() {
            scaled_right
        } else {
            0.0
        };
    }
}

/// Applies the resolved VCA gain to one stereo track buffer on the audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_vca_apply_track_gain(
    track_id: u32,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if frames == 0 || left.is_null() || right.is_null() {
        return;
    }
    let gain = global_manager().cumulative_gain(track_id);
    if !gain.is_finite() || gain == 1.0 {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames as usize) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames as usize) };
    apply_stereo_gain(left, right, gain);
}

#[no_mangle]
pub extern "C" fn hirari_vca_add_group(group_id: u32, gain: f32) {
    global_manager().add_group(group_id, gain);
}

#[no_mangle]
pub extern "C" fn hirari_vca_resolve_hierarchy() {
    global_manager().resolve();
}

#[no_mangle]
pub extern "C" fn hirari_vca_assign_track(track_id: u32, group_id: u32) -> bool {
    global_manager().assign_track(track_id, group_id)
}

#[no_mangle]
pub extern "C" fn hirari_vca_get_cumulative_gain(track_id: u32) -> f32 {
    global_manager().cumulative_gain(track_id)
}

#[no_mangle]
pub extern "C" fn hirari_vca_clear() {
    global_manager().clear();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_vca_snapshot_json(output: *mut u8, capacity: usize) -> usize {
    let json = global_manager().snapshot_json();
    copy_snapshot_json(&json, output, capacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_group_assignments_resolve_to_realtime_gain_table() {
        let manager = VcaManager::new();
        manager.add_group(10, 0.5);
        manager.add_group(20, 0.25);
        assert!(manager.assign_track(77, 10));
        assert!(manager.assign_track(77, 20));
        assert!(manager.assign_track(77, 20));
        assert!((manager.cumulative_gain(77) - 0.125).abs() < 1.0e-7);
        assert_eq!(manager.cumulative_gain(78), 1.0);
    }

    #[test]
    fn validation_clamping_and_clear_match_the_native_manager_contract() {
        let manager = VcaManager::new();
        manager.add_group(0, 4.0);
        manager.add_group(1, f32::NAN);
        manager.resolve();
        assert_eq!(manager.cumulative_gain(5), 1.0);
        assert!(!manager.assign_track(5, 0));
        assert!(!manager.assign_track(MAX_TRACKS as u32, 1));
        assert!(manager.assign_track(5, 1));
        manager.add_group(1, 20.0);
        manager.resolve();
        assert_eq!(manager.cumulative_gain(5), 8.0);
        manager.clear();
        assert_eq!(manager.cumulative_gain(5), 1.0);
        assert_eq!(manager.snapshot_json(), "[]");
    }

    #[test]
    fn snapshot_keeps_the_native_json_shape_and_member_order() {
        let manager = VcaManager::new();
        manager.add_group(42, 0.5);
        manager.assign_track(9, 42);
        manager.assign_track(3, 42);
        let json = manager.snapshot_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["id"], 42);
        assert_eq!(value[0]["gain"], 0.5);
        assert_eq!(value[0]["track_ids"], serde_json::json!([9, 3]));
    }

    #[test]
    fn ffi_snapshot_reports_required_buffer_size_before_copy() {
        let manager = VcaManager::new();
        manager.add_group(314, 0.75);
        let json = manager.snapshot_json();
        let required = json.len() + 1;
        let mut bytes = vec![0u8; required];
        assert_eq!(copy_snapshot_json(&json, std::ptr::null_mut(), 0), required);
        assert_eq!(
            copy_snapshot_json(&json, bytes.as_mut_ptr(), required),
            required
        );
        assert_eq!(required, bytes.len());
        assert_eq!(&bytes[..json.len()], json.as_bytes());
        assert_eq!(bytes[json.len()], 0);
    }

    #[test]
    fn track_gain_kernel_scales_stereo_and_contains_non_finite_results() {
        let mut left = [0.5, f32::MAX, f32::NAN, -0.25];
        let mut right = [-0.5, -f32::MAX, f32::INFINITY, 0.75];
        apply_stereo_gain(&mut left, &mut right, 2.0);
        assert_eq!(left, [1.0, 0.0, 0.0, -0.5]);
        assert_eq!(right, [-1.0, 0.0, 0.0, 1.5]);
    }

    #[test]
    fn track_gain_kernel_preserves_buffers_for_unity_or_invalid_gain() {
        let mut left = [f32::NAN, 0.25];
        let mut right = [0.5, -0.25];
        apply_stereo_gain(&mut left, &mut right, 1.0);
        assert!(left[0].is_nan());
        assert_eq!(&left[1..], &[0.25]);
        assert_eq!(right, [0.5, -0.25]);
        apply_stereo_gain(&mut left, &mut right, f32::NAN);
        assert!(left[0].is_nan());
        assert_eq!(right, [0.5, -0.25]);
    }
}
