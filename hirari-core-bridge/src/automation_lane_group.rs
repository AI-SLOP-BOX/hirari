use std::collections::HashMap;
use std::ffi::c_void;

#[derive(Default)]
struct LaneState {
    linked: bool,
    protected_lane: bool,
    preview: bool,
    preview_value: f32,
}

#[derive(Default)]
struct LaneGroup {
    lanes: HashMap<u32, LaneState>,
}

fn group<'a>(handle: *mut c_void) -> Option<&'a mut LaneGroup> {
    if handle.is_null() {
        return None;
    }
    unsafe { handle.cast::<LaneGroup>().as_mut() }
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_create() -> *mut c_void {
    Box::into_raw(Box::new(LaneGroup::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_lane_group_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<LaneGroup>()));
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_add(handle: *mut c_void, id: u32) -> bool {
    let Some(group) = group(handle) else {
        return false;
    };
    if id == 0 {
        return false;
    }
    group.lanes.entry(id).or_default();
    true
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_remove(handle: *mut c_void, id: u32) -> bool {
    let Some(group) = group(handle) else {
        return false;
    };
    group.lanes.remove(&id).is_some()
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_link(
    handle: *mut c_void,
    id: u32,
    linked: bool,
) -> bool {
    let Some(group) = group(handle) else {
        return false;
    };
    match group.lanes.get_mut(&id) {
        Some(lane) => {
            lane.linked = linked;
            true
        }
        None => false,
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_protect(
    handle: *mut c_void,
    id: u32,
    protected_lane: bool,
) -> bool {
    let Some(group) = group(handle) else {
        return false;
    };
    match group.lanes.get_mut(&id) {
        Some(lane) => {
            lane.protected_lane = protected_lane;
            true
        }
        None => false,
    }
}

#[no_mangle]
pub extern "C" fn hirari_automation_lane_group_preview(
    handle: *mut c_void,
    id: u32,
    value: f32,
) -> bool {
    let Some(group) = group(handle) else {
        return false;
    };
    match group.lanes.get_mut(&id) {
        Some(lane) if !lane.protected_lane => {
            lane.preview = true;
            lane.preview_value = value;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_flags_and_preview_follow_the_protection_rule() {
        let handle = hirari_automation_lane_group_create();
        assert!(!handle.is_null());
        assert!(!hirari_automation_lane_group_add(handle, 0));
        assert!(hirari_automation_lane_group_add(handle, 7));
        assert!(hirari_automation_lane_group_add(handle, 7));
        assert!(!hirari_automation_lane_group_link(handle, 8, true));
        assert!(hirari_automation_lane_group_link(handle, 7, true));
        assert!(hirari_automation_lane_group_preview(handle, 7, 0.25));
        assert!(hirari_automation_lane_group_protect(handle, 7, true));
        assert!(!hirari_automation_lane_group_preview(handle, 7, 0.5));
        assert!(!hirari_automation_lane_group_protect(handle, 8, true));
        assert!(hirari_automation_lane_group_remove(handle, 7));
        assert!(!hirari_automation_lane_group_remove(handle, 7));
        unsafe { hirari_automation_lane_group_destroy(handle) };
    }

    #[test]
    fn null_handles_report_failure_instead_of_panicking() {
        assert!(!hirari_automation_lane_group_add(std::ptr::null_mut(), 1));
        assert!(!hirari_automation_lane_group_remove(std::ptr::null_mut(), 1));
        assert!(!hirari_automation_lane_group_link(
            std::ptr::null_mut(),
            1,
            true
        ));
        assert!(!hirari_automation_lane_group_preview(
            std::ptr::null_mut(),
            1,
            0.0
        ));
        unsafe { hirari_automation_lane_group_destroy(std::ptr::null_mut()) };
    }
}
