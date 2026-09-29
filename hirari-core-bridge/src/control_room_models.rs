impl CueMix {
    fn validate(&self) -> bool {
        self.id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && valid_db(self.gain_db)
            && valid_db(self.talkback_send_db)
            && valid_db(self.click_level_db)
            && self.click_pan.is_finite()
            && (-1.0..=1.0).contains(&self.click_pan)
    }
}

impl DownmixPreset {
    fn validate(&self) -> bool {
        self.id != 0
            && self.monitor_id != 0
            && !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && (1..=16).contains(&self.source_channels)
            && self.coefficients.len() == self.source_channels as usize * self.output.channels()
            && self
                .coefficients
                .iter()
                .all(|gain| gain.is_finite() && (-4.0..=4.0).contains(gain))
    }
}

impl PhonesChannel {
    fn validate(&self, room: &ControlRoomConsole) -> bool {
        self.device_ports
            .iter()
            .all(|port| !port.trim().is_empty() && port.len() <= 128 && !port.contains('\0'))
            && !self.device_ports[0].eq_ignore_ascii_case(&self.device_ports[1])
            && valid_db(self.level_db)
            && valid_db(self.click_level_db)
            && self.click_pan.is_finite()
            && (-1.0..=1.0).contains(&self.click_pan)
            && valid_db(self.listen_level_db)
            && match self.source {
                PhonesSource::Mix => true,
                PhonesSource::External(id) => room.sources.iter().any(|source| source.id == id),
                PhonesSource::Cue(id) => room.cues.iter().any(|cue| cue.id == id),
            }
    }
}

fn valid_db(value: f32) -> bool {
    value.is_finite() && (-120.0..=24.0).contains(&value)
}
fn valid_reduction(value: f32) -> bool {
    value.is_finite() && (-120.0..=0.0).contains(&value)
}
fn default_true() -> bool {
    true
}
