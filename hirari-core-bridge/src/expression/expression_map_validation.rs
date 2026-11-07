fn packed_map_is_valid(values: &[u32]) -> bool {
    if values.len() % 6 != 0 || values.len() > 24_480 {
        return false;
    }
    let mut counts = [0u8; 256];
    for row in values.chunks_exact(6) {
        let (id, channel, kind, a, b, c) =
            (row[0], row[1], row[2], row[3], row[4], row[5]);
        if id == 0 || id >= 256 || channel >= 16 || !(1..=16).contains(&kind)
            || counts[id as usize] >= 16
        {
            return false;
        }
        let base_kind = if kind > 5 && kind < 11 { kind - 5 }
            else if kind > 10 && kind < 16 { kind - 10 }
            else { kind };
        let valid = if kind == 16 { (1..=16).contains(&a) && b == 0 && c == 0 }
            else if base_kind == 1 { a < 128 && (1..=127).contains(&b) && c <= 96_000 }
            else if base_kind == 2 { a < 128 && (b < 128 || b == 255) && (c < 128 || c == 255) }
            else if base_kind == 3 { a < 128 && b < 128 }
            else if base_kind == 4 { a < 128 }
            else if base_kind == 5 { a < 128 && b < 128 }
            else { false };
        if !valid { return false; }
        counts[id as usize] += 1;
    }
    true
}

/// Admission for a packed articulation map and its JSON side, shared by the
/// expression-map edit host. The size and id limits are the ones the engine's
/// `set_expression_map` applied before touching a track.
pub(crate) fn admit_packed_map(
    values: &[u32],
    json_size: usize,
    articulation_id: u32,
) -> bool {
    json_size <= 1024 * 1024 && articulation_id <= 255 && packed_map_is_valid(values)
}

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
