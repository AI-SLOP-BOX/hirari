// ============================================================
// Per-region processing-step history.
//
// The table is Rust-owned; C++ hands one step in at a time and materializes
// the whole history back out for project serialization. Nothing on the render
// path reads it, so no frozen snapshot or published projection is needed:
// the audio thread never touches this store.
//
// The rules are the ones the two C++ layers used to apply separately, and
// applied incompletely: the engine wrapper checked the parameter was finite,
// the Track mutator did not, and a dead `EventProcessingHistory` class in the
// header carried a third copy with a 128-byte limit instead of 256.
// ============================================================

const STEP_CAP: usize = 4096;
const OPERATION_MAX: usize = 256;

/// One processing step as it crosses the boundary. `operation` is borrowed for
/// the duration of the call and copied into the store.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariEventStep {
    pub id: u32,
    pub operation: *const u8,
    pub operation_len: usize,
    pub parameter: f32,
    pub enabled: bool,
}

/// A step read back out of the store. `operation_offset`/`operation_len`
/// address the flat byte buffer that accompanies the views.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariEventStepView {
    pub id: u32,
    pub parameter: f32,
    pub enabled: bool,
    pub operation_len: usize,
    pub operation_offset: usize,
}

#[derive(Clone, Debug, Default)]
struct Step {
    id: u32,
    operation: String,
    parameter: f32,
    enabled: bool,
}

/// Rust owns the authoritative step table for one region.
#[repr(C)]
pub struct HirariRegionProcessingHistoryStore {
    steps: Vec<Step>,
}

/// Decides whether a step may be appended to a history of `current_len`.
///
/// Rejects a zero id, an empty or oversized operation, a non-finite
/// parameter, and a history that is already at the cap. The id is the
/// caller's: it is the project-facing identity, not a store-allocated
/// sequence, so the caller keeps owning uniqueness.
fn admissible(step: &HirariEventStep, current_len: usize) -> bool {
    if step.id == 0 || current_len >= STEP_CAP {
        return false;
    }
    if step.operation_len == 0 || step.operation_len > OPERATION_MAX {
        return false;
    }
    if !step.parameter.is_finite() {
        return false;
    }
    true
}

/// # Safety
/// `step.operation` must be readable for `step.operation_len` bytes when the
/// length is non-zero.
unsafe fn operation_of(step: &HirariEventStep) -> Option<String> {
    if step.operation_len == 0 {
        return Some(String::new());
    }
    if step.operation.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(step.operation, step.operation_len) };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

impl HirariRegionProcessingHistoryStore {
    fn append(&mut self, step: &HirariEventStep) -> bool {
        if !admissible(step, self.steps.len()) {
            return false;
        }
        let Some(operation) = (unsafe { operation_of(step) }) else {
            return false;
        };
        self.steps.push(Step {
            id: step.id,
            operation,
            parameter: step.parameter,
            enabled: step.enabled,
        });
        true
    }

    fn total_operation_bytes(&self) -> usize {
        self.steps.iter().map(|step| step.operation.len()).sum()
    }
}

/// Create an empty Rust-owned history.
///
/// # Safety
/// The caller owns the returned pointer and must release it with
/// `hirari_region_processing_history_clear` exactly once.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_new(
) -> *mut HirariRegionProcessingHistoryStore {
    Box::into_raw(Box::new(HirariRegionProcessingHistoryStore {
        steps: Vec::new(),
    }))
}

/// Release a history.
///
/// # Safety
/// `store` must be null or come from `hirari_region_processing_history_new`,
/// and no other pointer into it may be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_clear(
    store: *mut HirariRegionProcessingHistoryStore,
) {
    if store.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(store) });
}

/// Drop every step without freeing the store.
///
/// # Safety
/// `store` must be a live store.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_reset(
    store: *mut HirariRegionProcessingHistoryStore,
) {
    if store.is_null() {
        return;
    }
    (*store).steps.clear();
}

/// Append one step. Returns false when the step is not admissible, leaving
/// the history untouched.
///
/// # Safety
/// `store` must be a live store. When `operation_len > 0`, `operation` must
/// be readable for that many bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_append(
    store: *mut HirariRegionProcessingHistoryStore,
    step: HirariEventStep,
) -> bool {
    if store.is_null() {
        return false;
    }
    (*store).append(&step)
}

/// Number of stored steps.
///
/// # Safety
/// `store` must be null or a live store.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_count(
    store: *const HirariRegionProcessingHistoryStore,
) -> usize {
    if store.is_null() {
        return 0;
    }
    (*store).steps.len()
}

/// Step count and total operation bytes, so a caller can allocate exactly
/// once before materializing.
///
/// # Safety
/// Both out-parameters must be writable. `store` may be null.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_sizes(
    store: *const HirariRegionProcessingHistoryStore,
    out_count: *mut usize,
    out_byte_total: *mut usize,
) -> bool {
    if out_count.is_null() || out_byte_total.is_null() {
        return false;
    }
    if store.is_null() {
        *out_count = 0;
        *out_byte_total = 0;
        return true;
    }
    let store = unsafe { &*store };
    *out_count = store.steps.len();
    *out_byte_total = store.total_operation_bytes();
    true
}

/// Materialize the history for serialization: one view per step plus a flat
/// buffer of operation bytes that the views address.
///
/// # Safety
/// `store` must be a live store. On success `out_views` needs `count` elements
/// and `out_bytes` the returned byte total. Both out-parameters must be
/// writable. Counts are only written once every capacity check has passed.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processing_history_materialize(
    store: *const HirariRegionProcessingHistoryStore,
    out_views: *mut HirariEventStepView,
    view_capacity: usize,
    out_bytes: *mut u8,
    byte_capacity: usize,
    out_count: *mut usize,
    out_byte_total: *mut usize,
) -> bool {
    if out_count.is_null() || out_byte_total.is_null() {
        return false;
    }
    if store.is_null() {
        *out_count = 0;
        *out_byte_total = 0;
        return true;
    }
    let store = unsafe { &*store };
    let needed_bytes = store.total_operation_bytes();
    if store.steps.len() > view_capacity || needed_bytes > byte_capacity {
        return false;
    }
    if !store.steps.is_empty() && out_views.is_null() {
        return false;
    }
    if needed_bytes > 0 && out_bytes.is_null() {
        return false;
    }
    let mut written = 0usize;
    for (index, step) in store.steps.iter().enumerate() {
        unsafe {
            *out_views.add(index) = HirariEventStepView {
                id: step.id,
                parameter: step.parameter,
                enabled: step.enabled,
                operation_len: step.operation.len(),
                operation_offset: written,
            };
        }
        let bytes = step.operation.as_bytes();
        if !bytes.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_bytes.add(written), bytes.len())
            };
            written += bytes.len();
        }
    }
    unsafe {
        *out_count = store.steps.len();
        *out_byte_total = needed_bytes;
    }
    true
}

#[cfg(test)]
mod processing_history_tests {
    use super::*;

    fn step(id: u32, operation: &str, parameter: f32, enabled: bool) -> HirariEventStep {
        HirariEventStep {
            id,
            operation: operation.as_ptr(),
            operation_len: operation.len(),
            parameter,
            enabled,
        }
    }

    fn store() -> *mut HirariRegionProcessingHistoryStore {
        unsafe { hirari_region_processing_history_new() }
    }

    fn view(store: *const HirariRegionProcessingHistoryStore) -> Vec<(u32, String, f32, bool)> {
        let mut count = 0usize;
        let mut bytes_total = 0usize;
        assert!(unsafe {
            hirari_region_processing_history_sizes(store, &mut count, &mut bytes_total)
        });
        let mut views = vec![HirariEventStepView::default(); count];
        let mut bytes = vec![0u8; bytes_total];
        let mut written_count = 0usize;
        let mut written_bytes = 0usize;
        assert!(unsafe {
            hirari_region_processing_history_materialize(
                store,
                views.as_mut_ptr(),
                views.len(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut written_count,
                &mut written_bytes,
            )
        });
        assert_eq!(written_count, count);
        assert_eq!(written_bytes, bytes_total);
        views
            .into_iter()
            .map(|view| {
                let operation = String::from_utf8_lossy(
                    &bytes[view.operation_offset..view.operation_offset + view.operation_len],
                )
                .into_owned();
                (view.id, operation, view.parameter, view.enabled)
            })
            .collect()
    }

    #[test]
    fn appends_and_materializes_in_order() {
        let store = store();
        assert!(unsafe { hirari_region_processing_history_append(store, step(1, "gain", 0.5, true)) });
        assert!(unsafe {
            hirari_region_processing_history_append(store, step(2, "normalize", -1.25, false))
        });
        assert_eq!(
            view(store),
            vec![
                (1, "gain".to_string(), 0.5, true),
                (2, "normalize".to_string(), -1.25, false),
            ]
        );
        unsafe { hirari_region_processing_history_clear(store) };
    }

    #[test]
    fn rejects_the_policy_the_two_c_layers_disagreed_on() {
        let store = store();
        // Zero id, empty and oversized operations, non-finite parameter.
        assert!(!unsafe { hirari_region_processing_history_append(store, step(0, "gain", 0.0, true)) });
        assert!(!unsafe { hirari_region_processing_history_append(store, step(1, "", 0.0, true)) });
        let long = "x".repeat(OPERATION_MAX + 1);
        assert!(!unsafe {
            hirari_region_processing_history_append(store, step(1, &long, 0.0, true))
        });
        // The Track mutator never checked this; only the engine wrapper did.
        assert!(!unsafe {
            hirari_region_processing_history_append(store, step(1, "gain", f32::NAN, true))
        });
        assert_eq!(
            unsafe { hirari_region_processing_history_count(store) },
            0,
            "a rejected batch must leave the history untouched"
        );
        // 256 is the boundary and stays admissible.
        let boundary = "x".repeat(OPERATION_MAX);
        assert!(unsafe {
            hirari_region_processing_history_append(store, step(1, &boundary, 0.0, true))
        });
        unsafe { hirari_region_processing_history_clear(store) };
    }

    #[test]
    fn caps_at_4096_steps() {
        let store = store();
        for index in 0..STEP_CAP {
            assert!(unsafe {
                hirari_region_processing_history_append(store, step(index as u32 + 1, "g", 0.0, true))
            });
        }
        assert!(!unsafe {
            hirari_region_processing_history_append(store, step(u32::MAX, "g", 0.0, true))
        });
        assert_eq!(
            unsafe { hirari_region_processing_history_count(store) },
            STEP_CAP
        );
        unsafe { hirari_region_processing_history_clear(store) };
    }

    #[test]
    fn reset_drops_steps_without_freeing_the_store() {
        let store = store();
        assert!(unsafe { hirari_region_processing_history_append(store, step(1, "gain", 0.0, true)) });
        unsafe { hirari_region_processing_history_reset(store) };
        assert_eq!(unsafe { hirari_region_processing_history_count(store) }, 0);
        // The store is still usable afterwards.
        assert!(unsafe { hirari_region_processing_history_append(store, step(2, "gain", 0.0, true)) });
        assert_eq!(view(store).len(), 1);
        unsafe { hirari_region_processing_history_clear(store) };
    }

    #[test]
    fn materialize_reports_failure_without_touching_the_counts() {
        let store = store();
        assert!(unsafe { hirari_region_processing_history_append(store, step(1, "gain", 0.0, true)) });
        let mut count = 7usize;
        let mut bytes_total = 7usize;
        // One view slot but the step needs one: capacity is short.
        assert!(!unsafe {
            hirari_region_processing_history_materialize(
                store,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                0,
                &mut count,
                &mut bytes_total,
            )
        });
        assert_eq!((count, bytes_total), (7, 7), "counts must survive a failure");
        unsafe { hirari_region_processing_history_clear(store) };
    }

    #[test]
    fn null_store_is_a_valid_empty_history() {
        let mut count = 9usize;
        let mut bytes_total = 9usize;
        assert!(unsafe {
            hirari_region_processing_history_sizes(std::ptr::null(), &mut count, &mut bytes_total)
        });
        assert_eq!((count, bytes_total), (0, 0));
        unsafe { hirari_region_processing_history_clear(std::ptr::null_mut()) };
    }
}
