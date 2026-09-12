impl AuraCore {
    pub fn set_project_scale_diagnostic_json(&self, root: i32, scale_type: i32) -> String {
        if !(0..=11).contains(&root) || !(0..=32).contains(&scale_type) {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_project_scale",
                    "root or scale type is outside the supported range",
                )
                .at_generation(self.project_generation()),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.set_project_scale(root, scale_type) {
            return format!("{{\"ok\":true,\"operation\":\"set_project_scale\",\"root\":{root},\"scale_type\":{scale_type}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "project_scale_rejected",
                "project scale was rejected by the native engine",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    // Telemetry
    pub fn get_all_peaks_l(&self, out: &mut Vec<f32>) {
        if let Some(e) = self.engine.as_ref() {
            let core = ffi::get_unified_engine(e);
            let pks = ffi::get_track_peaks_l_owned(core);
            out.clear();
            out.extend(pks);
        }
    }

    pub fn get_all_peaks_r(&self, out: &mut Vec<f32>) {
        if let Some(e) = self.engine.as_ref() {
            let core = ffi::get_unified_engine(e);
            let pks = ffi::get_track_peaks_r_owned(core);
            out.clear();
            out.extend(pks);
        }
    }

    pub fn get_cpu_total(&self) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.get_cpu_total_v() * 100.0)
    }
    pub fn get_buffer_size(&self) -> u32 {
        self.engine.as_ref().map_or(0, |e| e.get_block_size())
    }
    pub fn get_latency_ms(&self) -> f32 {
        self.engine.as_ref().map_or(0.0, |e| e.get_latency_ms())
    }
    pub fn get_track_latency_ms(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.get_track_latency_ms(track_id))
    }

    pub fn get_track_pdc_compensation_ms(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.get_track_pdc_compensation_ms(track_id))
    }

    pub fn set_low_latency_mode(&self, active: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_low_latency_mode(active))
    }

    pub fn low_latency_mode(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.low_latency_mode())
    }

    pub fn set_tonal_scale(&self, root: i32, scale_type: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_tonal_scale(root, scale_type))
    }

    pub fn is_note_in_tonal_scale(&self, midi_note: i32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.is_note_in_tonal_scale(midi_note))
    }

    pub fn generate_chord_notes(&self, root: i32, octave: i32, quality: u32) -> Vec<u8> {
        let quality = match quality {
            0 => crate::tonal::ChordQuality::Major,
            1 => crate::tonal::ChordQuality::Minor,
            2 => crate::tonal::ChordQuality::Diminished,
            3 => crate::tonal::ChordQuality::Dominant7,
            4 => crate::tonal::ChordQuality::Major7,
            5 => crate::tonal::ChordQuality::Minor7,
            _ => return Vec::new(),
        };
        crate::tonal::chord_notes(root, octave, quality)
    }

    pub fn suggest_next_chords(&self, last_chord_name: &str) -> Vec<String> {
        crate::harmonic::HarmonicOrchestrator::default().suggest_next_chords(last_chord_name)
    }

    pub fn generate_arpeggio(
        &self,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    ) -> Vec<(u8, u8)> {
        if pitches.is_empty() || pitches.len() != velocities.len() || steps == 0 {
            return Vec::new();
        }
        let pattern = match pattern {
            0 => crate::arpeggio::ArpPattern::Up,
            1 => crate::arpeggio::ArpPattern::Down,
            2 => crate::arpeggio::ArpPattern::UpDown,
            3 => crate::arpeggio::ArpPattern::Random,
            _ => return Vec::new(),
        };
        let mut arp = crate::arpeggio::ArpeggioOrchestrator::new();
        arp.pattern = pattern;
        arp.octaves = octaves.clamp(1, 4);
        arp.update_notes(
            pitches
                .into_iter()
                .zip(velocities)
                .map(|(pitch, velocity)| crate::arpeggio::HeldNote { pitch, velocity })
                .collect(),
        );
        (0..steps).filter_map(|_| arp.get_next_note()).collect()
    }

    pub fn place_arpeggio(
        &self,
        track_id: u32,
        start_sample: u64,
        step_samples: u64,
        gate_samples: u64,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    ) -> usize {
        if step_samples == 0 || gate_samples == 0 || gate_samples > step_samples {
            return 0;
        }
        let generated = self.generate_arpeggio(pitches, velocities, pattern, octaves, steps);
        let Some(engine) = self.engine.as_ref() else {
            return 0;
        };
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        for (index, (pitch, velocity)) in generated.iter().enumerate() {
            let position = start_sample.saturating_add(step_samples.saturating_mul(index as u64));
            let lyric = notes
                .iter()
                .find(|note| {
                    note.track_id == track_id
                        && note.pitch == *pitch
                        && note.start_sample == position
                })
                .map(|note| note.lyric.clone())
                .unwrap_or_default();
            notes.retain(|note| {
                !(note.track_id == track_id
                    && note.pitch == *pitch
                    && note.start_sample == position)
            });
            notes.push(crate::project_contracts::MidiNoteContract {
                track_id,
                pitch: *pitch,
                velocity: *velocity,
                start_sample: position,
                length_samples: gate_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
        let mut packed = Vec::with_capacity(notes.len() * 5);
        for note in &notes {
            packed.extend([
                note.track_id as u64,
                note.pitch as u64,
                note.velocity as u64,
                note.start_sample,
                note.length_samples,
            ]);
        }
        if !engine.replace_midi_notes(packed, true) {
            return 0;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = notes;
        }
        generated.len()
    }

    /// Places a generated chord voicing into the canonical piano-roll model.
    pub fn place_generated_chord(
        &self,
        track_id: u32,
        start_sample: u64,
        length_samples: u64,
        velocity: u8,
        root: i32,
        octave: i32,
        quality: u32,
    ) -> usize {
        if velocity == 0 || length_samples == 0 {
            return 0;
        }
        let pitches = self.generate_chord_notes(root, octave, quality);
        if pitches.is_empty() {
            return 0;
        }
        let Some(engine) = self.engine.as_ref() else {
            return 0;
        };
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        for pitch in &pitches {
            let lyric = notes
                .iter()
                .find(|note| {
                    note.track_id == track_id
                        && note.pitch == *pitch
                        && note.start_sample == start_sample
                })
                .map(|note| note.lyric.clone())
                .unwrap_or_default();
            notes.retain(|note| {
                !(note.track_id == track_id
                    && note.pitch == *pitch
                    && note.start_sample == start_sample)
            });
            notes.push(crate::project_contracts::MidiNoteContract {
                track_id,
                pitch: *pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
        let mut packed = Vec::with_capacity(notes.len() * 5);
        for note in &notes {
            packed.extend([
                note.track_id as u64,
                note.pitch as u64,
                note.velocity as u64,
                note.start_sample,
                note.length_samples,
            ]);
        }
        if !engine.replace_midi_notes(packed, true) {
            return 0;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = notes;
        }
        pitches.len()
    }

    pub fn drum_lane_label(&self, pitch: u8) -> String {
        crate::piano_roll_editor::drum_lane_label(pitch).to_owned()
    }
    pub fn get_fft_bands(&self) -> Vec<f32> {
        self.get_spectral_data_v()
    }
    pub fn get_synesthesia_colors(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_synesthesia_colors_v()
    }
    pub fn get_motion_energy(&self) -> f32 {
        let Some(a) = self.analysis() else {
            return 0.0;
        };
        a.get_motion_energy()
    }
    pub fn get_spectral_partials_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_spectral_partials_v()
    }
    pub fn get_video_frame(&self) -> Vec<u8> {
        self.engine
            .as_ref()
            .map_or_else(Vec::new, |e| e.get_video_frame().into_iter().collect())
    }

    pub fn get_video_frame_revision(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.get_video_frame_revision())
    }

    pub fn request_video_frame(&self, seconds: f64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.request_video_frame(seconds))
    }

    pub fn load_video(&self, path: &str) -> bool {
        self.engine.as_ref().is_some_and(|e| e.load_video(path))
    }

    pub fn get_song_structure_json(&self) -> String {
        let Some(a) = self.analysis() else {
            return String::new();
        };
        ffi::get_song_structure_json_ffi(a)
    }

    pub fn get_ai_advice(&self) -> Vec<String> {
        let mut results = Vec::new();
        let Some(a) = self.analysis() else {
            return results;
        };
        results.push(a.get_creative_advice());
        results.push(a.get_arrangement_advice(0));
        results
    }

    // --- COMPATIBILITY PASS-THROUGHS FOR UI ---
    pub fn get_mixer_levels_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_mixer_levels_v()
    }
    pub fn get_spectral_data_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_spectral_data_v()
    }
    pub fn get_master_loudness(&self) -> LoudnessData {
        let Some(a) = self.analysis() else {
            return LoudnessData::default();
        };
        let v = ffi::get_master_loudness_v_ffi(a);
        LoudnessData {
            integrated: v.integrated,
            short_term: v.short_term,
            true_peak_l: v.true_peak_l,
            true_peak_r: v.true_peak_r,
            correlation: v.correlation,
        }
    }
    pub fn get_intelligence_dashboard_json(&self) -> String {
        let Some(a) = self.analysis() else {
            return String::new();
        };
        a.get_intelligence_dashboard_json()
    }
    pub fn get_clashing_frequencies(&self) -> Vec<ClashData> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        ffi::get_spectral_clash_v_ffi(a)
            .into_iter()
            .map(|c| ClashData {
                frequency: c.frequency,
                severity: c.severity,
            })
            .collect()
    }
}
