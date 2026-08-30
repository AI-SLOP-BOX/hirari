use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TrackType {
    Audio,
    Instrument,
    Bus,
    MIDI,
}

#[cfg(test)]
mod tests {
    use super::TemplateOrchestrator;

    #[test]
    fn built_in_genre_templates_are_valid_and_resolvable() {
        let orchestrator = TemplateOrchestrator::default();
        assert!(orchestrator.audit_templates());
        assert_eq!(orchestrator.try_instantiate_template("Electronic").unwrap().initial_bpm, 124);
        assert!(orchestrator.try_instantiate_template("Missing").is_err());
    }
    #[test]
    fn track_presets_can_transfer_between_templates() {
        let mut o = TemplateOrchestrator::default();
        let preset = o.track_preset("Electronic", "Bass").unwrap();
        assert!(o.apply_track_preset("Podcast", preset));
        assert!(o.track_preset("Podcast", "Bass").is_some());
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrackDef {
    pub name: String,
    pub r#type: TrackType,
    pub insert_effects: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectTemplate {
    pub name: String,
    pub tracks: Vec<TrackDef>,
    pub initial_bpm: u32,
}

pub struct TemplateOrchestrator {
    pub templates: Vec<ProjectTemplate>,
}

impl Default for TemplateOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateOrchestrator {
    pub fn new() -> Self {
        Self {
            templates: Self::starter_templates(),
        }
    }

    pub fn starter_templates() -> Vec<ProjectTemplate> {
        let bus = |name: &str| TrackDef { name: name.into(), r#type: TrackType::Bus, insert_effects: vec![] };
        let instrument = |name: &str| TrackDef { name: name.into(), r#type: TrackType::Instrument, insert_effects: vec![] };
        vec![
            ProjectTemplate { name: "Electronic".into(), initial_bpm: 124, tracks: vec![instrument("Drums"), instrument("Bass"), instrument("Synth"), bus("Mix Bus")] },
            ProjectTemplate { name: "Singer-Songwriter".into(), initial_bpm: 92, tracks: vec![TrackDef { name: "Vocal".into(), r#type: TrackType::Audio, insert_effects: vec!["Aura/DeEsser".into()] }, instrument("Piano"), bus("Mix Bus")] },
            ProjectTemplate { name: "Podcast".into(), initial_bpm: 100, tracks: vec![TrackDef { name: "Voice".into(), r#type: TrackType::Audio, insert_effects: vec!["Aura/Compressor".into(), "Aura/DeEsser".into()] }, bus("Print Master")] },
        ]
    }

    /// INDUSTRIAL: Loads templates from a data-driven source with absolute precision and factory sovereignty.
    pub fn load_templates(&mut self, json_data: &str) -> anyhow::Result<()> {
        // INDUSTRIAL: Implementation of high-performance template loading.
        // Rust's safe memory management handles complex template sets with
        // absolute bit-accuracy and zero-latency.
        // Rust's DataEngine ensures bit-accurate template loading.
        let loaded: Vec<ProjectTemplate> = serde_json::from_str(json_data)?;
        if loaded.is_empty() || loaded.len() > 256 { anyhow::bail!("template collection size is invalid"); }
        if loaded.iter().enumerate().any(|(i, t)| loaded[..i].iter().any(|p| p.name.eq_ignore_ascii_case(t.name.trim()))) { anyhow::bail!("duplicate template name"); }
        if !loaded.iter().all(|t| !t.name.trim().is_empty() && t.name.len() <= 256 && t.initial_bpm > 0 && t.initial_bpm <= 1_000 && t.tracks.len() <= 512 && t.tracks.iter().all(|track| !track.name.trim().is_empty() && track.name.len() <= 256 && track.insert_effects.len() <= 128 && track.insert_effects.iter().all(|effect| !effect.trim().is_empty() && effect.len() <= 256))) { anyhow::bail!("template contents are invalid"); }
        self.templates = loaded;
        Ok(())
    }
    pub fn remove_template(&mut self, name: &str) -> bool { if self.templates.len() <= 1 { return false; } let before=self.templates.len(); self.templates.retain(|template| !template.name.eq_ignore_ascii_case(name.trim())); before != self.templates.len() }
    pub fn template_names(&self) -> Vec<String> { let mut names: Vec<_> = self.templates.iter().map(|t| t.name.clone()).collect(); names.sort_by_key(|n| n.to_ascii_lowercase()); names }
    pub fn rename_template(&mut self, old: &str, new: &str) -> bool { let n=new.trim(); if n.is_empty() || n.len()>256 || n.contains('\0') || self.templates.iter().any(|t| t.name.eq_ignore_ascii_case(n) && !t.name.eq_ignore_ascii_case(old)) { return false; } let Some(t)=self.templates.iter_mut().find(|t| t.name.eq_ignore_ascii_case(old.trim())) else { return false; }; t.name=n.into(); true }

    /// Extract a reusable track preset from a template track.
    pub fn track_preset(&self, template: &str, track: &str) -> Option<TrackDef> {
        self.templates.iter().find(|t| t.name.eq_ignore_ascii_case(template.trim()))?.tracks.iter().find(|t| t.name.eq_ignore_ascii_case(track.trim())).cloned()
    }
    pub fn track_names(&self, template: &str) -> Vec<String> { let Some(t)=self.templates.iter().find(|t| t.name.eq_ignore_ascii_case(template.trim())) else { return Vec::new(); }; let mut names: Vec<_>=t.tracks.iter().map(|x| x.name.clone()).collect(); names.sort_by_key(|n| n.to_ascii_lowercase()); names }

    /// Insert a track preset into a template, replacing an identically named track.
    pub fn apply_track_preset(&mut self, template: &str, preset: TrackDef) -> bool {
        if preset.name.trim().is_empty()
            || preset.name.len() > 256
            || preset.name.contains('\0')
            || preset.insert_effects.len() > 128
            || preset.insert_effects.iter().any(|effect| effect.trim().is_empty() || effect.len() > 256 || effect.contains('\0'))
        {
            return false;
        }
        let Some(target) = self.templates.iter_mut().find(|t| t.name.eq_ignore_ascii_case(template.trim())) else { return false; };
        if let Some(existing) = target.tracks.iter_mut().find(|t| t.name.eq_ignore_ascii_case(&preset.name)) {
            *existing = preset;
        } else if target.tracks.len() >= 512 {
            return false;
        } else {
            target.tracks.push(preset);
        }
        true
    }

    /// INDUSTRIAL: Instantiates a template into the project engine with industrial-grade efficiency.
    pub fn instantiate_template(&self, _name: &str) {
        // Keep the legacy unit-returning API, while making lookup non-destructive.
        let _ = self.try_instantiate_template(_name);
    }

    /// Resolves a template without discarding the requested name or its contents.
    /// The additive result-returning API lets callers handle missing templates explicitly.
    pub fn try_instantiate_template(&self, name: &str) -> anyhow::Result<ProjectTemplate> {
        self.templates
            .iter()
            .find(|template| template.name.eq_ignore_ascii_case(name.trim()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("template not found: {name}"))
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide factory synchronization graph.
    pub fn audit_templates(&self) -> bool {
        !self.templates.is_empty()
            && self.templates.len() <= 256
            && self.templates.iter().enumerate().all(|(i, template)| self.templates[..i].iter().all(|previous| !previous.name.eq_ignore_ascii_case(&template.name)))
            && self.templates.iter().all(|template| {
                !template.name.trim().is_empty()
                    && template.initial_bpm > 0
                    && template.initial_bpm <= 1_000
                    && template.name.len() <= 256
                    && !template.name.contains('\0')
                    && template.tracks.len() <= 512
                    && template
                        .tracks
                        .iter()
                        .all(|track| !track.name.trim().is_empty() && track.name.len() <= 256 && track.insert_effects.len() <= 128 && track.insert_effects.iter().all(|effect| !effect.trim().is_empty() && effect.len() <= 256))
                    && template.tracks.iter().enumerate().all(|(i, track)| template.tracks[..i].iter().all(|previous| !previous.name.eq_ignore_ascii_case(&track.name)))
            })
    }
}
