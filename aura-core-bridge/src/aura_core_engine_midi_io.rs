impl AuraCore {
    pub fn set_midi_events_json(&self, snapshot: &str) -> bool {
        let Ok(events) = serde_json::from_str::<Vec<midi::MIDIEvent>>(snapshot) else {
            return false;
        };
        if events.iter().any(|event| !event.validate()) {
            return false;
        }
        let mut events = events;
        if !midi::MIDIOrchestrator::normalize_events(&mut events) {
            return false;
        }
        let Ok(mut current) = self.midi_events.lock() else {
            return false;
        };
        *current = events;
        true
    }

    /// Structured counterpart for CLI/UI callers. The legacy bool API remains
    /// for ABI compatibility, but parse, validation, normalization, and lock
    /// failures must not collapse into the same silent `false` result.
    pub fn set_midi_events_diagnostic_json(&self, snapshot: &str) -> String {
        let events = match serde_json::from_str::<Vec<midi::MIDIEvent>>(snapshot) {
            Ok(events) => events,
            Err(error) => {
                return serde_json::json!({
                    "code": "invalid_midi_snapshot",
                    "message": error.to_string(),
                    "retryable": false
                })
                .to_string();
            }
        };
        if events.iter().any(|event| !event.validate()) {
            return serde_json::json!({
                "code": "invalid_midi_event",
                "message": "one or more MIDI events failed validation",
                "retryable": false
            })
            .to_string();
        }
        let mut normalized = events;
        if !midi::MIDIOrchestrator::normalize_events(&mut normalized) {
            return serde_json::json!({
                "code": "midi_normalization_rejected",
                "message": "MIDI event normalization was rejected",
                "retryable": false
            })
            .to_string();
        }
        let Ok(mut current) = self.midi_events.lock() else {
            return serde_json::json!({
                "code": "midi_state_unavailable",
                "message": "MIDI state lock is poisoned",
                "retryable": true
            })
            .to_string();
        };
        *current = normalized;
        serde_json::json!({"ok": true, "operation": "set_midi_events"}).to_string()
    }

    pub fn apply_midi_swing(&self, subdivision_beats: f32, amount: f32) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::apply_swing(&mut events, subdivision_beats, amount);
        drop(events);
        result && self.apply_swing_scheduled_midi(subdivision_beats, amount)
    }

    fn apply_swing_scheduled_midi(&self, subdivision_beats: f32, amount: f32) -> bool {
        let subdivision = self.beats_to_samples(subdivision_beats as f64);
        if subdivision == 0 {
            return false;
        }
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            let cell = note.start_sample / subdivision;
            if !cell.is_multiple_of(2) {
                let offset = (subdivision as f64 * 0.5 * amount as f64).round();
                let next = note.start_sample as f64 + offset;
                if !next.is_finite() || next < 0.0 || next > u64::MAX as f64 {
                    return false;
                }
                note.start_sample = next as u64;
            }
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn apply_midi_swing_diagnostic_json(&self, subdivision_beats: f32, amount: f32) -> String {
        let result = if !subdivision_beats.is_finite()
            || !(0.001..=16.0).contains(&subdivision_beats)
            || !amount.is_finite()
            || !(-1.0..=1.0).contains(&amount)
        {
            crate::bridge_error::BridgeError::new(
                "invalid_midi_swing",
                "subdivision and swing amount are out of range",
            )
        } else {
            match self.midi_events.lock() {
                Err(_) => crate::bridge_error::BridgeError::new(
                    "midi_state_unavailable",
                    "MIDI state lock is poisoned",
                )
                .retryable(true),
                Ok(mut events) => {
                    if midi::MIDIOrchestrator::apply_swing(&mut events, subdivision_beats, amount) {
                        drop(events);
                        if !self.apply_swing_scheduled_midi(subdivision_beats, amount) {
                            return serde_json::json!({"ok":false,"code":"scheduled_midi_swing_rejected","retryable":false}).to_string();
                        }
                        return "{\"ok\":true,\"operation\":\"apply_midi_swing\"}".to_owned();
                    }
                    crate::bridge_error::BridgeError::new(
                        "midi_swing_rejected",
                        "MIDI swing operation was rejected",
                    )
                }
            }
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    fn quantize_scheduled_midi(&self, grid_beats: f32, strength: f32) -> bool {
        let grid_samples = self.beats_to_samples(grid_beats as f64);
        if grid_samples == 0 {
            return false;
        }
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            let target = ((note.start_sample as f64 / grid_samples as f64).round()
                * grid_samples as f64) as u64;
            let position = note.start_sample as f64
                + (target as f64 - note.start_sample as f64) * strength as f64;
            if !position.is_finite() || position < 0.0 || position > u64::MAX as f64 {
                return false;
            }
            note.start_sample = position.round() as u64;
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn quantize_midi(&self, grid_beats: f32, strength: f32) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::quantize(&mut events, grid_beats, strength);
        drop(events);
        result && self.quantize_scheduled_midi(grid_beats, strength)
    }

    pub fn quantize_midi_diagnostic_json(&self, grid_beats: f32, strength: f32) -> String {
        if !grid_beats.is_finite()
            || !(0.001..=16.0).contains(&grid_beats)
            || !strength.is_finite()
            || !(0.0..=1.0).contains(&strength)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_quantize",
                "retryable": false,
            })
            .to_string();
        }
        let Ok(mut events) = self.midi_events.lock() else {
            return serde_json::json!({
                "ok": false,
                "code": "midi_state_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        if !midi::MIDIOrchestrator::quantize(&mut events, grid_beats, strength) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_quantize_rejected",
                "retryable": false,
            })
            .to_string();
        }
        let event_count = events.len();
        drop(events);
        if !self.quantize_scheduled_midi(grid_beats, strength) {
            return serde_json::json!({
                "ok": false,
                "code": "scheduled_midi_quantize_rejected",
                "retryable": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "quantize_midi",
            "grid_beats": grid_beats,
            "strength": strength,
            "event_count": event_count,
        })
        .to_string()
    }

    pub fn humanize_midi(&self, timing_beats: f32, velocity: i16, seed: u64) -> bool {
        let Ok(mut events) = self.midi_events.lock() else {
            return false;
        };
        let result = midi::MIDIOrchestrator::humanize(&mut events, timing_beats, velocity, seed);
        drop(events);
        result && self.humanize_scheduled_midi(timing_beats, velocity, seed)
    }

    fn humanize_scheduled_midi(&self, timing_beats: f32, velocity: i16, mut seed: u64) -> bool {
        let max_offset = self.beats_to_samples(timing_beats.abs() as f64);
        let Ok(current) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        if current.is_empty() {
            return true;
        }
        let mut updated = current.clone();
        for note in &mut updated {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let unit = ((seed >> 33) as f64 / (u32::MAX as f64)) * 2.0 - 1.0;
            let delta = (unit * max_offset as f64).round() as i128;
            let next = note.start_sample as i128 + delta;
            if next < 0 || next > u64::MAX as i128 {
                return false;
            }
            note.start_sample = next as u64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let velocity_delta = ((seed >> 33) % (velocity.unsigned_abs() as u64 + 1)) as i16;
            let signed_delta = if seed & 1 == 0 {
                velocity_delta
            } else {
                -velocity_delta
            };
            note.velocity = (note.velocity as i16 + signed_delta).clamp(1, 127) as u8;
        }
        drop(current);
        let packed = updated
            .iter()
            .flat_map(|note| {
                [
                    note.track_id as u64,
                    note.pitch as u64,
                    note.velocity as u64,
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, true) {
            return false;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = updated;
        }
        true
    }

    pub fn humanize_midi_diagnostic_json(
        &self,
        timing_beats: f32,
        velocity: i16,
        seed: u64,
    ) -> String {
        let result = if !timing_beats.is_finite()
            || !(-4.0..=4.0).contains(&timing_beats)
            || !(-127..=127).contains(&velocity)
        {
            crate::bridge_error::BridgeError::new(
                "invalid_midi_humanize",
                "timing and velocity variation are out of range",
            )
        } else {
            match self.midi_events.lock() {
                Err(_) => crate::bridge_error::BridgeError::new(
                    "midi_state_unavailable",
                    "MIDI state lock is poisoned",
                )
                .retryable(true),
                Ok(mut events) => {
                    if midi::MIDIOrchestrator::humanize(&mut events, timing_beats, velocity, seed) {
                        drop(events);
                        if !self.humanize_scheduled_midi(timing_beats, velocity, seed) {
                            return serde_json::json!({"ok":false,"code":"scheduled_midi_humanize_rejected","retryable":false}).to_string();
                        }
                        return "{\"ok\":true,\"operation\":\"humanize_midi\"}".to_owned();
                    }
                    crate::bridge_error::BridgeError::new(
                        "midi_humanize_rejected",
                        "MIDI humanize operation was rejected",
                    )
                }
            }
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
