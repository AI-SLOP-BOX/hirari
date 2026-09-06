//! OS-independent USB MIDI port lifecycle model.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MidiPort {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct MidiDeviceRegistry {
    ports: std::collections::BTreeMap<String, MidiPort>,
    generation: u64,
}

impl MidiDeviceRegistry {
    pub fn upsert(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<(), &'static str> {
        let id = id.into();
        let name = name.into();
        if id.trim().is_empty()
            || id.len() > 256
            || name.trim().is_empty()
            || name.len() > 256
            || id.contains('\0')
            || name.contains('\0')
        {
            return Err("invalid MIDI port");
        }
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        self.ports
            .entry(id.clone())
            .and_modify(|port| {
                port.name = name.clone();
                port.connected = true;
                port.generation = generation;
            })
            .or_insert(MidiPort {
                id,
                name,
                connected: true,
                generation,
            });
        Ok(())
    }
    pub fn disconnect(&mut self, id: &str) -> bool {
        self.ports
            .get_mut(id)
            .map(|port| {
                if !port.connected {
                    return false;
                }
                port.connected = false;
                true
            })
            .unwrap_or(false)
    }
    pub fn connected_ports(&self) -> Vec<&MidiPort> {
        self.ports.values().filter(|port| port.connected).collect()
    }
    pub fn reconnect_generation(&self, id: &str) -> Option<u64> {
        self.ports
            .get(id)
            .filter(|port| port.connected)
            .map(|port| port.generation)
    }
    pub fn ports(&self) -> impl Iterator<Item = &MidiPort> {
        self.ports.values()
    }

    /// Stable machine-readable snapshot for UI, CLI, and external control
    /// clients. BTreeMap ordering keeps reconnect results deterministic.
    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.ports.values().collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".to_owned())
    }
    pub fn connected_ids(&self) -> Vec<String> {
        self.ports
            .values()
            .filter(|port| port.connected)
            .map(|port| port.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::MidiDeviceRegistry;
    #[test]
    fn tracks_usb_midi_disconnect_and_reconnect_generation() {
        let mut registry = MidiDeviceRegistry::default();
        registry.upsert("usb:vendor:001", "Keyboard").unwrap();
        let first = registry.reconnect_generation("usb:vendor:001").unwrap();
        assert!(registry.disconnect("usb:vendor:001"));
        assert_eq!(registry.reconnect_generation("usb:vendor:001"), None);
        registry.upsert("usb:vendor:001", "Keyboard").unwrap();
        assert!(registry.reconnect_generation("usb:vendor:001").unwrap() > first);
        let snapshot = registry.snapshot_json();
        assert!(snapshot.contains("usb:vendor:001"));
        assert!(snapshot.contains("\"connected\":true"));
    }
}
