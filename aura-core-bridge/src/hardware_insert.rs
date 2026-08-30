//! Non-destructive external hardware insert contract.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HardwareInsert {
    pub input_bus: u32,
    pub output_bus: u32,
    pub round_trip_latency_samples: u32,
    pub wet: f32,
    pub dry: f32,
    pub enabled: bool,
    #[serde(default)]
    pub bypassed: bool,
    #[serde(default)]
    pub send_gain_db: f32,
    #[serde(default)]
    pub return_gain_db: f32,
    #[serde(default)]
    pub audio_driver_round_trip_samples: u32,
    #[serde(default)]
    pub midi_device: Option<u32>,
}

impl HardwareInsert {
    pub fn new(input_bus: u32, output_bus: u32) -> Self {
        Self { input_bus, output_bus, round_trip_latency_samples: 0, wet: 1.0, dry: 0.0,
            enabled: false, bypassed: false, send_gain_db: 0.0, return_gain_db: 0.0,
            audio_driver_round_trip_samples: 0, midi_device: None }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.input_bus == 0 || self.output_bus == 0 { return Err("hardware insert bus is invalid"); }
        if self.input_bus == self.output_bus { return Err("hardware insert cannot loop to the same bus"); }
        if self.round_trip_latency_samples > 16_777_216 { return Err("hardware insert latency is out of range"); }
        if self.audio_driver_round_trip_samples > 16_777_216 { return Err("audio driver latency is out of range"); }
        if !self.wet.is_finite() || !self.dry.is_finite() || !(0.0..=1.0).contains(&self.wet) || !(0.0..=1.0).contains(&self.dry) { return Err("hardware insert mix must be within 0..=1"); }
        if !valid_gain(self.send_gain_db) || !valid_gain(self.return_gain_db) { return Err("hardware insert gain is out of range"); }
        if self.midi_device == Some(0) { return Err("hardware insert MIDI device is invalid"); }
        Ok(())
    }

    pub fn compensated_delay_samples(&self) -> u32 {
        if self.enabled && !self.bypassed && self.midi_device.is_some() { self.round_trip_latency_samples } else { 0 }
    }

    pub fn associate_midi_device(&mut self, device_id: Option<u32>) -> bool {
        if device_id == Some(0) { return false; }
        self.midi_device = device_id; true
    }

    /// Applies the calibrated send gain before audio leaves the interface.
    pub fn prepare_send_block(&self, input: &[f32]) -> Option<Vec<f32>> {
        if !self.enabled || self.bypassed || self.validate().is_err() || input.iter().any(|sample| !sample.is_finite()) {
            return None;
        }
        let gain = db_gain(self.send_gain_db);
        Some(input.iter().map(|sample| sample * gain).collect())
    }

    /// Measures the hardware-only loop delay from a sent probe and recorded
    /// return. Interface latency is removed because Cubase handles it itself.
    pub fn check_user_delay(&mut self, sent: &[f32], returned: &[f32]) -> Result<u32, &'static str> {
        if sent.is_empty() || returned.is_empty() || sent.len() > 16_777_216 || returned.len() > 16_777_216
            || sent.iter().chain(returned).any(|sample| !sample.is_finite()) {
            return Err("invalid delay measurement buffers");
        }
        let sent_peak = peak_index(sent).ok_or("delay probe is silent")?;
        let return_peak = peak_index(returned).ok_or("delay return is silent")?;
        if return_peak < sent_peak { return Err("delay return precedes probe"); }
        let measured = u32::try_from(return_peak - sent_peak).map_err(|_| "measured delay is out of range")?;
        self.round_trip_latency_samples = measured.saturating_sub(self.audio_driver_round_trip_samples);
        Ok(self.round_trip_latency_samples)
    }

    /// Mixes the external return into the dry signal without mutating the
    /// caller's return buffer.  The transport layer owns the actual device
    /// I/O; this deterministic block operation is shared by realtime and
    /// offline paths and keeps the insert non-destructive.
    pub fn process_block(&self, dry_left: &[f32], dry_right: &[f32], return_left: &[f32], return_right: &[f32], output_left: &mut [f32], output_right: &mut [f32]) -> bool {
        if !self.enabled || self.validate().is_err() || dry_left.len() != dry_right.len()
            || return_left.len() != return_right.len() || output_left.len() != output_right.len()
            || dry_left.len() != return_left.len() || output_left.len() != dry_left.len()
            || dry_left.iter().chain(dry_right).chain(return_left).chain(return_right).any(|sample| !sample.is_finite()) {
            return false;
        }
        if self.bypassed {
            output_left.copy_from_slice(dry_left); output_right.copy_from_slice(dry_right); return true;
        }
        let return_gain = db_gain(self.return_gain_db);
        for index in 0..output_left.len() {
            output_left[index] = dry_left[index] * self.dry + return_left[index] * return_gain * self.wet;
            output_right[index] = dry_right[index] * self.dry + return_right[index] * return_gain * self.wet;
        }
        true
    }
}

fn valid_gain(value: f32) -> bool { value.is_finite() && (-120.0..=24.0).contains(&value) }
fn db_gain(value: f32) -> f32 { 10.0f32.powf(value / 20.0) }
fn peak_index(samples: &[f32]) -> Option<usize> {
    let (index, peak) = samples.iter().enumerate().max_by(|left, right| left.1.abs().total_cmp(&right.1.abs()))?;
    (peak.abs() > f32::EPSILON).then_some(index)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalInstrument {
    pub name: String,
    pub return_bus: u32,
    pub midi_device: u32,
    pub user_delay_samples: u32,
    pub audio_driver_round_trip_samples: u32,
    pub return_gain_db: f32,
    pub enabled: bool,
    pub bypassed: bool,
}

impl ExternalInstrument {
    pub fn new(name: &str, return_bus: u32, midi_device: u32) -> Option<Self> {
        let instrument = Self { name: name.trim().to_owned(), return_bus, midi_device,
            user_delay_samples: 0, audio_driver_round_trip_samples: 0, return_gain_db: 0.0,
            enabled: false, bypassed: false };
        instrument.validate().is_ok().then_some(instrument)
    }

    pub fn to_json(&self) -> Result<String, String> {
        self.validate().map_err(str::to_owned)?;
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let instrument: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        instrument.validate().map_err(str::to_owned)?;
        Ok(instrument)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || self.name.len() > 128 || self.name.contains('\0') {
            return Err("external instrument name is invalid");
        }
        if self.return_bus == 0 || self.midi_device == 0 { return Err("external instrument routing is invalid"); }
        if self.user_delay_samples > 16_777_216 || self.audio_driver_round_trip_samples > 16_777_216 {
            return Err("external instrument latency is out of range");
        }
        if !valid_gain(self.return_gain_db) { return Err("external instrument return gain is out of range"); }
        Ok(())
    }

    pub fn compensated_delay_samples(&self) -> u32 {
        if self.enabled && !self.bypassed { self.user_delay_samples } else { 0 }
    }

    pub fn check_user_delay(&mut self, midi_note_sent_sample: u64,
        audio_onset_sample: u64) -> Result<u32, &'static str> {
        if audio_onset_sample < midi_note_sent_sample { return Err("instrument return precedes MIDI note"); }
        let measured = audio_onset_sample - midi_note_sent_sample;
        let measured = u32::try_from(measured).map_err(|_| "measured delay is out of range")?;
        if measured > 16_777_216 { return Err("measured delay is out of range"); }
        self.user_delay_samples = measured.saturating_sub(self.audio_driver_round_trip_samples);
        Ok(self.user_delay_samples)
    }

    /// Applies Return Gain to interleaved audio. Bypass keeps the external
    /// instrument audible but skips host-side gain, matching a VSTi bypass.
    pub fn process_return(&self, input: &[f32]) -> Result<Vec<f32>, &'static str> {
        if !self.enabled || self.validate().is_err() || input.is_empty()
            || input.iter().any(|sample| !sample.is_finite()) {
            return Err("external instrument return is unavailable");
        }
        let gain = if self.bypassed { 1.0 } else { db_gain(self.return_gain_db) };
        Ok(input.iter().map(|sample| sample * gain).collect())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalEffectBus {
    pub id: u32,
    pub name: String,
    pub insert: HardwareInsert,
    pub send_ports: Vec<u32>,
    pub return_ports: Vec<u32>,
    pub used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalInstrumentBus {
    pub id: u32,
    pub instrument: ExternalInstrument,
    pub return_ports: Vec<u32>,
    pub used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExternalHardwareFavorite {
    Effect(ExternalEffectBus),
    Instrument(ExternalInstrumentBus),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExternalHardwareRegistry {
    pub effects: Vec<ExternalEffectBus>,
    pub instruments: Vec<ExternalInstrumentBus>,
    pub favorites: std::collections::BTreeMap<String, ExternalHardwareFavorite>,
}

impl ExternalHardwareRegistry {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid external hardware registry".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let registry: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if registry.audit() { Ok(registry) } else { Err("invalid external hardware registry".into()) }
    }

    pub fn upsert_effect(&mut self, mut effect: ExternalEffectBus) -> bool {
        effect.name = effect.name.trim().to_owned();
        if !effect.validate() || self.name_conflicts(effect.id, &effect.name)
            || self.ports_conflict(Some(effect.id), None, effect.send_ports.iter().chain(&effect.return_ports).copied()) {
            return false;
        }
        if let Some(existing) = self.effects.iter_mut().find(|item| item.id == effect.id) { *existing = effect; }
        else if self.effects.len() < 1024 { self.effects.push(effect); } else { return false; }
        self.effects.sort_by_key(|item| item.id); true
    }

    pub fn upsert_instrument(&mut self, mut instrument: ExternalInstrumentBus) -> bool {
        instrument.instrument.name = instrument.instrument.name.trim().to_owned();
        if !instrument.validate() || self.name_conflicts(instrument.id, &instrument.instrument.name)
            || self.ports_conflict(None, Some(instrument.id), instrument.return_ports.iter().copied()) {
            return false;
        }
        if let Some(existing) = self.instruments.iter_mut().find(|item| item.id == instrument.id) { *existing = instrument; }
        else if self.instruments.len() < 1024 { self.instruments.push(instrument); } else { return false; }
        self.instruments.sort_by_key(|item| item.id); true
    }

    pub fn set_used(&mut self, id: u32, instrument: bool, used: bool) -> bool {
        if instrument {
            let Some(item) = self.instruments.iter_mut().find(|item| item.id == id) else { return false; };
            item.used = used;
        } else {
            let Some(item) = self.effects.iter_mut().find(|item| item.id == id) else { return false; };
            item.used = used;
        }
        true
    }

    pub fn save_effect_favorite(&mut self, favorite_name: &str, effect_id: u32) -> bool {
        let Some(name) = valid_favorite_name(favorite_name) else { return false; };
        let Some(effect) = self.effects.iter().find(|item| item.id == effect_id) else { return false; };
        if self.favorites.keys().any(|existing| existing.eq_ignore_ascii_case(&name)) { return false; }
        let mut snapshot = effect.clone(); snapshot.id = 1; snapshot.used = false;
        self.favorites.insert(name, ExternalHardwareFavorite::Effect(snapshot)); true
    }

    pub fn save_instrument_favorite(&mut self, favorite_name: &str, instrument_id: u32) -> bool {
        let Some(name) = valid_favorite_name(favorite_name) else { return false; };
        let Some(instrument) = self.instruments.iter().find(|item| item.id == instrument_id) else { return false; };
        if self.favorites.keys().any(|existing| existing.eq_ignore_ascii_case(&name)) { return false; }
        let mut snapshot = instrument.clone(); snapshot.id = 1; snapshot.used = false;
        self.favorites.insert(name, ExternalHardwareFavorite::Instrument(snapshot)); true
    }

    pub fn instantiate_favorite(&mut self, favorite_name: &str, id: u32, name: &str) -> bool {
        if id == 0 { return false; }
        let Some(favorite) = self.favorites.iter().find(|(key, _)| key.eq_ignore_ascii_case(favorite_name))
            .map(|(_, value)| value.clone()) else { return false; };
        match favorite {
            ExternalHardwareFavorite::Effect(mut effect) => {
                effect.id = id; effect.name = name.into(); self.upsert_effect(effect)
            }
            ExternalHardwareFavorite::Instrument(mut instrument) => {
                instrument.id = id; instrument.instrument.name = name.into(); self.upsert_instrument(instrument)
            }
        }
    }

    pub fn audit(&self) -> bool {
        if self.effects.len() > 1024 || self.instruments.len() > 1024 || self.favorites.len() > 4096
            || !self.effects.iter().all(ExternalEffectBus::validate)
            || !self.instruments.iter().all(ExternalInstrumentBus::validate) {
            return false;
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        let mut ports = std::collections::BTreeSet::new();
        for effect in &self.effects {
            if !ids.insert(effect.id) || !names.insert(effect.name.to_ascii_lowercase())
                || !effect.send_ports.iter().chain(&effect.return_ports).all(|port| ports.insert(*port)) { return false; }
        }
        for instrument in &self.instruments {
            if !ids.insert(instrument.id) || !names.insert(instrument.instrument.name.to_ascii_lowercase())
                || !instrument.return_ports.iter().all(|port| ports.insert(*port)) { return false; }
        }
        self.favorites.iter().all(|(name, favorite)| valid_favorite_name(name).as_deref() == Some(name.as_str())
            && match favorite {
                ExternalHardwareFavorite::Effect(effect) => effect.validate() && !effect.used,
                ExternalHardwareFavorite::Instrument(instrument) => instrument.validate() && !instrument.used,
            })
    }

    fn name_conflicts(&self, id: u32, name: &str) -> bool {
        self.effects.iter().any(|item| item.id != id && item.name.eq_ignore_ascii_case(name))
            || self.instruments.iter().any(|item| item.id != id && item.instrument.name.eq_ignore_ascii_case(name))
    }

    fn ports_conflict(&self, effect_id: Option<u32>, instrument_id: Option<u32>, ports: impl Iterator<Item = u32>) -> bool {
        let occupied = self.effects.iter().filter(|item| Some(item.id) != effect_id)
            .flat_map(|item| item.send_ports.iter().chain(&item.return_ports)).copied()
            .chain(self.instruments.iter().filter(|item| Some(item.id) != instrument_id)
                .flat_map(|item| item.return_ports.iter()).copied())
            .collect::<std::collections::BTreeSet<_>>();
        let mut proposed = std::collections::BTreeSet::new();
        ports.into_iter().any(|port| port == 0 || occupied.contains(&port) || !proposed.insert(port))
    }
}

impl ExternalEffectBus {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 128 && !self.name.contains('\0')
            && self.insert.validate().is_ok() && !self.send_ports.is_empty() && self.send_ports.len() <= 32
            && !self.return_ports.is_empty() && self.return_ports.len() <= 32
            && self.send_ports.len() == self.return_ports.len()
            && unique_ports(self.send_ports.iter().chain(&self.return_ports).copied())
    }
}

impl ExternalInstrumentBus {
    fn validate(&self) -> bool {
        self.id != 0 && self.instrument.validate().is_ok() && !self.return_ports.is_empty()
            && self.return_ports.len() <= 32 && unique_ports(self.return_ports.iter().copied())
    }
}

fn unique_ports(ports: impl Iterator<Item = u32>) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    ports.into_iter().all(|port| port != 0 && seen.insert(port))
}

fn valid_favorite_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.len() <= 128 && !name.contains('\0')).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{ExternalEffectBus, ExternalHardwareRegistry, ExternalInstrument,
        ExternalInstrumentBus, HardwareInsert};
    #[test]
    fn validates_external_insert_and_exposes_pdc_delay() {
        let mut insert = HardwareInsert::new(1, 2);
        insert.round_trip_latency_samples = 512;
        insert.wet = 0.7;
        insert.dry = 0.3;
        assert!(insert.validate().is_ok());
        assert_eq!(insert.compensated_delay_samples(), 0);
        insert.enabled = true;
        assert_eq!(insert.compensated_delay_samples(), 0);
        assert!(insert.associate_midi_device(Some(9)));
        assert_eq!(insert.compensated_delay_samples(), 512);
        insert.wet = 2.0;
        assert!(insert.validate().is_err());
    }

    #[test]
    fn mixes_external_return_without_aliasing_input_buffers() {
        let mut insert = HardwareInsert::new(1, 2);
        insert.enabled = true;
        insert.wet = 0.75;
        insert.dry = 0.25;
        let dry_left = [1.0, -1.0];
        let dry_right = [0.5, -0.5];
        let return_left = [0.0, 0.5];
        let return_right = [1.0, -0.5];
        let mut output_left = [0.0; 2];
        let mut output_right = [0.0; 2];
        assert!(insert.process_block(&dry_left, &dry_right, &return_left, &return_right, &mut output_left, &mut output_right));
        assert_eq!(output_left, [0.25, 0.125]);
        assert_eq!(output_right, [0.875, -0.5]);
        assert_eq!(dry_left, [1.0, -1.0]);
    }

    #[test]
    fn measures_only_hardware_delay_after_subtracting_driver_round_trip() {
        let mut insert = HardwareInsert::new(1, 2);
        insert.audio_driver_round_trip_samples = 32;
        let mut sent = vec![0.0; 256];
        let mut returned = vec![0.0; 256];
        sent[10] = 1.0;
        returned[106] = 0.8;
        assert_eq!(insert.check_user_delay(&sent, &returned), Ok(64));
        assert_eq!(insert.round_trip_latency_samples, 64);
        assert!(insert.check_user_delay(&[0.0; 4], &[0.0; 4]).is_err());
    }

    #[test]
    fn send_and_return_gain_are_independent_and_bypass_is_dry_unity() {
        let mut insert = HardwareInsert::new(1, 2);
        insert.enabled = true;
        insert.send_gain_db = -6.0206;
        insert.return_gain_db = 6.0206;
        let send = insert.prepare_send_block(&[1.0, -1.0]).unwrap();
        assert!((send[0] - 0.5).abs() < 0.001);

        let mut left = [0.0];
        let mut right = [0.0];
        assert!(insert.process_block(&[0.25], &[0.5], &[0.25], &[0.25], &mut left, &mut right));
        assert!((left[0] - 0.5).abs() < 0.001);
        assert!((right[0] - 0.5).abs() < 0.001);

        insert.bypassed = true;
        assert!(insert.process_block(&[0.25], &[0.5], &[1.0], &[1.0], &mut left, &mut right));
        assert_eq!((left, right), ([0.25], [0.5]));
        assert!(insert.prepare_send_block(&[1.0]).is_none());
        assert_eq!(insert.compensated_delay_samples(), 0);
    }

    #[test]
    fn external_instrument_measures_midi_to_audio_delay_for_pdc() {
        let mut instrument = ExternalInstrument::new("Prophet", 7, 12).unwrap();
        instrument.audio_driver_round_trip_samples = 64;
        assert_eq!(instrument.check_user_delay(1_000, 1_320), Ok(256));
        assert_eq!(instrument.compensated_delay_samples(), 0);
        instrument.enabled = true;
        assert_eq!(instrument.compensated_delay_samples(), 256);
        instrument.bypassed = true;
        assert_eq!(instrument.compensated_delay_samples(), 0);
        assert!(instrument.check_user_delay(2_000, 1_999).is_err());
    }

    #[test]
    fn external_instrument_applies_return_gain_and_round_trips_settings() {
        let mut instrument = ExternalInstrument::new("Juno", 3, 4).unwrap();
        instrument.enabled = true;
        instrument.return_gain_db = 6.0206;
        let output = instrument.process_return(&[0.25, -0.5]).unwrap();
        assert!((output[0] - 0.5).abs() < 0.001);
        assert!((output[1] + 1.0).abs() < 0.001);
        let restored = ExternalInstrument::from_json(&instrument.to_json().unwrap()).unwrap();
        assert_eq!(restored, instrument);

        instrument.bypassed = true;
        assert_eq!(instrument.process_return(&[0.25, -0.5]).unwrap(), vec![0.25, -0.5]);
        assert!(ExternalInstrument::new("", 3, 4).is_none());
        assert!(ExternalInstrument::new("Bad", 0, 4).is_none());
        assert!(ExternalInstrument::new("Bad", 3, 0).is_none());
    }

    #[test]
    fn audio_connections_registry_rejects_shared_physical_ports_and_names() {
        let mut registry = ExternalHardwareRegistry::default();
        let effect = ExternalEffectBus { id: 1, name: "Compressor".into(),
            insert: HardwareInsert::new(10, 11), send_ports: vec![1, 2],
            return_ports: vec![3, 4], used: false };
        assert!(registry.upsert_effect(effect.clone()));
        assert!(!registry.upsert_effect(ExternalEffectBus { id: 2, name: "compressor".into(),
            send_ports: vec![5, 6], return_ports: vec![7, 8], ..effect.clone() }));
        let instrument = ExternalInstrumentBus { id: 2,
            instrument: ExternalInstrument::new("Synth", 12, 20).unwrap(),
            return_ports: vec![4, 9], used: false };
        assert!(!registry.upsert_instrument(instrument));
        assert!(registry.upsert_instrument(ExternalInstrumentBus { id: 2,
            instrument: ExternalInstrument::new("Synth", 12, 20).unwrap(),
            return_ports: vec![9, 10], used: false }));
        assert!(registry.set_used(1, false, true));
        assert!(registry.set_used(2, true, true));
        assert!(registry.audit());
    }

    #[test]
    fn external_hardware_favorites_round_trip_and_reinstantiate_configuration() {
        let mut registry = ExternalHardwareRegistry::default();
        assert!(registry.upsert_effect(ExternalEffectBus { id: 7, name: "Plate".into(),
            insert: HardwareInsert::new(20, 21), send_ports: vec![11, 12],
            return_ports: vec![13, 14], used: true }));
        assert!(registry.save_effect_favorite("Studio Plate", 7));
        assert!(!registry.save_effect_favorite("studio plate", 7));
        registry.effects.clear();
        assert!(registry.instantiate_favorite("STUDIO PLATE", 9, "Plate B"));
        assert_eq!(registry.effects[0].id, 9);
        assert_eq!(registry.effects[0].name, "Plate B");
        assert!(!registry.effects[0].used);
        let restored = ExternalHardwareRegistry::from_json(&registry.to_json().unwrap()).unwrap();
        assert_eq!(restored, registry);
    }
}
