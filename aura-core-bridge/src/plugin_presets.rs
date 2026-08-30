use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PluginPreset { pub id: String, pub plugin_id: String, pub name: String, pub category: String, pub tags: Vec<String>, pub favorite: bool, pub compatible: bool }

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct PluginPresetBrowser { pub presets: Vec<PluginPreset> }

impl PluginPresetBrowser {
    pub fn upsert(&mut self, preset: PluginPreset) -> bool {
        if !valid_text(&preset.id, 256) || !valid_text(&preset.plugin_id, 256)
            || !valid_text(&preset.name, 256) || !valid_text(&preset.category, 128)
            || preset.tags.len() > 64
            || preset.tags.iter().any(|tag| !valid_text(tag, 128))
        { return false; }
        if preset.tags.iter().enumerate().any(|(i, tag)| preset.tags[..i].iter().any(|other| other.eq_ignore_ascii_case(tag))) { return false; }
        if let Some(old)=self.presets.iter_mut().find(|p| p.id==preset.id) { *old=preset; } else if self.presets.len() < 100_000 { self.presets.push(preset); } else { return false; }
        true
    }
    pub fn search(&self, plugin_id: Option<&str>, query: &str, favorites_only: bool, compatible_only: bool) -> Vec<PluginPreset> {
        let q=query.trim().to_ascii_lowercase(); let plugin = plugin_id.map(str::to_ascii_lowercase); let mut result: Vec<_> = self.presets.iter().filter(|p| plugin.as_deref().map(|id| p.plugin_id.eq_ignore_ascii_case(id)).unwrap_or(true) && (!favorites_only||p.favorite) && (!compatible_only||p.compatible) && (q.is_empty()||p.name.to_ascii_lowercase().contains(&q)||p.category.to_ascii_lowercase().contains(&q)||p.tags.iter().any(|t| t.to_ascii_lowercase().contains(&q)))).cloned().collect();
        result.sort_by(|a,b| b.favorite.cmp(&a.favorite).then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())).then_with(|| a.id.cmp(&b.id))); result
    }
    pub fn remove(&mut self, id: &str) -> bool { let before=self.presets.len(); self.presets.retain(|preset| preset.id != id); before != self.presets.len() }
    pub fn validate(&self) -> bool { self.presets.len() <= 100_000 && self.presets.iter().all(|p| valid_text(&p.id,256) && valid_text(&p.plugin_id,256) && valid_text(&p.name,256) && valid_text(&p.category,128) && p.tags.len() <= 64 && p.tags.iter().all(|tag| valid_text(tag,128))) && self.presets.iter().enumerate().all(|(i,p)| self.presets[..i].iter().all(|q| q.id != p.id)) }
}

fn valid_text(value: &str, max: usize) -> bool { !value.trim().is_empty() && value.len() <= max && !value.contains('\0') }

#[cfg(test)]
mod tests { use super::*; #[test] fn browser_filters_presets() { let mut b=PluginPresetBrowser::default(); assert!(b.upsert(PluginPreset{id:"1".into(),plugin_id:"p".into(),name:"Warm Pad".into(),category:"Pads".into(),tags:vec!["analog".into()],favorite:true,compatible:true})); assert_eq!(b.search(Some("p"),"analog",true,true).len(),1); } }
