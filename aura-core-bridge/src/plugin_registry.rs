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
        self.entries
            .iter_mut()
            .find(|e| e.plugin_id.eq_ignore_ascii_case(plugin_id))
            .map(|e| {
                e.blacklisted = blacklisted;
                true
            })
            .unwrap_or(false)
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
