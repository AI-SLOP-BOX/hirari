impl HirariCore {
    pub fn set_midi_note(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    ) {
        if let Some(e) = self.engine.as_ref() {
            e.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        }
        if pitch <= 127
            && velocity != 0
            && length_samples != 0
            && start_sample.checked_add(length_samples).is_some()
        {
            if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                let previous = notes
                    .iter()
                    .find(|existing| {
                        existing.track_id == track_id
                            && existing.region_id == 0
                            && existing.pitch == pitch
                            && existing.midi_channel == 0
                            && existing.start_sample == start_sample
                    })
                    .cloned();
                notes.retain(|existing| {
                    !(existing.track_id == track_id
                        && existing.region_id == 0
                        && existing.pitch == pitch
                        && existing.midi_channel == 0
                        && existing.start_sample == start_sample)
                });
                let note = crate::project_contracts::MidiNoteContract {
                    region_id: previous.as_ref().map(|note| note.region_id).unwrap_or(0),
                    midi_channel: 0,
                    articulation: previous.as_ref().map(|note| note.articulation).unwrap_or(0),
                    track_id,
                    pitch,
                    velocity,
                    start_sample,
                    length_samples,
                    lyric: previous
                        .as_ref()
                        .map(|note| note.lyric.clone())
                        .unwrap_or_default(),
                    phoneme: previous
                        .as_ref()
                        .map(|note| note.phoneme.clone())
                        .unwrap_or_default(),
                    pitch_curve_cents: previous
                        .as_ref()
                        .map(|note| note.pitch_curve_cents.clone())
                        .unwrap_or_default(),
                    vibrato_depth_cents: previous
                        .as_ref()
                        .map(|note| note.vibrato_depth_cents)
                        .unwrap_or_default(),
                    vibrato_rate_millihz: previous
                        .as_ref()
                        .map(|note| note.vibrato_rate_millihz)
                        .unwrap_or(5_000),
                    portamento_samples: previous
                        .as_ref()
                        .map(|note| {
                            note.portamento_samples
                                .min(length_samples.min(u64::from(u32::MAX)) as u32)
                        })
                        .unwrap_or_default(),
                    probability: previous
                        .as_ref()
                        .map(|note| note.probability)
                        .unwrap_or(100),
                    repeat_count: previous.as_ref().map(|note| note.repeat_count).unwrap_or(1),
                };
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
    }

    pub fn set_midi_note_diagnostic_json(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    ) -> String {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_note",
                "retryable": false,
            })
            .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            let lyric = notes
                .iter()
                .find(|existing| {
                    existing.track_id == track_id
                        && existing.region_id == 0
                        && existing.pitch == pitch
                        && existing.midi_channel == 0
                        && existing.start_sample == start_sample
                })
                .map(|existing| existing.lyric.clone())
                .unwrap_or_default();
            notes.retain(|existing| {
                !(existing.track_id == track_id
                    && existing.region_id == 0
                    && existing.pitch == pitch
                    && existing.midi_channel == 0
                    && existing.start_sample == start_sample)
            });
            let note = crate::project_contracts::MidiNoteContract {
                region_id: 0,
                midi_channel: 0,
                articulation: 0,
                track_id,
                pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                vibrato_rate_millihz: 5_000,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            };
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_midi_note",
            "track_id": track_id,
            "pitch": pitch,
            "velocity": velocity,
            "start_sample": start_sample,
            "length_samples": length_samples,
        })
        .to_string()
    }

    pub fn set_midi_note_lyric_diagnostic_json(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        lyric: &str,
    ) -> String {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
            || lyric.len() > 1_024
            || lyric.contains('\0')
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_note_or_lyric",
                "retryable": false,
            })
            .to_string();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if !self.set_midi_note_lyric(
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples,
            lyric,
        ) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_note_rejected",
                "retryable": true,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_midi_note",
            "track_id": track_id,
            "pitch": pitch,
            "velocity": velocity,
            "start_sample": start_sample,
            "length_samples": length_samples,
            "lyric": lyric,
        })
        .to_string()
    }

    /// Schedule a note and keep its vocal lyric in the canonical control
    /// plane. The native realtime note remains allocation-free; the lyric is
    /// intentionally persisted only in the project-side model.
    pub fn set_midi_note_lyric(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        lyric: &str,
    ) -> bool {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
            || lyric.len() > 1_024
            || lyric.contains('\0')
        {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let before_metadata = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        let before_vibrato_rates = self
            .midi_vibrato_rates
            .lock()
            .map(|rates| rates.clone())
            .unwrap_or_default();
        let previous = before_metadata.iter().find(|note| {
            note.track_id == track_id
                && note.region_id == 0
                && note.pitch == pitch
                && note.midi_channel == 0
                && note.start_sample == start_sample
        });
        let undo_before = engine.get_undo_count();
        engine.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        let Ok(mut notes) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        // The native note is upserted by (track, pitch, start). Mirror the
        // same identity here so changing length/velocity while entering a
        // lyric cannot leave stale duplicate authoring records.
        notes.retain(|note| {
            !(note.track_id == track_id
                && note.region_id == 0
                && note.pitch == pitch
                && note.midi_channel == 0
                && note.start_sample == start_sample)
        });
        notes.push(crate::project_contracts::MidiNoteContract {
            region_id: previous.map(|note| note.region_id).unwrap_or(0),
            midi_channel: 0,
            articulation: previous.map(|note| note.articulation).unwrap_or(0),
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples,
            lyric: lyric.to_owned(),
            phoneme: previous
                .map(|note| note.phoneme.clone())
                .unwrap_or_default(),
            pitch_curve_cents: previous
                .map(|note| note.pitch_curve_cents.clone())
                .unwrap_or_default(),
            vibrato_depth_cents: previous
                .map(|note| note.vibrato_depth_cents)
                .unwrap_or_default(),
            vibrato_rate_millihz: previous
                .map(|note| note.vibrato_rate_millihz)
                .unwrap_or(5_000),
            portamento_samples: previous
                .map(|note| {
                    note.portamento_samples
                        .min(length_samples.min(u64::from(u32::MAX)) as u32)
                })
                .unwrap_or_default(),
            probability: previous.map(|note| note.probability).unwrap_or(100),
            repeat_count: previous.map(|note| note.repeat_count).unwrap_or(1),
        });
        let after_metadata = notes.clone();
        drop(notes);
        let depth_after = engine.get_undo_count();
        if depth_after > undo_before {
            if let Ok(mut history) = self.midi_lyric_history.lock() {
                history.retain(|entry| entry.depth_after <= undo_before);
                history.push(crate::MidiLyricHistoryEntry {
                    depth_after,
                    before: before_metadata,
                    after: after_metadata,
                    before_vibrato_rates,
                    after_vibrato_rates: self
                        .midi_vibrato_rates
                        .lock()
                        .map(|rates| rates.clone())
                        .unwrap_or_default(),
                });
            }
        }
        true
    }

    /// Update non-destructive vocal articulation without touching the
    /// realtime MIDI event. This keeps pitch curves and pronunciation safe
    /// for an editor while playback remains allocation-free.
    pub fn set_midi_note_articulation(
        &self,
        track_id: u32,
        pitch: u8,
        start_sample: u64,
        phoneme: &str,
        pitch_curve_cents: &[i16],
        vibrato_depth_cents: u16,
        portamento_samples: u32,
    ) -> bool {
        if pitch > 127
            || phoneme.len() > 128
            || phoneme.contains('\0')
            || pitch_curve_cents.len() > 256
        {
            return false;
        }
        let Ok(mut notes) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        let Some(note) = notes.iter_mut().find(|note| {
            note.track_id == track_id
                && note.region_id == 0
                && note.pitch == pitch
                && note.start_sample == start_sample
        }) else {
            return false;
        };
        note.phoneme = phoneme.to_owned();
        note.pitch_curve_cents = pitch_curve_cents.to_vec();
        note.vibrato_depth_cents = vibrato_depth_cents;
        note.portamento_samples =
            portamento_samples.min(note.length_samples.min(u64::from(u32::MAX)) as u32);
        let snapshot = notes.clone();
        drop(notes);
        // `set_midi_note` creates the native undo point immediately before
        // this metadata refinement. Extend that same entry so one user edit
        // undoes both the realtime note and its vocal articulation.
        let inside_midi_edit_transaction = self
            .midi_note_transaction_before
            .lock()
            .map(|pending| pending.is_some())
            .unwrap_or(false);
        if !inside_midi_edit_transaction {
            if let Some(engine) = self.engine.as_ref() {
                let depth = engine.get_undo_count();
                if let Ok(mut history) = self.midi_lyric_history.lock() {
                    if let Some(entry) = history
                        .iter_mut()
                        .rev()
                        .find(|entry| entry.depth_after == depth)
                    {
                        entry.after = snapshot;
                    }
                }
            }
        }
        true
    }

    /// Atomically replace the realtime MIDI snapshot. Each packed note is
    /// `[track, pitch, velocity, channel, start_sample, length_samples,
    /// probability_percent, region_id, articulation_id]`. The UI keeps lyrics and other authoring
    /// metadata in the canonical project model; this bounded form is the
    /// allocation-safe playback snapshot.
    pub fn replace_midi_notes(&self, packed: Vec<u64>, record_undo: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.replace_midi_notes(packed, record_undo))
    }

    /// Replace realtime notes and their authoring metadata as one undoable
    /// operation. This is the bridge used by data-driven MIDI editors.
    pub fn replace_midi_note_contracts(
        &self,
        notes: Vec<crate::project_contracts::MidiNoteContract>,
        record_undo: bool,
    ) -> bool {
        let packed = notes
            .iter()
            .flat_map(|note| {
                [
                    u64::from(note.track_id),
                    u64::from(note.pitch),
                    u64::from(note.velocity),
                    u64::from(note.midi_channel),
                    note.start_sample,
                    note.length_samples,
                    u64::from(note.probability),
                    u64::from(note.region_id),
                    u64::from(note.articulation),
                ]
            })
            .collect::<Vec<_>>();
        if notes.iter().any(|note| note.validate().is_err()) {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let inside_midi_edit_transaction = self
            .midi_note_transaction_before
            .lock()
            .map(|pending| pending.is_some())
            .unwrap_or(false);
        let direct_undo_before = if record_undo && !inside_midi_edit_transaction {
            Some((
                engine.get_undo_count(),
                self.scheduled_midi_notes
                    .lock()
                    .map(|current| current.clone())
                    .unwrap_or_default(),
                self.midi_vibrato_rates
                    .lock()
                    .map(|rates| rates.clone())
                    .unwrap_or_default(),
            ))
        } else {
            None
        };
        if !engine.replace_midi_notes(packed, record_undo) {
            return false;
        }
        let valid_vibrato_rate_keys = notes
            .iter()
            .map(|note| (note.track_id, note.region_id, note.pitch, note.start_sample))
            .collect::<std::collections::HashSet<_>>();
        let note_vibrato_rates = notes
            .iter()
            .map(|note| {
                (
                    (note.track_id, note.region_id, note.pitch, note.start_sample),
                    note.vibrato_rate_millihz,
                )
            })
            .collect::<Vec<_>>();
        if let Ok(mut metadata) = self.scheduled_midi_notes.lock() {
            *metadata = notes;
        } else {
            return false;
        }
        if let Ok(mut rates) = self.midi_vibrato_rates.lock() {
            rates.retain(|key, _| valid_vibrato_rate_keys.contains(key));
            rates.extend(note_vibrato_rates);
        } else {
            return false;
        }
        if let Some((depth_before, before, before_vibrato_rates)) = direct_undo_before {
            let depth_after = engine.get_undo_count();
            if depth_after > depth_before {
                let after = self
                    .scheduled_midi_notes
                    .lock()
                    .map(|current| current.clone())
                    .unwrap_or_default();
                let after_vibrato_rates = self
                    .midi_vibrato_rates
                    .lock()
                    .map(|rates| rates.clone())
                    .unwrap_or_default();
                if let Ok(mut history) = self.midi_lyric_history.lock() {
                    history.retain(|entry| entry.depth_after <= depth_before);
                    history.push(crate::MidiLyricHistoryEntry {
                        depth_after,
                        before,
                        after,
                        before_vibrato_rates,
                        after_vibrato_rates,
                    });
                }
            }
        }
        true
    }

    /// Return the nine-word packed realtime note snapshot used by
    /// `replace_midi_notes`.
    pub fn midi_notes_snapshot(&self) -> Vec<u64> {
        self.engine
            .as_ref()
            .map(|engine| engine.midi_notes_snapshot())
            .unwrap_or_default()
    }

    /// Reconcile the authoring mirror after native MIDI undo/redo. Native
    /// history owns the realtime snapshot; this keeps project saves and
    /// bounce extent calculations on the same note set without discarding
    /// lyrics attached to unchanged notes.
    fn sync_midi_note_metadata_from_engine(&self) {
        let packed = self.midi_notes_snapshot();
        let Ok(mut metadata) = self.scheduled_midi_notes.lock() else {
            return;
        };
        let old = std::mem::take(&mut *metadata);
        let mut synced = Vec::with_capacity(packed.len() / 9);
        for chunk in packed.chunks_exact(9) {
            let previous = old
                .iter()
                .find(|note| {
                    note.track_id as u64 == chunk[0]
                        && note.region_id as u64 == chunk[7]
                        && note.pitch as u64 == chunk[1]
                        && note.velocity as u64 == chunk[2]
                        && note.midi_channel as u64 == chunk[3]
                        && note.start_sample == chunk[4]
                        && note.length_samples == chunk[5]
                })
                .or_else(|| {
                    old.iter().find(|note| {
                        note.track_id as u64 == chunk[0]
                            && note.region_id as u64 == chunk[7]
                            && note.pitch as u64 == chunk[1]
                            && note.midi_channel as u64 == chunk[3]
                            && note.start_sample == chunk[4]
                    })
                });
            synced.push(crate::project_contracts::MidiNoteContract {
                region_id: chunk[7] as u32,
                midi_channel: chunk[3] as u8,
                articulation: chunk[8] as u8,
                track_id: chunk[0] as u32,
                pitch: chunk[1] as u8,
                velocity: chunk[2] as u8,
                start_sample: chunk[4],
                length_samples: chunk[5],
                lyric: previous.map(|note| note.lyric.clone()).unwrap_or_default(),
                phoneme: previous
                    .map(|note| note.phoneme.clone())
                    .unwrap_or_default(),
                pitch_curve_cents: previous
                    .map(|note| note.pitch_curve_cents.clone())
                    .unwrap_or_default(),
                vibrato_depth_cents: previous
                    .map(|note| note.vibrato_depth_cents)
                    .unwrap_or_default(),
                vibrato_rate_millihz: previous
                    .map(|note| note.vibrato_rate_millihz)
                    .unwrap_or(5_000),
                portamento_samples: previous
                    .map(|note| note.portamento_samples)
                    .unwrap_or_default(),
                probability: chunk[6] as u8,
                repeat_count: previous.map(|note| note.repeat_count).unwrap_or(1),
            });
        }
        *metadata = synced;
    }

    pub fn clear_midi_note_metadata(&self) -> bool {
        let Ok(mut notes) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        notes.clear();
        drop(notes);
        let Ok(mut rates) = self.midi_vibrato_rates.lock() else {
            return false;
        };
        rates.clear();
        true
    }

    /// Canonical authoring view for piano-roll and vocal editors. Unlike the
    /// realtime packed snapshot, this preserves lyric metadata and is safe
    /// for UI/JSON consumers to inspect.
    pub fn midi_notes_json(&self) -> String {
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        notes.sort_by_key(|note| (note.track_id, note.start_sample, note.pitch));
        let enriched = notes
            .into_iter()
            .map(|note| {
                let mut value =
                    serde_json::to_value(&note).unwrap_or_else(|_| serde_json::json!({}));
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "vibrato_rate_millihz".to_owned(),
                        serde_json::Value::from(note.vibrato_rate_millihz),
                    );
                    object.insert(
                        "drum_lane".to_owned(),
                        serde_json::Value::String(
                            crate::piano_roll_editor::drum_lane_label(note.pitch).to_owned(),
                        ),
                    );
                }
                value
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&enriched).unwrap_or_else(|_| "[]".to_owned())
    }
}
