use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiPatch {
    pub bank_msb: u8,
    pub bank_lsb: u8,
    pub program: u8,
    pub name: String,
}
impl MidiPatch {
    pub fn bank_program_bytes(&self) -> [u8; 3] {
        [self.bank_msb, self.bank_lsb, self.program]
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiDeviceProfile {
    pub name: String,
    pub manufacturer: String,
    pub port: String,
    pub patches: Vec<MidiPatch>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiProfileRegistry {
    pub profiles: Vec<MidiDeviceProfile>,
}
impl MidiProfileRegistry {
    pub fn upsert(&mut self, profile: MidiDeviceProfile) -> bool {
        if !profile.validate() {
            return false;
        }
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
        {
            *existing = profile;
        } else if self.profiles.len() < 4096 {
            self.profiles.push(profile);
        } else {
            return false;
        }
        self.profiles
            .sort_by_key(|profile| profile.name.to_ascii_lowercase());
        true
    }
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.profiles.len();
        self.profiles
            .retain(|p| !p.name.eq_ignore_ascii_case(name.trim()));
        before != self.profiles.len()
    }
    pub fn find(&self, name: &str) -> Option<&MidiDeviceProfile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }
    pub fn find_by_port(&self, port: &str) -> Vec<&MidiDeviceProfile> {
        let needle = port.trim();
        let mut out: Vec<_> = self
            .profiles
            .iter()
            .filter(|p| p.port.eq_ignore_ascii_case(needle))
            .collect();
        out.sort_by_key(|p| p.name.to_ascii_lowercase());
        out
    }
    pub fn audit(&self) -> bool {
        self.profiles.len() <= 4096
            && self.profiles.iter().all(MidiDeviceProfile::validate)
            && self
                .profiles
                .windows(2)
                .all(|w| w[0].name.to_ascii_lowercase() < w[1].name.to_ascii_lowercase())
    }
}

impl MidiDeviceProfile {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && !self.manufacturer.trim().is_empty()
            && self.manufacturer.len() <= 256
            && !self.manufacturer.contains('\0')
            && !self.port.trim().is_empty()
            && self.port.len() <= 256
            && !self.port.contains('\0')
            && self.patches.len() <= 16_384
            && self
                .patches
                .iter()
                .all(|p| !p.name.trim().is_empty() && p.name.len() <= 256 && !p.name.contains('\0'))
            && self.patches.iter().enumerate().all(|(i, p)| {
                self.patches[..i].iter().all(|q| {
                    (q.bank_msb, q.bank_lsb, q.program) != (p.bank_msb, p.bank_lsb, p.program)
                })
            })
    }
    pub fn find_patch(&self, bank_msb: u8, bank_lsb: u8, program: u8) -> Option<&MidiPatch> {
        self.patches
            .iter()
            .find(|p| p.bank_msb == bank_msb && p.bank_lsb == bank_lsb && p.program == program)
    }
    /// Resolves a bank/program message and returns the user-facing patch name.
    pub fn resolve_program_change(
        &self,
        bank_msb: Option<u8>,
        bank_lsb: Option<u8>,
        program: u8,
    ) -> Option<&MidiPatch> {
        let msb = bank_msb.unwrap_or(0);
        let lsb = bank_lsb.unwrap_or(0);
        self.find_patch(msb, lsb, program)
    }
    pub fn search_patches(&self, query: &str) -> Vec<&MidiPatch> {
        let needle = query.trim().to_lowercase();
        let mut result: Vec<_> = self
            .patches
            .iter()
            .filter(|patch| needle.is_empty() || patch.name.to_lowercase().contains(&needle))
            .collect();
        result.sort_by_key(|patch| (patch.bank_msb, patch.bank_lsb, patch.program));
        result
    }
    pub fn upsert_patch(&mut self, patch: MidiPatch) -> bool {
        if patch.name.trim().is_empty() || patch.name.len() > 256 || patch.name.contains('\0') {
            return false;
        }
        let patch = MidiPatch {
            name: patch.name.trim().to_owned(),
            ..patch
        };
        if let Some(existing) = self.patches.iter_mut().find(|p| {
            (p.bank_msb, p.bank_lsb, p.program) == (patch.bank_msb, patch.bank_lsb, patch.program)
        }) {
            *existing = patch;
            true
        } else if self.patches.len() < 16_384 {
            self.patches.push(patch);
            true
        } else {
            false
        }
    }
    pub fn remove_patch(&mut self, bank_msb: u8, bank_lsb: u8, program: u8) -> bool {
        let before = self.patches.len();
        self.patches
            .retain(|p| (p.bank_msb, p.bank_lsb, p.program) != (bank_msb, bank_lsb, program));
        before != self.patches.len()
    }
    /// Stable interchange format for hardware patch libraries.
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid MIDI device profile".into());
        }
        let mut normalized = self.clone();
        normalized
            .patches
            .sort_by_key(|p| (p.bank_msb, p.bank_lsb, p.program));
        serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        let profile: Self =
            serde_json::from_str(json).map_err(|e| format!("invalid MIDI profile JSON: {e}"))?;
        if !profile.validate() {
            return Err("invalid MIDI device profile".into());
        }
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_patch_by_bank() {
        let p = MidiDeviceProfile {
            name: "Synth".into(),
            manufacturer: "A".into(),
            port: "MIDI 1".into(),
            patches: vec![MidiPatch {
                bank_msb: 1,
                bank_lsb: 2,
                program: 3,
                name: "Lead".into(),
            }],
        };
        assert!(p.validate());
        assert_eq!(p.find_patch(1, 2, 3).unwrap().name, "Lead");
    }
    #[test]
    fn upserts_and_removes_patches() {
        let mut p = MidiDeviceProfile {
            name: "Synth".into(),
            manufacturer: "A".into(),
            port: "MIDI 1".into(),
            patches: vec![],
        };
        assert!(p.upsert_patch(MidiPatch {
            bank_msb: 0,
            bank_lsb: 0,
            program: 1,
            name: "Init".into()
        }));
        assert!(p.upsert_patch(MidiPatch {
            bank_msb: 0,
            bank_lsb: 0,
            program: 1,
            name: "Bright".into()
        }));
        assert_eq!(p.patches.len(), 1);
        assert!(p.remove_patch(0, 0, 1));
        assert!(p.validate());
    }
    #[test]
    fn profile_json_roundtrip_is_validated() {
        let p = MidiDeviceProfile {
            name: "Synth".into(),
            manufacturer: "A".into(),
            port: "MIDI 1".into(),
            patches: vec![MidiPatch {
                bank_msb: 2,
                bank_lsb: 1,
                program: 4,
                name: "Pad".into(),
            }],
        };
        let json = p.to_json().unwrap();
        assert_eq!(MidiDeviceProfile::from_json(&json).unwrap(), p);
        assert!(MidiDeviceProfile::from_json("{}").is_err());
    }
}
