use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

/// Rust-owned scalar parameter registry used by the native compatibility API.
pub struct ParamTreeOrchestrator {
    params: Mutex<HashMap<u32, f32>>,
}

impl Default for ParamTreeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamTreeOrchestrator {
    pub fn new() -> Self {
        Self {
            params: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_param(&self, id: u32, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        self.params
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, value.clamp(0.0, 1.0));
        true
    }

    pub fn get_param(&self, id: u32, fallback: f32) -> f32 {
        let fallback = if fallback.is_finite() {
            fallback.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.params
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .copied()
            .unwrap_or(fallback)
    }

    pub fn clear(&self) {
        self.params
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn audit_param_tree(&self) -> bool {
        self.params
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
    }
}

#[no_mangle]
pub extern "C" fn hirari_param_tree_create() -> *mut c_void {
    Box::into_raw(Box::new(ParamTreeOrchestrator::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_param_tree_destroy(tree: *mut c_void) {
    if !tree.is_null() {
        drop(Box::from_raw(tree.cast::<ParamTreeOrchestrator>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_param_tree_set(tree: *const c_void, id: u32, value: f32) -> bool {
    tree.cast::<ParamTreeOrchestrator>()
        .as_ref()
        .is_some_and(|tree| tree.set_param(id, value))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_param_tree_get(tree: *const c_void, id: u32, fallback: f32) -> f32 {
    let fallback = if fallback.is_finite() {
        fallback.clamp(0.0, 1.0)
    } else {
        0.0
    };
    tree.cast::<ParamTreeOrchestrator>()
        .as_ref()
        .map_or(fallback, |tree| tree.get_param(id, fallback))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_param_tree_clear(tree: *const c_void) {
    if let Some(tree) = tree.cast::<ParamTreeOrchestrator>().as_ref() {
        tree.clear();
    }
}
