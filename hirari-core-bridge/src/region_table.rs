//! Rust-side mirror of a track's region table (stage 1 of the Region storage
//! migration, see `region_migration_design.md`).
//!
//! The C++ `Track::m_regions` is still authoritative. Every publish of the
//! region snapshot replaces this mirror wholesale, so the two cannot drift
//! across the many mutation paths that end in `publishRegionSnapshot()`.
//! Later stages move mutators onto this table and finally flip authority.

use crate::region_edit_host::HirariRegionState;

/// Upper bound on mirrored regions, matching the engine's practical limits so
/// a corrupt caller cannot make the mirror allocate without bound.
pub const HIRARI_REGION_TABLE_MAX: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RegionEntry {
    pub id: u32,
    pub state: HirariRegionState,
}

impl PartialEq for HirariRegionState {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.len == other.len
            && self.base_start == other.base_start
            && self.base_length == other.base_length
            && self.source_offset == other.source_offset
            && self.base_source_offset == other.base_source_offset
            && self.source_length == other.source_length
            && self.fade_in_samples == other.fade_in_samples
            && self.fade_out_samples == other.fade_out_samples
            && self.warp_ratio.to_bits() == other.warp_ratio.to_bits()
            && self.source_sample_rate.to_bits() == other.source_sample_rate.to_bits()
            && self.timeline_sample_rate.to_bits() == other.timeline_sample_rate.to_bits()
            && self.clip_gain.to_bits() == other.clip_gain.to_bits()
            && self.pitch_semitones.to_bits() == other.pitch_semitones.to_bits()
            && self.loop_count == other.loop_count
            && self.sync_group == other.sync_group
            && self.muted == other.muted
            && self.reverse == other.reverse
            && self.locked == other.locked
            && self.pitch_preserve_warp == other.pitch_preserve_warp
    }
}

/// Ordered (by start, as the C++ vector is) list of region entries.
#[derive(Default, Debug)]
pub struct RegionTable {
    entries: Vec<RegionEntry>,
}

impl RegionTable {
    pub fn replace(&mut self, entries: &[RegionEntry]) -> bool {
        if entries.len() > HIRARI_REGION_TABLE_MAX {
            return false;
        }
        self.entries.clear();
        self.entries.extend_from_slice(entries);
        true
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&RegionEntry> {
        self.entries.get(index)
    }

    pub fn find(&self, id: u32) -> Option<&RegionEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

#[no_mangle]
pub extern "C" fn hirari_region_table_new() -> *mut RegionTable {
    Box::into_raw(Box::default())
}

/// # Safety
/// `table` must come from `hirari_region_table_new` and not be used again.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_table_free(table: *mut RegionTable) {
    if !table.is_null() {
        drop(unsafe { Box::from_raw(table) });
    }
}

/// Replaces the mirror with `count` entries. Returns false on a null handle,
/// null arrays with a non-zero count, or a count above the table limit.
///
/// # Safety
/// `ids` and `states` must each point to `count` valid elements.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_table_replace(
    table: *mut RegionTable,
    ids: *const u32,
    states: *const HirariRegionState,
    count: usize,
) -> bool {
    let Some(table) = (unsafe { table.as_mut() }) else {
        return false;
    };
    if count == 0 {
        return table.replace(&[]);
    }
    if ids.is_null() || states.is_null() || count > HIRARI_REGION_TABLE_MAX {
        return false;
    }
    let ids = unsafe { std::slice::from_raw_parts(ids, count) };
    let states = unsafe { std::slice::from_raw_parts(states, count) };
    let entries: Vec<RegionEntry> = ids
        .iter()
        .zip(states)
        .map(|(&id, &state)| RegionEntry { id, state })
        .collect();
    table.replace(&entries)
}

/// # Safety
/// `table` must be a live handle or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_table_len(table: *const RegionTable) -> usize {
    unsafe { table.as_ref() }.map_or(0, RegionTable::len)
}

/// Copies entry `index` out. Returns false when out of range or on null.
///
/// # Safety
/// `id` and `state` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_table_get(
    table: *const RegionTable,
    index: usize,
    id: *mut u32,
    state: *mut HirariRegionState,
) -> bool {
    let (Some(table), false, false) = (unsafe { table.as_ref() }, id.is_null(), state.is_null())
    else {
        return false;
    };
    let Some(entry) = table.get(index) else {
        return false;
    };
    unsafe {
        *id = entry.id;
        *state = entry.state;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(start: u64, gain: f32) -> HirariRegionState {
        HirariRegionState {
            start,
            len: 100,
            clip_gain: gain,
            warp_ratio: 1.0,
            loop_count: 1,
            ..Default::default()
        }
    }

    #[test]
    fn replace_roundtrips_in_order_including_duplicate_ids() {
        let table = hirari_region_table_new();
        let ids = [3u32, 7, 3];
        let states = [state(0, 1.0), state(50, 0.5), state(90, 0.25)];
        unsafe {
            assert!(hirari_region_table_replace(table, ids.as_ptr(), states.as_ptr(), 3));
            assert_eq!(hirari_region_table_len(table), 3);
            for index in 0..3 {
                let (mut id, mut out) = (0u32, HirariRegionState::default());
                assert!(hirari_region_table_get(table, index, &mut id, &mut out));
                assert_eq!(id, ids[index]);
                assert_eq!(out, states[index]);
            }
            let (mut id, mut out) = (0u32, HirariRegionState::default());
            assert!(!hirari_region_table_get(table, 3, &mut id, &mut out));
            hirari_region_table_free(table);
        }
    }

    #[test]
    fn replace_with_zero_clears_and_bad_arguments_are_rejected() {
        let table = hirari_region_table_new();
        let ids = [1u32];
        let states = [state(0, 1.0)];
        unsafe {
            assert!(hirari_region_table_replace(table, ids.as_ptr(), states.as_ptr(), 1));
            assert!(hirari_region_table_replace(table, std::ptr::null(), std::ptr::null(), 0));
            assert_eq!(hirari_region_table_len(table), 0);
            assert!(!hirari_region_table_replace(table, std::ptr::null(), states.as_ptr(), 1));
            assert!(!hirari_region_table_replace(
                table,
                ids.as_ptr(),
                states.as_ptr(),
                HIRARI_REGION_TABLE_MAX + 1
            ));
            assert!(!hirari_region_table_replace(std::ptr::null_mut(), ids.as_ptr(), states.as_ptr(), 1));
            hirari_region_table_free(table);
        }
        assert_eq!(unsafe { hirari_region_table_len(std::ptr::null()) }, 0);
    }

    #[test]
    fn find_returns_first_matching_id() {
        let mut table = RegionTable::default();
        assert!(table.replace(&[
            RegionEntry { id: 5, state: state(0, 1.0) },
            RegionEntry { id: 5, state: state(10, 2.0) },
        ]));
        assert_eq!(table.find(5).map(|entry| entry.state.start), Some(0));
        assert!(table.find(9).is_none());
    }
}
