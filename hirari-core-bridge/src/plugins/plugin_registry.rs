use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCompatibility {
    pub plugin_id: String,
    pub scan_failures: u32,
    pub crashes: u32,
    pub blacklisted: bool,
    pub last_error: String,
}
impl PluginCompatibility {
    pub fn validate(&self) -> bool {
        !self.plugin_id.trim().is_empty()
            && self.plugin_id.len() <= 256
            && !self.plugin_id.contains('\0')
            && self.last_error.len() <= 4096
            && !self.last_error.contains('\0')
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginRegistry {
    pub entries: Vec<PluginCompatibility>,
}
impl PluginRegistry {
    pub fn record_scan_failure(&mut self, plugin_id: &str, error: &str) -> bool {
        self.update(plugin_id, error, true, false)
    }
    pub fn record_crash(&mut self, plugin_id: &str, error: &str) -> bool {
        self.update(plugin_id, error, false, true)
    }

    pub fn record_crash_preserving_error(&mut self, plugin_id: &str) -> bool {
        let previous_error = self
            .entries
            .iter()
            .find(|entry| entry.plugin_id.eq_ignore_ascii_case(plugin_id.trim()))
            .map(|entry| entry.last_error.clone())
            .unwrap_or_default();
        self.update(plugin_id, &previous_error, false, true)
    }
    fn update(&mut self, plugin_id: &str, error: &str, scan: bool, crash: bool) -> bool {
        let plugin_id = plugin_id.trim();
        if plugin_id.is_empty()
            || plugin_id.len() > 256
            || plugin_id.contains('\0')
            || error.len() > 4096
            || error.contains('\0')
        {
            return false;
        }
        let entry = if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.plugin_id.eq_ignore_ascii_case(plugin_id))
        {
            entry
        } else {
            if self.entries.len() >= 65_536 {
                return false;
            }
            self.entries.push(PluginCompatibility {
                plugin_id: plugin_id.into(),
                scan_failures: 0,
                crashes: 0,
                blacklisted: false,
                last_error: String::new(),
            });
            self.entries.last_mut().expect("entry was just inserted")
        };
        if scan {
            entry.scan_failures = entry.scan_failures.saturating_add(1);
        }
        if crash {
            entry.crashes = entry.crashes.saturating_add(1);
        }
        entry.last_error = error.into();
        true
    }
    pub fn set_blacklisted(&mut self, plugin_id: &str, blacklisted: bool) -> bool {
        let plugin_id = plugin_id.trim();
        if plugin_id.is_empty() || plugin_id.len() > 256 || plugin_id.contains('\0') {
            return false;
        }
        let existing = self
            .entries
            .iter_mut()
            .find(|entry| entry.plugin_id.eq_ignore_ascii_case(plugin_id));
        if let Some(entry) = existing {
            entry.blacklisted = blacklisted;
            return true;
        }
        if self.entries.len() >= 65_536 {
            return false;
        }
        self.entries.push(PluginCompatibility {
            plugin_id: plugin_id.to_owned(),
            scan_failures: 0,
            crashes: 0,
            blacklisted,
            last_error: String::new(),
        });
        true
    }
    pub fn is_allowed(&self, plugin_id: &str) -> bool {
        self.entries
            .iter()
            .find(|e| e.plugin_id.eq_ignore_ascii_case(plugin_id.trim()))
            .map(|e| !e.blacklisted)
            .unwrap_or(true)
    }
    pub fn blacklisted_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.blacklisted)
            .map(|entry| entry.plugin_id.clone())
            .collect();
        ids.sort();
        ids
    }
    pub fn failed_scans(&self) -> Vec<(String, u32)> {
        let mut items: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.scan_failures > 0)
            .map(|entry| (entry.plugin_id.clone(), entry.scan_failures))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items
    }
    pub fn clear_failures(&mut self, plugin_id: &str) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| entry.plugin_id.eq_ignore_ascii_case(plugin_id.trim()))
            .map(|entry| {
                entry.scan_failures = 0;
                entry.crashes = 0;
                entry.last_error.clear();
                true
            })
            .unwrap_or(false)
    }
    /// Returns plugin IDs whose compatibility state differs from a baseline scan.
    pub fn regression_ids(&self, baseline: &PluginRegistry) -> Vec<String> {
        let mut ids: Vec<String> = self
            .entries
            .iter()
            .filter_map(|current| {
                let previous = baseline
                    .entries
                    .iter()
                    .find(|e| e.plugin_id.eq_ignore_ascii_case(&current.plugin_id));
                (previous != Some(current)).then_some(current.plugin_id.clone())
            })
            .chain(
                baseline
                    .entries
                    .iter()
                    .filter(|old| {
                        !self
                            .entries
                            .iter()
                            .any(|e| e.plugin_id.eq_ignore_ascii_case(&old.plugin_id))
                    })
                    .map(|e| e.plugin_id.clone()),
            )
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
    pub fn validate(&self) -> bool {
        self.entries.len() <= 65_536
            && self.entries.iter().all(PluginCompatibility::validate)
            && self.entries.iter().enumerate().all(|(i, e)| {
                self.entries[..i]
                    .iter()
                    .all(|p| !p.plugin_id.eq_ignore_ascii_case(&e.plugin_id))
            })
    }
}

struct PluginRegistryState {
    registry: PluginRegistry,
}

unsafe fn input_text<'a>(pointer: *const u8, size: usize) -> Option<&'a str> {
    if pointer.is_null() || size == 0 || size > 4096 {
        return None;
    }
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, size) }).ok()
}

#[no_mangle]
pub extern "C" fn hirari_plugin_registry_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(PluginRegistryState {
        registry: PluginRegistry::default(),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<PluginRegistryState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_record_scan_failure(
    state: *mut std::ffi::c_void,
    plugin_id: *const u8,
    plugin_id_size: usize,
    error: *const u8,
    error_size: usize,
) -> bool {
    let (Some(state), Some(plugin_id), Some(error)) = (
        unsafe { state.cast::<PluginRegistryState>().as_mut() },
        unsafe { input_text(plugin_id, plugin_id_size) },
        if error_size == 0 {
            Some("")
        } else {
            unsafe { input_text(error, error_size) }
        },
    ) else {
        return false;
    };
    state.registry.record_scan_failure(plugin_id, error)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_record_crash(
    state: *mut std::ffi::c_void,
    plugin_id: *const u8,
    plugin_id_size: usize,
) -> bool {
    let (Some(state), Some(plugin_id)) = (
        unsafe { state.cast::<PluginRegistryState>().as_mut() },
        unsafe { input_text(plugin_id, plugin_id_size) },
    ) else {
        return false;
    };
    state.registry.record_crash_preserving_error(plugin_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_set_blacklisted(
    state: *mut std::ffi::c_void,
    plugin_id: *const u8,
    plugin_id_size: usize,
    blacklisted: bool,
) -> bool {
    let (Some(state), Some(plugin_id)) = (
        unsafe { state.cast::<PluginRegistryState>().as_mut() },
        unsafe { input_text(plugin_id, plugin_id_size) },
    ) else {
        return false;
    };
    state.registry.set_blacklisted(plugin_id, blacklisted)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_snapshot_json(
    state: *const std::ffi::c_void,
    output: *mut *mut u8,
    output_size: *mut usize,
) -> bool {
    if state.is_null() || output.is_null() || output_size.is_null() {
        return false;
    }
    unsafe {
        output.write(std::ptr::null_mut());
        output_size.write(0);
    }
    let state = unsafe { &*state.cast::<PluginRegistryState>() };
    let mut entries = state.registry.entries.clone();
    entries.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    let value: Vec<_> = entries
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.plugin_id,
                "blacklisted": entry.blacklisted,
                "scan_failures": entry.scan_failures,
                "crashes": entry.crashes,
                "last_error": entry.last_error,
            })
        })
        .collect();
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return false;
    };
    let mut bytes = bytes.into_boxed_slice();
    unsafe {
        output_size.write(bytes.len());
        output.write(bytes.as_mut_ptr());
    }
    std::mem::forget(bytes);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_registry_snapshot_json_free(bytes: *mut u8, size: usize) {
    if !bytes.is_null() {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                bytes, size,
            )))
        };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_failures_and_blacklist() {
        let mut r = PluginRegistry::default();
        assert!(r.record_scan_failure("vst", "bad metadata"));
        assert!(r.record_crash("vst", "segfault"));
        assert!(r.set_blacklisted("VST", true));
        assert!(!r.is_allowed("vst"));
        assert_eq!(r.blacklisted_ids(), vec!["vst"]);
        assert_eq!(r.failed_scans(), vec![("vst".into(), 1)]);
        assert!(r.validate());
    }
    #[test]
    fn detects_regressions() {
        let mut base = PluginRegistry::default();
        base.record_scan_failure("a", "old");
        let mut current = base.clone();
        current.record_crash("a", "new");
        current.record_scan_failure("b", "bad");
        let ids = current.regression_ids(&base);
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }
}
