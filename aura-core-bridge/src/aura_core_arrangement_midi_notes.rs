impl AuraCore {
    pub fn clear_midi_notes(&self) {
        if let Some(e) = self.engine.as_ref() {
            e.clear_midi_notes();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.clear();
        }
    }

    pub fn clear_midi_notes_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.clear_midi_notes();
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.clear();
        }
        "{\"ok\":true,\"operation\":\"clear_midi_notes\"}".to_owned()
    }

    pub fn remove_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if start_sample >= end_sample {
            return "{\"ok\":false,\"code\":\"invalid_midi_range\",\"retryable\":false}".to_owned();
        }
        if !engine.remove_midi_notes_range(track_id, start_sample, end_sample) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_notes_not_found",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.retain(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                !(note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample)
            });
        }
        serde_json::json!({
            "ok": true,
            "operation": "remove_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
        })
        .to_string()
    }

    pub fn transpose_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if track_id == 0 || start_sample >= end_sample || !(-127..=127).contains(&semitones) {
            return "{\"ok\":false,\"code\":\"invalid_midi_transpose\",\"retryable\":false}"
                .to_owned();
        }
        let notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes,
            Err(_) => {
                return "{\"ok\":false,\"code\":\"midi_state_unavailable\",\"retryable\":true}"
                    .to_owned();
            }
        };
        {
            let out_of_range = notes.iter().any(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
                    && !(0..=127).contains(&(i32::from(note.pitch) + semitones))
            });
            if out_of_range {
                return serde_json::json!({
                    "ok": false,
                    "code": "midi_transpose_out_of_range",
                    "retryable": false,
                })
                .to_string();
            }
        }
        if !engine.transpose_midi_notes_range(track_id, start_sample, end_sample, semitones) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_transpose_rejected",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            for note in notes.iter_mut().filter(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
            }) {
                note.pitch = (i32::from(note.pitch) + semitones) as u8;
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "transpose_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
            "semitones": semitones,
        })
        .to_string()
    }

    pub fn move_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if track_id == 0 || start_sample >= end_sample {
            return "{\"ok\":false,\"code\":\"invalid_midi_move_range\",\"retryable\":false}"
                .to_owned();
        }
        let notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes,
            Err(_) => {
                return "{\"ok\":false,\"code\":\"midi_state_unavailable\",\"retryable\":true}"
                    .to_owned();
            }
        };
        {
            let moves_before_zero = delta_samples < 0
                && notes.iter().any(|note| {
                    let note_end = note.start_sample.saturating_add(note.length_samples);
                    note.track_id == track_id
                        && note.start_sample < end_sample
                        && note_end > start_sample
                        && note.start_sample < delta_samples.unsigned_abs()
                });
            if moves_before_zero {
                return serde_json::json!({
                    "ok": false,
                    "code": "midi_move_before_zero",
                    "retryable": false,
                })
                .to_string();
            }
        }
        if !engine.move_midi_notes_range(track_id, start_sample, end_sample, delta_samples) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_move_rejected",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            for note in notes.iter_mut().filter(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
            }) {
                note.start_sample = if delta_samples >= 0 {
                    note.start_sample.saturating_add(delta_samples as u64)
                } else {
                    note.start_sample
                        .saturating_sub(delta_samples.unsigned_abs())
                };
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "move_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
            "delta_samples": delta_samples,
        })
        .to_string()
    }
}
