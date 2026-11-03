//! Engine sidechain-route bookkeeping. Routes are project state that the
//! layout writer, track duplication and track removal all read, so Rust owns
//! the list and the engine keeps only the opaque handle.

use std::ffi::c_void;
use std::sync::Mutex;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct HirariSidechainRoute {
    pub source_id: u32,
    pub destination_id: u32,
    pub plugin_index: u32,
    pub tap_point: u32,
}

struct SidechainRouteStore {
    routes: Mutex<Vec<HirariSidechainRoute>>,
}

unsafe fn store(handle: *const c_void) -> Option<&'static SidechainRouteStore> {
    if handle.is_null() {
        None
    } else {
        // SAFETY: handles are allocated and released by this module.
        Some(unsafe { &*handle.cast::<SidechainRouteStore>() })
    }
}

fn with_routes<R>(
    handle: *const c_void,
    fallback: R,
    visit: impl FnOnce(&mut Vec<HirariSidechainRoute>) -> R,
) -> R {
    let Some(store) = (unsafe { store(handle) }) else {
        return fallback;
    };
    let Ok(mut routes) = store.routes.lock() else {
        return fallback;
    };
    visit(&mut routes)
}

#[no_mangle]
pub extern "C" fn hirari_sidechain_routes_create() -> *mut c_void {
    Box::into_raw(Box::new(SidechainRouteStore {
        routes: Mutex::new(Vec::new()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: handle was returned by `hirari_sidechain_routes_create`.
        drop(unsafe { Box::from_raw(handle.cast::<SidechainRouteStore>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_clear(handle: *const c_void) {
    with_routes(handle, (), |routes| routes.clear());
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_count(handle: *const c_void) -> usize {
    with_routes(handle, 0, |routes| routes.len())
}

/// Copies up to `capacity` routes and returns the total count, so a caller can
/// size storage with one call and fill it with the next.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_copy(
    handle: *const c_void,
    out: *mut HirariSidechainRoute,
    capacity: usize,
) -> usize {
    with_routes(handle, 0, |routes| {
        if !out.is_null() {
            let count = routes.len().min(capacity);
            unsafe { std::ptr::copy_nonoverlapping(routes.as_ptr(), out, count) };
        }
        routes.len()
    })
}

/// Upserts the route keyed by destination and plugin index.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_set(
    handle: *const c_void,
    source: u32,
    destination: u32,
    plugin: u32,
    tap: u32,
) -> bool {
    with_routes(handle, false, |routes| {
        for route in routes.iter_mut() {
            if route.destination_id == destination && route.plugin_index == plugin {
                route.source_id = source;
                route.tap_point = tap;
                return true;
            }
        }
        routes.push(HirariSidechainRoute {
            source_id: source,
            destination_id: destination,
            plugin_index: plugin,
            tap_point: tap,
        });
        true
    })
}

/// Drops the route keyed by destination and plugin index.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_remove_link(
    handle: *const c_void,
    destination: u32,
    plugin: u32,
) -> bool {
    with_routes(handle, false, |routes| {
        let before = routes.len();
        routes.retain(|route| {
            route.destination_id != destination || route.plugin_index != plugin
        });
        routes.len() != before
    })
}

/// Drops every route touching the track.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_erase_track(
    handle: *const c_void,
    track: u32,
) -> bool {
    with_routes(handle, false, |routes| {
        let before = routes.len();
        routes.retain(|route| route.source_id != track && route.destination_id != track);
        routes.len() != before
    })
}

/// Removes every route touching the track and returns the removed entries as
/// an owned handle for `hirari_sidechain_routes_restore`.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_capture_track(
    handle: *const c_void,
    track: u32,
) -> *mut c_void {
    let captured = with_routes(handle, Vec::new(), |routes| {
        let mut captured = Vec::new();
        routes.retain(|route| {
            let keep = route.source_id != track && route.destination_id != track;
            if !keep {
                captured.push(*route);
            }
            keep
        });
        captured
    });
    Box::into_raw(Box::new(captured)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_release(captured: *mut c_void) {
    if !captured.is_null() {
        // SAFETY: handled was returned by `hirari_sidechain_routes_capture_track`.
        drop(unsafe { Box::from_raw(captured.cast::<Vec<HirariSidechainRoute>>()) });
    }
}

/// Re-adds captured routes, restoring the routing dependency first and
/// skipping any destination/plugin pair that is already registered.
#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_routes_restore(
    handle: *const c_void,
    routing: *mut c_void,
    captured: *mut c_void,
) {
    if captured.is_null() {
        return;
    }
    // SAFETY: handed back from `hirari_sidechain_routes_capture_track`.
    let captured = unsafe { &*captured.cast::<Vec<HirariSidechainRoute>>() };
    for route in captured {
        let duplicate = with_routes(handle, true, |routes| {
            routes.iter().any(|current| {
                current.destination_id == route.destination_id
                    && current.plugin_index == route.plugin_index
            })
        });
        if duplicate {
            continue;
        }
        let added = unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_dependency(
                routing,
                route.source_id,
                route.destination_id,
            )
        };
        if added {
            with_routes(handle, (), |routes| routes.push(*route));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(handle: *const c_void) -> Vec<HirariSidechainRoute> {
        let count = unsafe { hirari_sidechain_routes_count(handle) };
        let mut routes = vec![
            HirariSidechainRoute {
                source_id: 0,
                destination_id: 0,
                plugin_index: 0,
                tap_point: 0,
            };
            count
        ];
        unsafe { hirari_sidechain_routes_copy(handle, routes.as_mut_ptr(), count) };
        routes
    }

    #[test]
    fn set_upserts_by_destination_and_plugin() {
        let store = hirari_sidechain_routes_create();
        assert!(unsafe { hirari_sidechain_routes_set(store, 2, 3, 0, 1) });
        assert!(unsafe { hirari_sidechain_routes_set(store, 4, 3, 0, 2) });
        assert!(unsafe { hirari_sidechain_routes_set(store, 5, 3, 1, 0) });
        let routes = read(store);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].source_id, 4);
        assert_eq!(routes[0].tap_point, 2);
        unsafe { hirari_sidechain_routes_destroy(store) };
    }

    #[test]
    fn capture_removes_track_routes_and_restore_readds_them() {
        let store = hirari_sidechain_routes_create();
        assert!(unsafe { hirari_sidechain_routes_set(store, 1, 3, 0, 2) });
        assert!(unsafe { hirari_sidechain_routes_set(store, 2, 1, 1, 2) });
        assert!(unsafe { hirari_sidechain_routes_set(store, 2, 4, 0, 2) });
        let captured = unsafe { hirari_sidechain_routes_capture_track(store, 1) };
        assert_eq!(read(store).len(), 1);
        // Restoring without a routing engine keeps the entries absent.
        unsafe { hirari_sidechain_routes_restore(store, std::ptr::null_mut(), captured) };
        assert_eq!(read(store).len(), 1);
        unsafe { hirari_sidechain_routes_release(captured) };
        unsafe { hirari_sidechain_routes_destroy(store) };
    }

    #[test]
    fn remove_link_reports_whether_an_entry_matched() {
        let store = hirari_sidechain_routes_create();
        assert!(!unsafe { hirari_sidechain_routes_remove_link(store, 1, 0) });
        assert!(unsafe { hirari_sidechain_routes_set(store, 1, 2, 0, 2) });
        assert!(unsafe { hirari_sidechain_routes_remove_link(store, 2, 0) });
        assert_eq!(read(store).len(), 0);
        unsafe { hirari_sidechain_routes_destroy(store) };
    }
}
