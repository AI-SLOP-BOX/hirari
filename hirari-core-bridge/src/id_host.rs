//! Track and region id allocation for the whole project.
//!
//! ID 0 is the invalid/stale sentinel across the bridge: document validation
//! rejects it and the argument guards treat it as "no track", so the starter
//! track takes ID 1 and user tracks continue at 2. `u32::MAX` is reserved the
//! same way, which is why the walk back to 1 happens one step before it.
//!
//! The engine keeps the tracks and regions and answers which ids it already
//! holds; this module keeps the counters and decides the order ids are handed
//! out, including what a new or loaded project restarts them at.
//!
//! Every entry point runs on the control thread and must not be called while
//! the engine holds its project topology lock — the listings take it.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::region_edit_host::{HirariRegionListRegionsFn, HirariRegionListTracksFn};

/// The first track id a fresh project hands out is 2: ID 1 belongs to the
/// starter track that `new_project()` installs.
const TRACK_COUNTER_AFTER_RESET: u32 = 2;
/// Region ids restart with the project, and zero is not a usable region id.
const REGION_COUNTER_AFTER_RESET: u32 = 1;

/// The id a counter resumes from after handing out `id`.
fn next_counter(id: u32) -> u32 {
    id.checked_add(1).unwrap_or(u32::MAX)
}

#[derive(Clone, Copy, Default)]
struct Counters {
    /// Default 0 means unset: the allocator starts at 1, the same way the
    /// engine's counter did before this moved.
    track: u32,
    region: u32,
}

fn counters() -> &'static Mutex<HashMap<usize, Counters>> {
    static COUNTERS: OnceLock<Mutex<HashMap<usize, Counters>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_counters<T>(host: *mut c_void, edit: impl FnOnce(&mut Counters) -> T) -> T {
    let mut map = match counters().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    edit(map.entry(host as usize).or_default())
}

/// Lists the track ids in storage order, or `None` when the host cannot answer.
fn listed_tracks(
    list: Option<HirariRegionListTracksFn>,
    host: *mut c_void,
) -> Option<Vec<u32>> {
    let list = list?;
    let count = unsafe { list(host, std::ptr::null_mut(), 0) };
    if count == 0 {
        return Some(Vec::new());
    }
    let mut ids = vec![0u32; count];
    let written = unsafe { list(host, ids.as_mut_ptr(), count) };
    ids.truncate(written.min(count));
    Some(ids)
}

fn listed_regions(
    list: Option<HirariRegionListRegionsFn>,
    host: *mut c_void,
    track_id: u32,
) -> Option<Vec<u32>> {
    let list = list?;
    let count = unsafe { list(host, track_id, std::ptr::null_mut(), 0) };
    if count == 0 {
        return Some(Vec::new());
    }
    let mut ids = vec![0u32; count];
    let written = unsafe { list(host, track_id, ids.as_mut_ptr(), count) };
    ids.truncate(written.min(count));
    Some(ids)
}

/// Takes the first free id at or after `counter`. An unset counter starts at 1,
/// reserved ids are skipped and the walk wraps back to 1 before it can reach
/// the `u32::MAX` sentinel. `None` when the range holds no free id.
fn take_one(counter: u32, used: &HashSet<u32>) -> Option<(u32, u32)> {
    let mut candidate = if counter == 0 { 1 } else { counter };
    for _ in 0..u32::MAX {
        if candidate != 0 && candidate != u32::MAX && !used.contains(&candidate) {
            return Some((candidate, next_counter(candidate)));
        }
        candidate = if candidate >= u32::MAX - 1 {
            1
        } else {
            candidate + 1
        };
    }
    None
}

/// Takes `count` ids in allocation order; ids already taken by this batch count
/// as used, so a batch never repeats one.
fn take_ids(counter: u32, used: &HashSet<u32>, count: usize) -> Option<(Vec<u32>, u32)> {
    let mut free = used.clone();
    let mut ids = Vec::with_capacity(count);
    let mut candidate = counter;
    for _ in 0..count {
        let (id, next) = take_one(candidate, &free)?;
        free.insert(id);
        ids.push(id);
        candidate = next;
    }
    Some((ids, candidate))
}

/// Hands out one track id, or 0 when the project has none left.
pub fn next_track_id(list_tracks: Option<HirariRegionListTracksFn>, host: *mut c_void) -> u32 {
    if host.is_null() {
        return 0;
    }
    let used: HashSet<u32> = match listed_tracks(list_tracks, host) {
        Some(ids) => ids.into_iter().collect(),
        None => return 0,
    };
    with_counters(host, |counters| {
        let (id, next) = match take_one(counters.track, &used) {
            Some(pair) => pair,
            None => return 0,
        };
        counters.track = next;
        id
    })
}

/// Hands out `count` region ids; empty when the project has too few left. The
/// caller already knows the count it needs, so an empty result for a zero count
/// is not a failure.
pub fn next_region_ids(
    list_tracks: Option<HirariRegionListTracksFn>,
    list_regions: Option<HirariRegionListRegionsFn>,
    host: *mut c_void,
    count: usize,
) -> Vec<u32> {
    if host.is_null() || count == 0 {
        return Vec::new();
    }
    let tracks = match listed_tracks(list_tracks, host) {
        Some(ids) => ids,
        None => return Vec::new(),
    };
    let mut used = HashSet::new();
    for track in tracks {
        match listed_regions(list_regions, host, track) {
            Some(ids) => used.extend(ids),
            None => return Vec::new(),
        }
    }
    with_counters(host, |counters| match take_ids(counters.region, &used, count) {
        Some((ids, next)) => {
            counters.region = next;
            ids
        }
        None => Vec::new(),
    })
}

/// Hands out one region id, or 0 when the project has none left.
pub fn next_region_id(
    list_tracks: Option<HirariRegionListTracksFn>,
    list_regions: Option<HirariRegionListRegionsFn>,
    host: *mut c_void,
) -> u32 {
    next_region_ids(list_tracks, list_regions, host, 1)
        .into_iter()
        .next()
        .unwrap_or(0)
}

/// How many regions a track holds; 0 when the host cannot answer.
pub fn region_count(
    list_regions: Option<HirariRegionListRegionsFn>,
    host: *mut c_void,
    track_id: u32,
) -> usize {
    if host.is_null() {
        return 0;
    }
    listed_regions(list_regions, host, track_id)
        .map(|ids| ids.len())
        .unwrap_or_default()
}

/// Rewinds the track counter when the bootstrap placeholder (ID 1) was the last
/// track in the project, so the first track a document installs keeps its
/// canonical ID instead of becoming ID 2.
pub fn rewind_track_counter(list_tracks: Option<HirariRegionListTracksFn>, host: *mut c_void) {
    if host.is_null() {
        return;
    }
    if matches!(listed_tracks(list_tracks, host), Some(ids) if ids.is_empty()) {
        with_counters(host, |counters| counters.track = 1);
    }
}

/// Restarts both counters for a project that has just been emptied; the caller
/// installs the starter track at ID 1 itself.
///
/// # Safety
/// `host` is the engine pointer the installed region host callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_ids_reset_project(host: *mut c_void) {
    if host.is_null() {
        return;
    }
    with_counters(host, |counters| {
        counters.track = TRACK_COUNTER_AFTER_RESET;
        counters.region = REGION_COUNTER_AFTER_RESET;
    });
}

/// Resumes allocation after a document has been installed, one past the highest
/// track and region ids the loaded project carries.
pub fn seed_from_project(
    list_tracks: Option<HirariRegionListTracksFn>,
    list_regions: Option<HirariRegionListRegionsFn>,
    host: *mut c_void,
) {
    if host.is_null() {
        return;
    }
    let Some(tracks) = listed_tracks(list_tracks, host) else {
        return;
    };
    let mut next_track = 1;
    let mut next_region = 1;
    for track in tracks {
        next_track = next_track.max(next_counter(track));
        let Some(regions) = listed_regions(list_regions, host, track) else {
            return;
        };
        for region in regions {
            next_region = next_region.max(next_counter(region));
        }
    }
    with_counters(host, |counters| {
        counters.track = next_track;
        counters.region = next_region;
    });
}

/// # Safety
/// `host` is the engine pointer the installed region host callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_ids_seed_from_project(host: *mut c_void) {
    let Some(ops) = crate::region_edit_host::host_ops() else {
        return;
    };
    seed_from_project(ops.list_tracks, ops.list_regions, host);
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());
    static TRACKS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    static REGIONS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

    fn tracks() -> &'static Mutex<Vec<u32>> {
        TRACKS.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn regions() -> &'static Mutex<Vec<u32>> {
        REGIONS.get_or_init(|| Mutex::new(Vec::new()))
    }

    unsafe extern "C" fn list_tracks(host: *mut c_void, out: *mut u32, capacity: usize) -> usize {
        let ids = tracks().lock().unwrap().clone();
        fill(&ids, host, out, capacity)
    }

    unsafe extern "C" fn list_regions(
        host: *mut c_void,
        _track_id: u32,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let ids = regions().lock().unwrap().clone();
        fill(&ids, host, out, capacity)
    }

    fn fill(ids: &[u32], host: *mut c_void, out: *mut u32, capacity: usize) -> usize {
        assert!(!host.is_null());
        if out.is_null() || capacity < ids.len() {
            return ids.len();
        }
        unsafe { std::ptr::copy_nonoverlapping(ids.as_ptr(), out, ids.len()) };
        ids.len()
    }

    fn used(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    #[test]
    fn unset_counters_start_at_one_and_reserved_ids_end_the_range() {
        assert_eq!(take_one(0, &used(&[])), Some((1, 2)));
        assert_eq!(take_one(u32::MAX - 1, &used(&[])), Some((u32::MAX - 1, u32::MAX)));
        // A counter at the sentinel wraps to 1 rather than handing it out.
        assert_eq!(take_one(u32::MAX, &used(&[1, 2])), Some((3, 4)));
        assert_eq!(take_one(1, &used(&[1, 2, 3])), Some((4, 5)));
    }

    #[test]
    fn a_batch_never_repeats_an_id_and_leaves_the_counter_after_the_last() {
        let (ids, next) = take_ids(1, &used(&[2]), 3).unwrap();
        assert_eq!(ids, vec![1, 3, 4]);
        assert_eq!(next, 5);
        assert_eq!(take_ids(1, &used(&[1, 3]), 2).unwrap(), (vec![2, 4], 5));
        assert_eq!(take_ids(1, &used(&[]), 0).unwrap(), (Vec::new(), 1));
    }

    #[test]
    fn track_ids_skip_what_the_project_holds() {
        let _guard = SERIAL.lock().unwrap();
        let host = 0x1000_usize as *mut c_void;
        *tracks().lock().unwrap() = vec![1, 2, 5];
        unsafe { hirari_ids_reset_project(host) };
        assert_eq!(next_track_id(Some(list_tracks), host), 3);
        *tracks().lock().unwrap() = vec![1, 2, 3, 5];
        assert_eq!(next_track_id(Some(list_tracks), host), 4);
        assert_eq!(next_track_id(None, host), 0);
        assert_eq!(next_track_id(Some(list_tracks), std::ptr::null_mut()), 0);
    }

    #[test]
    fn region_ids_advance_with_every_allocation() {
        let _guard = SERIAL.lock().unwrap();
        let host = 0x2000_usize as *mut c_void;
        *tracks().lock().unwrap() = vec![1];
        *regions().lock().unwrap() = vec![1, 2];
        unsafe { hirari_ids_reset_project(host) };
        assert_eq!(next_region_ids(Some(list_tracks), Some(list_regions), host, 3), vec![3, 4, 5]);
        assert_eq!(next_region_id(Some(list_tracks), Some(list_regions), host), 6);
        assert_eq!(region_count(Some(list_regions), host, 1), 2);
        assert_eq!(
            next_region_ids(Some(list_tracks), Some(list_regions), host, 0),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn seeding_resumes_past_the_installed_project_and_reset_restarts_it() {
        let _guard = SERIAL.lock().unwrap();
        let host = 0x3000_usize as *mut c_void;
        *tracks().lock().unwrap() = vec![4, 9];
        *regions().lock().unwrap() = vec![7];
        seed_from_project(Some(list_tracks), Some(list_regions), host);
        assert_eq!(next_track_id(Some(list_tracks), host), 10);
        assert_eq!(next_region_id(Some(list_tracks), Some(list_regions), host), 8);
        unsafe { hirari_ids_reset_project(host) };
        assert_eq!(next_track_id(Some(list_tracks), host), 2);
        assert_eq!(next_region_id(Some(list_tracks), Some(list_regions), host), 1);
    }

    #[test]
    fn an_emptied_project_rewinds_the_track_counter_only_when_it_is_empty() {
        let _guard = SERIAL.lock().unwrap();
        let host = 0x4000_usize as *mut c_void;
        *tracks().lock().unwrap() = vec![1];
        rewind_track_counter(Some(list_tracks), host);
        assert_eq!(next_track_id(Some(list_tracks), host), 2);
        *tracks().lock().unwrap() = Vec::new();
        rewind_track_counter(Some(list_tracks), host);
        assert_eq!(next_track_id(Some(list_tracks), host), 1);
        assert_eq!(next_track_id(Some(list_tracks), host), 2);
    }
}
