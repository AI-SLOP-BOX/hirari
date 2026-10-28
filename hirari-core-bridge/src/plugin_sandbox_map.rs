//! Sandboxed-plugin slot bookkeeping. A track keeps one slot index per
//! sandboxed plugin next to the effect-chain order; removing or moving a
//! plugin has to carry that list through the same permutation. The rules live
//! here so removal and reordering cannot drift apart.

use std::slice;

/// Position of the first slot that points at `plugin_index`, or the slot count
/// when no slot does. The caller uses it to reach the parallel path entry.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_find(
    indices: *const u32,
    count: usize,
    plugin_index: u32,
) -> usize {
    if indices.is_null() || count == 0 {
        return 0;
    }
    let values = unsafe { slice::from_raw_parts(indices, count) };
    values
        .iter()
        .position(|&value| value == plugin_index)
        .unwrap_or(count)
}

/// Drops every slot that points at `plugin_index` and shifts later slots down
/// to follow the effect-chain erase. `order` receives the old position of each
/// survivor so the caller can rebuild its parallel path list; the return value
/// is the survivor count. Slots beyond `count` are untouched.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_remove(
    indices: *mut u32,
    order: *mut u32,
    count: usize,
    plugin_index: u32,
) -> usize {
    if indices.is_null() || order.is_null() || count == 0 {
        return 0;
    }
    let values: Vec<u32> = unsafe { slice::from_raw_parts(indices, count) }.to_vec();
    let mut kept = 0usize;
    for (position, &value) in values.iter().enumerate() {
        if value == plugin_index {
            continue;
        }
        let next = if value > plugin_index { value - 1 } else { value };
        unsafe {
            *indices.add(kept) = next;
            *order.add(kept) = position as u32;
        }
        kept += 1;
    }
    kept
}

/// Re-points the slots after the effect chain moved a plugin from `from` to
/// `to` and sorts them back into chain order. Slots whose position has no
/// matching path entry are dropped, exactly as the previous loop did. The
/// return value is the survivor count; `order` maps each survivor to its old
/// position.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_move(
    indices: *mut u32,
    order: *mut u32,
    count: usize,
    path_count: usize,
    from: u32,
    to: u32,
) -> usize {
    if indices.is_null() || order.is_null() || count == 0 || from == to {
        return 0;
    }
    let values: Vec<u32> = unsafe { slice::from_raw_parts(indices, count) }.to_vec();
    let mut moved: Vec<(u32, u32)> = Vec::with_capacity(count.min(path_count));
    for (position, &value) in values.iter().enumerate() {
        if position >= path_count {
            continue;
        }
        let index = if value == from {
            to
        } else if from < to && value > from && value <= to {
            value - 1
        } else if to < from && value >= to && value < from {
            value + 1
        } else {
            value
        };
        moved.push((index, position as u32));
    }
    moved.sort_unstable_by_key(|entry| entry.0);
    for (slot, &(index, position)) in moved.iter().enumerate() {
        unsafe {
            *indices.add(slot) = index;
            *order.add(slot) = position;
        }
    }
    moved.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remove(mut indices: Vec<u32>, plugin: u32) -> (Vec<u32>, Vec<u32>) {
        let count = indices.len();
        let mut order = vec![u32::MAX; count];
        let kept = unsafe {
            hirari_plugin_sandbox_remove(indices.as_mut_ptr(), order.as_mut_ptr(), count, plugin)
        };
        indices.truncate(kept);
        order.truncate(kept);
        (indices, order)
    }

    fn move_slots(mut indices: Vec<u32>, paths: usize, from: u32, to: u32) -> (Vec<u32>, Vec<u32>) {
        let count = indices.len();
        let mut order = vec![u32::MAX; count];
        let kept = unsafe {
            hirari_plugin_sandbox_move(
                indices.as_mut_ptr(),
                order.as_mut_ptr(),
                count,
                paths,
                from,
                to,
            )
        };
        indices.truncate(kept);
        order.truncate(kept);
        (indices, order)
    }

    #[test]
    fn removal_shifts_later_slots_down() {
        let (indices, order) = remove(vec![0, 2, 5], 2);
        assert_eq!(indices, vec![0, 4]);
        assert_eq!(order, vec![0, 2]);
    }

    #[test]
    fn removal_before_every_slot_shifts_all() {
        let (indices, order) = remove(vec![3, 7], 0);
        assert_eq!(indices, vec![2, 6]);
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn removal_of_an_absent_plugin_only_shifts_slots_above_it() {
        let (indices, order) = remove(vec![1, 4], 9);
        assert_eq!(indices, vec![1, 4]);
        assert_eq!(order, vec![0, 1]);
        let (indices, _) = remove(vec![4, 6], 5);
        assert_eq!(indices, vec![4, 5]);
    }

    #[test]
    fn moving_forward_repeats_the_chain_permutation() {
        // Plugin 1 moves to slot 3: 0 stays, 2 and 3 shift down, 1 becomes 3.
        let (indices, order) = move_slots(vec![0, 1, 2, 3], 4, 1, 3);
        assert_eq!(indices, vec![0, 1, 2, 3]);
        assert_eq!(order, vec![0, 2, 3, 1]);
    }

    #[test]
    fn moving_backward_repeats_the_chain_permutation() {
        let (indices, order) = move_slots(vec![0, 2], 2, 3, 1);
        assert_eq!(indices, vec![0, 3]);
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn slots_without_a_path_are_dropped() {
        let (indices, order) = move_slots(vec![0, 1, 2], 2, 0, 1);
        assert_eq!(indices, vec![0, 1]);
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn degenerate_requests_do_nothing() {
        let (indices, _) = remove(vec![1], 0);
        assert_eq!(indices, vec![0]);
        let (indices, _) = move_slots(vec![1], 1, 1, 1);
        assert!(indices.is_empty());
        let kept = unsafe {
            hirari_plugin_sandbox_remove(std::ptr::null_mut(), std::ptr::null_mut(), 4, 0)
        };
        assert_eq!(kept, 0);
    }
}
