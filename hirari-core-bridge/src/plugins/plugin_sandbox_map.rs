//! Sandboxed-plugin slot registry. A track keeps one slot per sandboxed plugin
//! next to the effect-chain order, so removal and reordering have to carry the
//! slot list through the same permutation. The registry owns both halves of a
//! slot (chain index and plugin path) here so the two lists cannot drift apart
//! the way two C++ vectors could.

use std::slice;

/// Owned slot registry: `indices[i]` is the effect-chain index of slot `i` and
/// `paths[i]` is the plugin path loaded into it.
pub struct SandboxSlotStore {
    indices: Vec<u32>,
    paths: Vec<String>,
}

impl SandboxSlotStore {
    fn new() -> Self {
        Self {
            indices: Vec::new(),
            paths: Vec::new(),
        }
    }

    fn position(&self, plugin_index: u32) -> Option<usize> {
        self.indices.iter().position(|&value| value == plugin_index)
    }

    fn remove(&mut self, plugin_index: u32) {
        let mut kept = 0usize;
        for position in 0..self.indices.len() {
            let value = self.indices[position];
            if value == plugin_index {
                continue;
            }
            self.indices[kept] = if value > plugin_index { value - 1 } else { value };
            self.paths.swap(kept, position);
            kept += 1;
        }
        self.indices.truncate(kept);
        self.paths.truncate(kept);
    }

    fn insert(&mut self, plugin_index: u32, path: String) {
        for value in &mut self.indices {
            if *value >= plugin_index {
                *value += 1;
            }
        }
        let offset = self
            .indices
            .iter()
            .position(|&value| value >= plugin_index)
            .unwrap_or(self.indices.len());
        self.indices.insert(offset, plugin_index);
        self.paths.insert(offset, path);
    }

    fn move_slot(&mut self, from: u32, to: u32) {
        if from == to {
            return;
        }
        let mut moved: Vec<(u32, usize)> = self
            .indices
            .iter()
            .enumerate()
            .map(|(position, &value)| {
                let index = if value == from {
                    to
                } else if from < to && value > from && value <= to {
                    value - 1
                } else if to < from && value >= to && value < from {
                    value + 1
                } else {
                    value
                };
                (index, position)
            })
            .collect();
        moved.sort_unstable_by_key(|entry| entry.0);
        let paths = &self.paths;
        let reordered: Vec<String> = moved.iter().map(|&(_, position)| paths[position].clone()).collect();
        let indices: Vec<u32> = moved.iter().map(|&(index, _)| index).collect();
        self.indices = indices;
        self.paths = reordered;
    }
}

#[no_mangle]
pub extern "C" fn hirari_sandbox_slots_create() -> *mut SandboxSlotStore {
    Box::into_raw(Box::new(SandboxSlotStore::new()))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_destroy(store: *mut SandboxSlotStore) {
    if store.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(store) });
}

/// Reads a caller-owned UTF-8 path. Returns `None` for a null pointer with a
/// non-zero length so callers cannot smuggle an empty path past validation.
unsafe fn path_from<'a>(data: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(data, len) })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_len(store: *const SandboxSlotStore) -> usize {
    if store.is_null() {
        return 0;
    }
    unsafe { &*store }.indices.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_index(
    store: *const SandboxSlotStore,
    slot: usize,
) -> u32 {
    if store.is_null() {
        return u32::MAX;
    }
    unsafe { &*store }
        .indices
        .get(slot)
        .copied()
        .unwrap_or(u32::MAX)
}

/// Position of the slot that points at `plugin_index`, or `usize::MAX` when no
/// slot does. Callers use it to reach the parallel path entry.
#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_find(
    store: *const SandboxSlotStore,
    plugin_index: u32,
) -> usize {
    if store.is_null() {
        return usize::MAX;
    }
    unsafe { &*store }.position(plugin_index).unwrap_or(usize::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_path_size(
    store: *const SandboxSlotStore,
    slot: usize,
) -> usize {
    if store.is_null() {
        return 0;
    }
    unsafe { &*store }
        .paths
        .get(slot)
        .map(String::len)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_path_copy(
    store: *const SandboxSlotStore,
    slot: usize,
    out: *mut u8,
    capacity: usize,
) -> usize {
    if store.is_null() {
        return 0;
    }
    let Some(path) = unsafe { &*store }.paths.get(slot) else {
        return 0;
    };
    if out.is_null() || capacity < path.len() {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(path.as_ptr(), out, path.len()) };
    path.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_contains_path(
    store: *const SandboxSlotStore,
    path: *const u8,
    len: usize,
) -> bool {
    if store.is_null() {
        return false;
    }
    let Some(bytes) = (unsafe { path_from(path, len) }) else {
        return false;
    };
    let Ok(path) = std::str::from_utf8(bytes) else {
        return false;
    };
    unsafe { &*store }.paths.iter().any(|existing| existing == path)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_push(
    store: *mut SandboxSlotStore,
    plugin_index: u32,
    path: *const u8,
    len: usize,
) -> bool {
    if store.is_null() {
        return false;
    }
    let Some(bytes) = (unsafe { path_from(path, len) }) else {
        return false;
    };
    let Ok(path) = std::str::from_utf8(bytes) else {
        return false;
    };
    let store = unsafe { &mut *store };
    store.indices.push(plugin_index);
    store.paths.push(path.to_owned());
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_remove(
    store: *mut SandboxSlotStore,
    plugin_index: u32,
) -> usize {
    if store.is_null() {
        return 0;
    }
    let store = unsafe { &mut *store };
    store.remove(plugin_index);
    store.indices.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_insert(
    store: *mut SandboxSlotStore,
    plugin_index: u32,
    path: *const u8,
    len: usize,
) -> bool {
    if store.is_null() {
        return false;
    }
    let Some(bytes) = (unsafe { path_from(path, len) }) else {
        return false;
    };
    let Ok(path) = std::str::from_utf8(bytes) else {
        return false;
    };
    unsafe { &mut *store }.insert(plugin_index, path.to_owned());
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sandbox_slots_move(
    store: *mut SandboxSlotStore,
    from: u32,
    to: u32,
) {
    if store.is_null() {
        return;
    }
    unsafe { &mut *store }.move_slot(from, to);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(entries: &[(u32, &str)]) -> SandboxSlotStore {
        let mut store = SandboxSlotStore::new();
        for &(index, path) in entries {
            store.indices.push(index);
            store.paths.push(path.to_owned());
        }
        store
    }

    #[test]
    fn removal_shifts_later_slots_down_and_keeps_paths_aligned() {
        let mut slots = store(&[(0, "a"), (2, "b"), (5, "c")]);
        slots.remove(2);
        assert_eq!(slots.indices, vec![0, 4]);
        assert_eq!(slots.paths, vec!["a".to_owned(), "c".to_owned()]);
    }

    #[test]
    fn removal_before_every_slot_shifts_all() {
        let mut slots = store(&[(3, "a"), (7, "b")]);
        slots.remove(0);
        assert_eq!(slots.indices, vec![2, 6]);
        assert_eq!(slots.paths, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn removal_of_an_absent_plugin_only_shifts_slots_above_it() {
        let mut slots = store(&[(1, "a"), (4, "b")]);
        slots.remove(9);
        assert_eq!(slots.indices, vec![1, 4]);
        let mut slots = store(&[(4, "a"), (6, "b")]);
        slots.remove(5);
        assert_eq!(slots.indices, vec![4, 5]);
    }

    #[test]
    fn insert_shifts_later_slots_and_keeps_order() {
        let mut slots = store(&[(0, "a"), (2, "b")]);
        slots.insert(1, "new".to_owned());
        assert_eq!(slots.indices, vec![0, 1, 3]);
        assert_eq!(slots.paths, vec!["a", "new", "b"]);
    }

    #[test]
    fn moving_forward_repeats_the_chain_permutation() {
        let mut slots = store(&[(0, "a"), (1, "b"), (2, "c"), (3, "d")]);
        slots.move_slot(1, 3);
        assert_eq!(slots.indices, vec![0, 1, 2, 3]);
        assert_eq!(slots.paths, vec!["a", "c", "d", "b"]);
    }

    #[test]
    fn moving_backward_repeats_the_chain_permutation() {
        let mut slots = store(&[(0, "a"), (2, "b")]);
        slots.move_slot(3, 1);
        assert_eq!(slots.indices, vec![0, 3]);
        assert_eq!(slots.paths, vec!["a", "b"]);
    }

    #[test]
    fn duplicate_paths_are_detected() {
        let slots = store(&[(0, "a"), (1, "b")]);
        assert!(slots.paths.contains(&"b".to_owned()));
        assert_eq!(slots.position(1), Some(1));
        assert_eq!(slots.position(9), None);
    }
}
