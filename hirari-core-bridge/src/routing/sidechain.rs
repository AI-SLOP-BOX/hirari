use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidechainPort {
    pub plugin_id: String,
    pub port: u16,
    pub name: String,
    pub channels: u8,
    pub enabled: bool,
}
impl SidechainPort {
    pub fn validate(&self) -> bool {
        !self.plugin_id.trim().is_empty()
            && self.plugin_id.len() <= 256
            && !self.plugin_id.contains('\0')
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && self.channels > 0
            && self.channels <= 32
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidechainRegistry {
    pub ports: Vec<SidechainPort>,
}
impl SidechainRegistry {
    pub fn register(&mut self, mut port: SidechainPort) -> bool {
        if !port.validate() || self.ports.len() >= 65_536 {
            return false;
        }
        port.plugin_id = port.plugin_id.trim().to_owned();
        port.name = port.name.trim().to_owned();
        if let Some(existing) = self
            .ports
            .iter_mut()
            .find(|p| p.plugin_id.eq_ignore_ascii_case(&port.plugin_id) && p.port == port.port)
        {
            *existing = port;
        } else {
            self.ports.push(port);
        }
        self.ports
            .sort_by_key(|p| (p.plugin_id.to_ascii_lowercase(), p.port));
        true
    }
    pub fn unregister(&mut self, plugin_id: &str, port: u16) -> bool {
        let before = self.ports.len();
        self.ports
            .retain(|p| !(p.plugin_id.eq_ignore_ascii_case(plugin_id.trim()) && p.port == port));
        before != self.ports.len()
    }
    pub fn set_enabled(&mut self, plugin_id: &str, port: u16, enabled: bool) -> bool {
        self.ports
            .iter_mut()
            .find(|p| p.plugin_id.eq_ignore_ascii_case(plugin_id.trim()) && p.port == port)
            .map(|p| {
                p.enabled = enabled;
                true
            })
            .unwrap_or(false)
    }
    pub fn for_plugin(&self, plugin_id: &str) -> Vec<&SidechainPort> {
        let mut out: Vec<_> = self
            .ports
            .iter()
            .filter(|p| p.plugin_id.eq_ignore_ascii_case(plugin_id.trim()))
            .collect();
        out.sort_by_key(|p| p.port);
        out
    }
    pub fn validate(&self) -> bool {
        self.ports.len() <= 65_536
            && self.ports.iter().all(SidechainPort::validate)
            && self.ports.windows(2).all(|w| {
                (w[0].plugin_id.to_ascii_lowercase(), w[0].port)
                    < (w[1].plugin_id.to_ascii_lowercase(), w[1].port)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manages_dynamic_ports() {
        let mut r = SidechainRegistry::default();
        assert!(r.register(SidechainPort {
            plugin_id: "Comp".into(),
            port: 1,
            name: "Key".into(),
            channels: 2,
            enabled: true
        }));
        assert!(r.set_enabled("comp", 1, false));
        assert_eq!(r.for_plugin("COMP")[0].enabled, false);
        assert!(r.validate());
        assert!(r.unregister("comp", 1));
    }
}
