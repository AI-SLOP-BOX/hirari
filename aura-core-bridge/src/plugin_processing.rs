use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginProcessingState { pub plugin_id: String, pub online: bool, pub reason: String }

impl PluginProcessingState {
    pub fn set_online(&mut self, online: bool, reason: &str) -> bool { if reason.len() > 512 || reason.contains('\0') || self.plugin_id.trim().is_empty() || self.plugin_id.contains('\0') { return false; } self.plugin_id = self.plugin_id.trim().to_owned(); self.reason = reason.trim().to_owned(); self.online = online; true }
    pub fn validate(&self) -> bool { !self.plugin_id.trim().is_empty() && self.plugin_id.len() <= 256 && self.plugin_id == self.plugin_id.trim() && !self.plugin_id.contains('\0') && self.reason.len() <= 512 && !self.reason.contains('\0') }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginLatencyRegistry { pub samples: std::collections::BTreeMap<String, u32> }
impl PluginLatencyRegistry {
    pub fn set_latency(&mut self, plugin_id: &str, samples: u32) -> bool { if plugin_id.trim().is_empty() || plugin_id.len() > 256 || plugin_id.contains('\0') || samples > 10_000_000 { return false; } let id = plugin_id.trim().to_owned(); self.samples.insert(id, samples); true }
    pub fn latency(&self, plugin_id: &str) -> Option<u32> { self.samples.iter().find(|(id, _)| id.eq_ignore_ascii_case(plugin_id.trim())).map(|(_, value)| *value) }
    pub fn remove(&mut self, plugin_id: &str) -> bool { let key = self.samples.keys().find(|id| id.eq_ignore_ascii_case(plugin_id.trim())).cloned(); key.and_then(|key| self.samples.remove(&key)).is_some() }
    pub fn total_samples(&self) -> u64 { self.samples.values().map(|v| *v as u64).sum() }
    pub fn max_latency(&self) -> Option<(String, u32)> { self.samples.iter().max_by_key(|(_, value)| **value).map(|(id, value)| (id.clone(), *value)) }
    pub fn compensation_samples(&self, plugin_id: &str) -> Option<u32> {
        let plugin_latency = self.latency(plugin_id)?;
        let max_latency = self.samples.values().copied().max().unwrap_or(plugin_latency);
        Some(max_latency.saturating_sub(plugin_latency))
    }

    /// Delays a block by the amount needed to align this plugin with the
    /// slowest registered path. The returned buffer is deterministic and
    /// bounded; callers can feed it into the PDC graph without reallocating
    /// the original audio block.
    pub fn compensate_block(&self, plugin_id: &str, input: &[f32]) -> Option<Vec<f32>> {
        if input.len() > 16_000_000 || input.iter().any(|sample| !sample.is_finite()) { return None; }
        let delay = self.compensation_samples(plugin_id)? as usize;
        let mut output = vec![0.0; input.len().saturating_add(delay)];
        output[delay..].copy_from_slice(input);
        Some(output)
    }
    pub fn latency_report(&self) -> Vec<(String, u32)> { let mut out: Vec<_> = self.samples.iter().map(|(id,v)|(id.clone(),*v)).collect(); out.sort_by(|a,b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))); out }
    pub fn validate(&self) -> bool { self.samples.len() <= 4096 && self.samples.iter().all(|(id, value)| !id.trim().is_empty() && id.len() <= 256 && *id == id.trim() && !id.contains('\0') && *value <= 10_000_000) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn toggles_processing_mode() { let mut s=PluginProcessingState{plugin_id:"plug".into(),online:true,reason:String::new()}; assert!(s.set_online(false,"offline render")); assert!(!s.online); assert!(s.validate()); }
    #[test] fn latency_registry_supports_removal() { let mut r=PluginLatencyRegistry::default(); assert!(r.set_latency("a",128)); assert_eq!(r.latency("a"),Some(128)); assert!(r.remove("a")); assert!(!r.remove("a")); assert_eq!(r.latency("a"),None); }
    #[test] fn latency_compensation_aligns_fast_and_slow_paths() { let mut r=PluginLatencyRegistry::default(); assert!(r.set_latency("fast",32)); assert!(r.set_latency("slow",96)); assert_eq!(r.compensation_samples("fast"),Some(64)); assert_eq!(r.compensate_block("fast", &[1.0, 2.0]).unwrap(), vec![0.0; 64].into_iter().chain([1.0, 2.0]).collect::<Vec<_>>()); assert!(r.compensate_block("fast", &[f32::NAN]).is_none()); }
}
