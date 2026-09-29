impl SoundSlot {
    fn validate(&self, ids: &std::collections::BTreeSet<String>) -> bool {
        let own_ids: std::collections::BTreeSet<_> = self
            .articulation_ids
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .collect();
        !self.id.trim().is_empty()
            && self.id.len() <= 128
            && !self.id.contains('\0')
            && !self.name.trim().is_empty()
            && self.name.len() <= 256
            && !self.name.contains('\0')
            && !self.articulation_ids.is_empty()
            && self.articulation_ids.len() <= 16
            && own_ids.len() == self.articulation_ids.len()
            && own_ids.iter().all(|id| ids.contains(id))
            && self.outputs.len() <= 32
            && self.off_outputs.len() <= 32
            && self.outputs.iter().all(MidiOutput::validate)
            && self.off_outputs.iter().all(MidiOutput::validate)
            && self.channel.is_none_or(|value| value < 16)
            && (-48..=48).contains(&self.transpose)
            && self.velocity_scale.is_finite()
            && (0.01..=8.0).contains(&self.velocity_scale)
            && self
                .pitch_range
                .is_none_or(|(min, max)| min <= max && max < 128)
            && self
                .velocity_range
                .is_none_or(|(min, max)| min >= 1 && min <= max && max <= 127)
            && (!self.add_on
                || self.channel.is_none()
                    && self.transpose == 0
                    && (self.velocity_scale - 1.0).abs() < f32::EPSILON)
            && self
                .note_length_ticks
                .is_none_or(|length| (1..=96_000).contains(&length))
            && self.attack_compensation_ticks <= 96_000
            && self.separation_ticks <= 96_000
    }
}
