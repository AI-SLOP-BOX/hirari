//! Rust-owned arrangement timeline document: markers and arranger parts.
//!
//! The engine only holds an opaque handle. Marker ordering/deduplication,
//! arranger-part validation and ordering, and the storage itself live here so
//! no C++ model has to mirror the project timeline.

use std::ffi::c_void;
use std::sync::Mutex;

#[derive(Clone)]
struct Marker {
    sample: u64,
    name: Vec<u8>,
    color: u32,
}

#[derive(Clone)]
struct ArrangerPart {
    start: u64,
    length: u64,
    name: Vec<u8>,
    repeats: u32,
}

#[derive(Default)]
struct Document {
    markers: Vec<Marker>,
    parts: Vec<ArrangerPart>,
}

pub struct ArrangementDocument {
    document: Mutex<Document>,
}

impl ArrangementDocument {
    fn new() -> Self {
        Self {
            document: Mutex::new(Document::default()),
        }
    }
}

#[repr(C)]
pub struct HirariArrangementPartInput {
    pub start: u64,
    pub length: u64,
    pub name: *const u8,
    pub name_len: usize,
    pub repeats: u32,
}

/// Copies the document for the project-save encoder. Returning owned copies
/// keeps the lock duration bounded and lets the encoder run without holding
/// the document mutex.
pub fn snapshot(
    handle: *mut std::ffi::c_void,
) -> Option<(
    Vec<(u64, Vec<u8>, u32)>,
    Vec<(u64, u64, Vec<u8>, u32)>,
)> {
    let doc = document(handle)?;
    let state = doc.document.lock().ok()?;
    Some((
        state
            .markers
            .iter()
            .map(|marker| (marker.sample, marker.name.clone(), marker.color))
            .collect(),
        state
            .parts
            .iter()
            .map(|part| {
                (
                    part.start,
                    part.length,
                    part.name.clone(),
                    part.repeats,
                )
            })
            .collect(),
    ))
}

fn document(handle: *mut c_void) -> Option<&'static ArrangementDocument> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &*(handle.cast::<ArrangementDocument>()) })
}

fn copy_name(data: *const u8, len: usize) -> Option<Vec<u8>> {
    if len == 0 || data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) }.to_vec())
}

#[no_mangle]
pub extern "C" fn hirari_arrangement_document_create() -> *mut c_void {
    Box::into_raw(Box::new(ArrangementDocument::new())).cast()
}

/// # Safety
/// `handle` must come from `hirari_arrangement_document_create` and not be
/// used afterwards.
#[no_mangle]
pub unsafe extern "C" fn hirari_arrangement_document_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<ArrangementDocument>())) };
    }
}

/// # Safety
/// `handle` must be a live document handle.
#[no_mangle]
pub unsafe extern "C" fn hirari_arrangement_document_reset_markers(handle: *mut c_void) {
    if let Some(doc) = document(handle) {
        if let Ok(mut state) = doc.document.lock() {
            state.markers.clear();
        }
    }
}

/// Inserts or replaces the marker at `sample`. A marker without a name is
/// rejected: the timeline has no way to address an unnamed marker.
///
/// # Safety
/// `name` must point to `name_len` readable bytes when `name_len` is nonzero.
#[no_mangle]
pub unsafe extern "C" fn hirari_arrangement_document_upsert_marker(
    handle: *mut c_void,
    sample: u64,
    name: *const u8,
    name_len: usize,
    color: u32,
) -> bool {
    let Some(doc) = document(handle) else {
        return false;
    };
    let Some(name) = copy_name(name, name_len) else {
        return false;
    };
    let Ok(mut state) = doc.document.lock() else {
        return false;
    };
    let marker = Marker {
        sample,
        name,
        color,
    };
    match state.markers.binary_search_by_key(&sample, |entry| entry.sample) {
        Ok(index) => state.markers[index] = marker,
        Err(index) => state.markers.insert(index, marker),
    }
    true
}

/// Replaces every arranger part. Parts must have a positive length, at least
/// one repeat and a name; the stored order is by start position.
///
/// # Safety
/// `parts` must point to `count` valid records.
#[no_mangle]
pub unsafe extern "C" fn hirari_arrangement_document_set_parts(
    handle: *mut c_void,
    parts: *const HirariArrangementPartInput,
    count: usize,
) -> bool {
    let Some(doc) = document(handle) else {
        return false;
    };
    if count != 0 && parts.is_null() {
        return false;
    }
    let mut converted = Vec::with_capacity(count);
    let entries: &[HirariArrangementPartInput] = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(parts, count) }
    };
    for part in entries {
        let Some(name) = copy_name(part.name, part.name_len) else {
            return false;
        };
        if part.length == 0 || part.repeats == 0 {
            return false;
        }
        converted.push(ArrangerPart {
            start: part.start,
            length: part.length,
            name,
            repeats: part.repeats,
        });
    }
    converted.sort_by_key(|part| part.start);
    let Ok(mut state) = doc.document.lock() else {
        return false;
    };
    state.parts = converted;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_keep_order_and_replace_by_sample() {
        let handle = hirari_arrangement_document_create();
        unsafe {
            assert!(hirari_arrangement_document_upsert_marker(
                handle,
                200,
                b"b".as_ptr(),
                1,
                0
            ));
            assert!(hirari_arrangement_document_upsert_marker(
                handle,
                100,
                b"a".as_ptr(),
                1,
                7
            ));
            assert!(hirari_arrangement_document_upsert_marker(
                handle,
                200,
                b"c".as_ptr(),
                1,
                9
            ));
            let (markers, _) = snapshot(handle).unwrap();
            assert_eq!(markers.len(), 2);
            assert_eq!(markers[0], (100, b"a".to_vec(), 7));
            assert_eq!(markers[1], (200, b"c".to_vec(), 9));
            hirari_arrangement_document_destroy(handle);
        }
    }

    #[test]
    fn unnamed_markers_are_rejected() {
        let handle = hirari_arrangement_document_create();
        unsafe {
            assert!(!hirari_arrangement_document_upsert_marker(
                handle,
                1,
                std::ptr::null(),
                0,
                0
            ));
            assert_eq!(snapshot(handle).unwrap().0.len(), 0);
            hirari_arrangement_document_destroy(handle);
        }
    }

    #[test]
    fn parts_are_validated_and_sorted() {
        let handle = hirari_arrangement_document_create();
        let later = HirariArrangementPartInput {
            start: 480,
            length: 240,
            name: b"chorus".as_ptr(),
            name_len: 6,
            repeats: 2,
        };
        let earlier = HirariArrangementPartInput {
            start: 0,
            length: 480,
            name: b"verse".as_ptr(),
            name_len: 5,
            repeats: 1,
        };
        unsafe {
            let parts = [later, earlier];
            assert!(hirari_arrangement_document_set_parts(
                handle,
                parts.as_ptr(),
                parts.len()
            ));
            let parts = snapshot(handle).unwrap().1;
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0], (0, 480, b"verse".to_vec(), 1));
            assert_eq!(parts[1], (480, 240, b"chorus".to_vec(), 2));

            let invalid = HirariArrangementPartInput {
                start: 0,
                length: 0,
                name: b"empty".as_ptr(),
                name_len: 5,
                repeats: 1,
            };
            assert!(!hirari_arrangement_document_set_parts(handle, &invalid, 1));
            // A rejected list leaves the previous parts untouched.
            assert_eq!(snapshot(handle).unwrap().1.len(), 2);
            hirari_arrangement_document_destroy(handle);
        }
    }
}
