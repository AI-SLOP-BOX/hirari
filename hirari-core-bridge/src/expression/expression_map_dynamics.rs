impl DynamicsMap {
    pub fn initialized(range: DynamicRange) -> Self {
        let symbols = [
            DynamicSymbol::Pppp,
            DynamicSymbol::Ppp,
            DynamicSymbol::Pp,
            DynamicSymbol::P,
            DynamicSymbol::Mp,
            DynamicSymbol::Mf,
            DynamicSymbol::F,
            DynamicSymbol::Ff,
            DynamicSymbol::Fff,
            DynamicSymbol::Ffff,
        ];
        let entries = symbols
            .into_iter()
            .enumerate()
            .map(|(index, symbol)| DynamicMappingEntry {
                symbol,
                velocity_percent: 25.0 + index as f32 * (125.0 / 9.0),
                volume_value: (16.0 + index as f32 * (111.0 / 9.0)).round() as u8,
                controller_value: (16.0 + index as f32 * (111.0 / 9.0)).round() as u8,
            })
            .collect();
        Self {
            range,
            change_velocities: true,
            volume_output: DynamicVolumeOutput::ExpressionCc11,
            send_controller: None,
            entries,
        }
    }

    pub fn apply(&self, symbol: DynamicSymbol, input_velocity: u8) -> Option<RenderedDynamics> {
        if !self.validate() || !(1..=127).contains(&input_velocity) {
            return None;
        }
        if self.range == DynamicRange::PpToFf
            && matches!(
                symbol,
                DynamicSymbol::Pppp | DynamicSymbol::Ppp | DynamicSymbol::Fff | DynamicSymbol::Ffff
            )
        {
            return Some(RenderedDynamics {
                velocity: input_velocity,
                midi_outputs: Vec::new(),
                vst3_volume: None,
            });
        }
        let entry = self.entries.iter().find(|entry| entry.symbol == symbol)?;
        let velocity = if self.change_velocities {
            (f32::from(input_velocity) * entry.velocity_percent / 100.0)
                .round()
                .clamp(1.0, 127.0) as u8
        } else {
            input_velocity
        };
        let mut midi_outputs = Vec::new();
        let mut vst3_volume = None;
        match self.volume_output {
            DynamicVolumeOutput::Off => {}
            DynamicVolumeOutput::MainVolumeCc7 => midi_outputs.push(MidiOutput::ControlChange {
                controller: 7,
                value: entry.volume_value,
            }),
            DynamicVolumeOutput::ExpressionCc11 => midi_outputs.push(MidiOutput::ControlChange {
                controller: 11,
                value: entry.volume_value,
            }),
            DynamicVolumeOutput::Vst3Volume => {
                vst3_volume = Some(f32::from(entry.volume_value) / 127.0)
            }
        }
        if let Some(controller) = self.send_controller {
            midi_outputs.push(MidiOutput::ControlChange {
                controller,
                value: entry.controller_value,
            });
        }
        Some(RenderedDynamics {
            velocity,
            midi_outputs,
            vst3_volume,
        })
    }

    pub fn validate(&self) -> bool {
        self.entries.len() == 10
            && self
                .send_controller
                .is_none_or(|controller| controller < 128)
            && self.entries.iter().all(|entry| {
                entry.velocity_percent.is_finite()
                    && (0.0..=800.0).contains(&entry.velocity_percent)
                    && entry.volume_value < 128
                    && entry.controller_value < 128
            })
            && self.entries.iter().enumerate().all(|(index, entry)| {
                self.entries[..index]
                    .iter()
                    .all(|previous| previous.symbol != entry.symbol)
            })
    }
}
