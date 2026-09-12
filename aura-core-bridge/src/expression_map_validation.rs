impl ProArticulation {
    fn validate(&self, ids: &std::collections::BTreeSet<String>) -> bool {
        let reference_valid = |reference: &Option<String>| {
            reference.as_ref().is_none_or(|id| {
                !id.eq_ignore_ascii_case(&self.id) && ids.contains(&id.to_ascii_lowercase())
            })
        };
        !self.id.trim().is_empty()
            && self.id.len() <= 128
            && !self.id.contains('\0')
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && (1..=16).contains(&self.group)
            && self.playback_technique.len() <= 256
            && !self.playback_technique.contains('\0')
            && reference_valid(&self.alias_for)
            && reference_valid(&self.fallback)
            && self
                .remote_trigger
                .as_ref()
                .is_none_or(RemoteTrigger::validate)
    }
}

impl RemoteTrigger {
    fn validate(&self) -> bool {
        match self {
            Self::Key { note } => *note < 128,
            Self::Program { program } => *program < 128,
        }
    }
}

impl MidiOutput {
    fn validate(&self) -> bool {
        match self {
            Self::KeySwitch {
                note,
                velocity,
                length_ticks,
            } => *note < 128 && (1..=127).contains(velocity) && (1..=96_000).contains(length_ticks),
            Self::ProgramChange {
                bank_msb,
                bank_lsb,
                program,
            } => {
                bank_msb.is_none_or(|v| v < 128)
                    && bank_lsb.is_none_or(|v| v < 128)
                    && *program < 128
            }
            Self::ControlChange { controller, value } => *controller < 128 && *value < 128,
            Self::ChannelPressure { value } => *value < 128,
            Self::PitchBend { value } => (-8192..=8191).contains(value),
        }
    }
}
