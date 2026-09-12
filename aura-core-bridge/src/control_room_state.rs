#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRoomCueState {
    pub id: u32,
    pub gain: f32,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRoomState {
    pub monitor_outputs: Vec<String>,
    #[serde(default = "default_monitor_output_gains")]
    pub monitor_output_gains: Vec<f32>,
    #[serde(default = "default_monitor_output_enabled")]
    pub monitor_output_enabled: Vec<bool>,
    pub active_output: usize,
    pub dim: bool,
    pub dim_db: f32,
    pub talkback: bool,
    pub talkback_gain: f32,
    pub cue_gain_db: f32,
    pub reference_track: Option<String>,
    pub reference_enabled: bool,
    #[serde(default)]
    pub cues: Vec<ControlRoomCueState>,
}
impl Default for ControlRoomState {
    fn default() -> Self {
        Self {
            monitor_outputs: vec!["Main".into()],
            monitor_output_gains: vec![1.0],
            monitor_output_enabled: vec![true],
            active_output: 0,
            dim: false,
            dim_db: -20.0,
            talkback: false,
            talkback_gain: 1.0,
            cue_gain_db: 0.0,
            reference_track: None,
            reference_enabled: false,
            cues: Vec::new(),
        }
    }
}
fn default_monitor_output_gains() -> Vec<f32> { vec![1.0] }
fn default_monitor_output_enabled() -> Vec<bool> { vec![true] }
impl ControlRoomState {
    pub fn validate(&self) -> bool {
        !self.monitor_outputs.is_empty()
            && self.monitor_outputs.len() <= 16
            && self.monitor_output_gains.len() == self.monitor_outputs.len()
            && self.monitor_output_enabled.len() == self.monitor_outputs.len()
            && self.monitor_output_gains.iter().all(|gain| gain.is_finite() && (0.0..=4.0).contains(gain))
            && self
                .monitor_outputs
                .iter()
                .all(|o| !o.trim().is_empty() && o.len() <= 128 && !o.contains('\0'))
            && self.monitor_outputs.iter().enumerate().all(|(i, o)| {
                self.monitor_outputs[..i]
                    .iter()
                    .all(|p| !p.trim().eq_ignore_ascii_case(o.trim()))
            })
            && self.active_output < self.monitor_outputs.len()
            && self.dim_db.is_finite()
            && (-60.0..=0.0).contains(&self.dim_db)
            && self.talkback_gain.is_finite()
            && (0.0..=4.0).contains(&self.talkback_gain)
            && self.cue_gain_db.is_finite()
            && (-120.0..=24.0).contains(&self.cue_gain_db)
            && self
                .reference_track
                .as_ref()
                .map(|p| !p.trim().is_empty() && p.len() <= 4096 && !p.contains('\0'))
                .unwrap_or(true)
            && (!self.reference_enabled || self.reference_track.is_some())
    }
    pub fn select_output(&mut self, index: usize) -> bool {
        if index >= self.monitor_outputs.len() {
            false
        } else {
            self.active_output = index;
            true
        }
    }
    pub fn set_dim_db(&mut self, db: f32) -> bool {
        if !db.is_finite() || !(-60.0..=0.0).contains(&db) {
            return false;
        }
        self.dim_db = db;
        true
    }
    pub fn set_cue_gain_db(&mut self, db: f32) -> bool {
        if !db.is_finite() || !(-120.0..=24.0).contains(&db) {
            return false;
        }
        self.cue_gain_db = db;
        true
    }
    pub fn set_talkback_gain(&mut self, gain: f32) -> bool {
        if !gain.is_finite() || !(0.0..=4.0).contains(&gain) {
            return false;
        }
        self.talkback_gain = gain;
        true
    }
    pub fn add_monitor_output(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 128
            || name.contains('\0')
            || self.monitor_outputs.len() >= 16
            || self
                .monitor_outputs
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.monitor_outputs.push(name.to_owned());
        self.monitor_output_gains.push(1.0);
        self.monitor_output_enabled.push(true);
        true
    }
    pub fn rename_monitor_output(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        if index >= self.monitor_outputs.len()
            || name.is_empty()
            || name.len() > 128
            || name.contains('\0')
            || self
                .monitor_outputs
                .iter()
                .enumerate()
                .any(|(i, existing)| i != index && existing.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.monitor_outputs[index] = name.to_owned();
        true
    }
    pub fn remove_monitor_output(&mut self, index: usize) -> bool {
        if index >= self.monitor_outputs.len() || self.monitor_outputs.len() <= 1 {
            return false;
        }
        self.monitor_outputs.remove(index);
        self.monitor_output_gains.remove(index);
        self.monitor_output_enabled.remove(index);
        if self.active_output > index {
            self.active_output -= 1;
        } else if self.active_output >= self.monitor_outputs.len() {
            self.active_output = self.monitor_outputs.len() - 1;
        }
        true
    }
    pub fn set_output_gain(&mut self, index: usize, gain: f32) -> bool {
        if !gain.is_finite() || !(0.0..=4.0).contains(&gain) { return false; }
        self.monitor_output_gains.get_mut(index).map(|value| *value = gain).is_some()
    }
    pub fn set_output_enabled(&mut self, index: usize, enabled: bool) -> bool {
        self.monitor_output_enabled.get_mut(index).map(|value| *value = enabled).is_some()
    }
    pub fn upsert_cue(&mut self, id: u32, gain: f32, enabled: bool) -> bool {
        if id == 0 || !gain.is_finite() || !(0.0..=4.0).contains(&gain) { return false; }
        if let Some(cue) = self.cues.iter_mut().find(|cue| cue.id == id) {
            cue.gain = gain; cue.enabled = enabled;
        } else {
            self.cues.push(ControlRoomCueState { id, gain, enabled });
        }
        true
    }
    pub fn remove_cue(&mut self, id: u32) -> bool {
        let original_len = self.cues.len();
        self.cues.retain(|cue| cue.id != id);
        self.cues.len() != original_len
    }
    pub fn set_cue_enabled(&mut self, id: u32, enabled: bool) -> bool {
        self.cues
            .iter_mut()
            .find(|cue| cue.id == id)
            .map(|cue| cue.enabled = enabled)
            .is_some()
    }
    pub fn set_reference_track(&mut self, path: Option<String>) -> bool {
        if path
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.len() > 4096 || p.contains('\0'))
        {
            return false;
        }
        self.reference_track = path.map(|p| p.trim().to_owned());
        self.reference_enabled = self.reference_track.is_some();
        true
    }
    pub fn set_reference_enabled(&mut self, enabled: bool) -> bool {
        if enabled && self.reference_track.is_none() {
            return false;
        }
        self.reference_enabled = enabled;
        true
    }
    pub fn effective_monitor_gain(&self) -> f32 {
        if self.dim {
            10.0f32.powf(self.dim_db / 20.0)
        } else {
            1.0
        }
    }
    pub fn effective_talkback_gain(&self) -> f32 {
        if self.talkback {
            self.talkback_gain.clamp(0.0, 4.0)
        } else {
            0.0
        }
    }
    pub fn effective_cue_gain(&self) -> f32 {
        10.0f32.powf(self.cue_gain_db.clamp(-120.0, 24.0) / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switches_monitors_and_dims() {
        let mut s = ControlRoomState {
            monitor_outputs: vec!["Main".into(), "Nearfield".into()],
            monitor_output_gains: vec![1.0, 1.0],
            monitor_output_enabled: vec![true, true],
            ..Default::default()
        };
        assert!(s.select_output(1));
        assert!(s.set_reference_track(Some("ref.wav".into())));
        s.dim = true;
        assert!((s.effective_monitor_gain() - 0.1).abs() < 0.001);
        assert!(s.validate());
    }
    #[test]
    fn rejects_duplicate_monitor_outputs() {
        let s = ControlRoomState {
            monitor_outputs: vec!["Main".into(), "Main".into()],
            ..Default::default()
        };
        assert!(!s.validate());
    }

    #[test]
    fn removing_output_before_active_keeps_same_output_selected() {
        let mut s = ControlRoomState {
            monitor_outputs: vec!["Main".into(), "Nearfield".into(), "Farfield".into()],
            monitor_output_gains: vec![1.0; 3],
            monitor_output_enabled: vec![true; 3],
            active_output: 2,
            ..Default::default()
        };
        assert!(s.remove_monitor_output(0));
        assert_eq!(s.monitor_outputs, vec!["Nearfield", "Farfield"]);
        assert_eq!(s.active_output, 1);
        assert_eq!(s.monitor_outputs[s.active_output], "Farfield");
        assert!(s.validate());
    }
}
