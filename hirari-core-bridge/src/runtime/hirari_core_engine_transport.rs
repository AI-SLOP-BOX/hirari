impl HirariCore {
    pub fn set_playing(&self, p: bool) {
        if let Some(e) = self.engine_opt() {
            let _ = crate::engine_transport::try_set_playing(e, e.get_core(), p);
        }
    }

    pub fn try_set_playing(&self, p: bool) -> bool {
        let changed = self
            .engine_opt()
            .is_some_and(|engine| crate::engine_transport::try_set_playing(engine, engine.get_core(), p));
        if changed {
            self.publish_production_event(if p {
                crate::production_events::ProductionEvent::TransportStarted
            } else {
                crate::production_events::ProductionEvent::TransportStopped
            });
        }
        changed
    }

    pub fn set_playing_diagnostic_json(&self, playing: bool) -> String {
        if self.engine_opt().is_none() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if self.try_set_playing(playing) {
            return serde_json::json!({
                "ok": true,
                "operation": "set_playing",
                "playing": playing,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "transport_rejected",
            "retryable": true,
            "playing": playing,
        })
        .to_string()
    }
    pub fn set_loop(&self, enabled: bool) {
        if let Some(e) = self.engine_opt() {
            e.set_loop(enabled);
        }
    }

    pub fn is_loop_enabled(&self) -> bool {
        self.engine_opt()
            .is_some_and(|engine| engine.is_loop_enabled())
    }

    pub fn is_metronome_enabled(&self) -> bool {
        self.engine_opt()
            .is_some_and(|engine| engine.is_metronome_enabled())
    }

    pub fn set_metronome_enabled(&self, enabled: bool) {
        if let Some(e) = self.engine_opt() {
            e.set_metronome_enabled(enabled);
        }
    }

    pub fn cycle_start_sample(&self) -> u64 {
        self.engine_opt()
            .map_or(0, |engine| engine.cycle_start())
    }

    pub fn cycle_end_sample(&self) -> u64 {
        self.engine_opt().map_or(0, |engine| engine.cycle_end())
    }

    pub fn set_loop_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine_opt() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_loop(enabled);
        serde_json::json!({"ok": true, "operation": "set_loop", "enabled": enabled}).to_string()
    }

    pub fn set_metronome_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine_opt() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_metronome_enabled(enabled);
        serde_json::json!({
            "ok": true,
            "operation": "set_metronome",
            "enabled": engine.is_metronome_enabled(),
        })
        .to_string()
    }

    pub fn set_cycle_range_diagnostic_json(
        &self,
        start_sample: u64,
        end_sample: u64,
        enabled: bool,
    ) -> String {
        let Some(engine) = self.engine_opt() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if start_sample >= end_sample || !engine.set_cycle_range(start_sample, end_sample, enabled)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_cycle_range",
                "retryable": false,
                "start_sample": start_sample,
                "end_sample": end_sample,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_cycle_range",
            "enabled": enabled,
            "start_sample": start_sample,
            "end_sample": end_sample,
        })
        .to_string()
    }
    pub fn is_playing(&self) -> bool {
        self.engine_opt().is_some_and(|e| e.is_playing())
    }
    pub fn get_playhead(&self) -> u64 {
        self.engine_opt().map_or(0, |e| e.get_playhead())
    }
    pub fn samples_to_beats(&self, samples: u64) -> f64 {
        self.engine_opt()
            .map_or(0.0, |e| e.samples_to_beats(samples))
    }
    pub fn beats_to_samples(&self, beats: f64) -> u64 {
        if !beats.is_finite() || beats <= 0.0 {
            return 0;
        }
        self.engine_opt()
            .map_or(0, |e| e.beats_to_samples(beats))
    }
    pub fn get_tempo_events(&self) -> Vec<f64> {
        self.tempo_read_events()
    }
    pub fn get_time_signature_events(&self) -> Vec<f64> {
        self.tempo_read_signatures()
    }
    pub fn set_time_signature_event(&self, beat: f64, numerator: u8, denominator: u8) -> bool {
        if !beat.is_finite() || beat < 0.0 {
            return false;
        }
        self.tempo_write_signature(beat, numerator, denominator)
    }
    pub fn remove_time_signature_event(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        self.tempo_erase_signature(beat)
    }
    pub fn set_time_signature_event_diagnostic_json(
        &self,
        beat: f64,
        numerator: u8,
        denominator: u8,
    ) -> String {
        if !beat.is_finite()
            || beat < 0.0
            || numerator == 0
            || numerator > 32
            || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32)
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_time_signature",
                "retryable": false,
            })
            .to_string();
        }
        if self.set_time_signature_event(beat, numerator, denominator) {
            return serde_json::json!({
                "ok": true,
                "beat": beat,
                "numerator": numerator,
                "denominator": denominator,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "time_signature_rejected",
            "retryable": false,
        })
        .to_string()
    }
    pub fn set_tempo_event(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() {
            return false;
        }
        let changed = self.tempo_write_event(beat, bpm, ramp);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn set_tempo_transition_event(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() || !(20.0..=300.0).contains(&bpm) {
            return false;
        }
        let changed = self.tempo_write_transition(beat, bpm, ramp);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn set_tempo_event_diagnostic_json(&self, beat: f64, bpm: f64, ramp: bool) -> String {
        if !beat.is_finite() || !bpm.is_finite() {
            return "{\"code\":\"non_finite_tempo_event\",\"retryable\":false}".to_owned();
        }
        if beat < 0.0 || !(20.0..=300.0).contains(&bpm) {
            return "{\"code\":\"tempo_event_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_tempo_event(beat, bpm, ramp) {
            return serde_json::json!({"ok": true, "beat": beat, "bpm": bpm, "ramp": ramp})
                .to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "tempo_event_rejected",
                "tempo event was rejected",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn remove_tempo_event(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let changed = self.tempo_erase_event(beat);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn remove_tempo_event_diagnostic_json(&self, beat: f64) -> String {
        if !beat.is_finite() || beat < 0.0 {
            return "{\"code\":\"invalid_tempo_position\",\"retryable\":false}".to_owned();
        }
        if self.remove_tempo_event(beat) {
            return serde_json::json!({"ok": true, "beat": beat}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "tempo_event_not_found",
                "tempo event was not found",
            )
            .object(format!("tempo:{beat}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn move_tempo_event(&self, from_beat: f64, to_beat: f64) -> bool {
        if !from_beat.is_finite() || !to_beat.is_finite() || from_beat <= 0.0 || to_beat <= 0.0 {
            return false;
        }
        let changed = self.tempo_move_event(from_beat, to_beat);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TempoMapChanged,
            );
        }
        changed
    }
    pub fn get_tempo(&self) -> f32 {
        self.tempo_read()
    }
    pub fn set_tempo(&self, bpm: f32) -> bool {
        if !bpm.is_finite() {
            return false;
        }
        self.tempo_write(bpm)
    }
    pub fn set_tempo_diagnostic_json(&self, bpm: f32) -> String {
        if !bpm.is_finite() {
            return "{\"code\":\"non_finite_tempo\",\"retryable\":false}".to_owned();
        }
        if !(20.0..=300.0).contains(&bpm) {
            return "{\"code\":\"tempo_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_tempo(bpm) {
            return serde_json::json!({"ok": true, "bpm": bpm}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new("tempo_rejected", "tempo update was rejected")
                .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn master_gain(&self) -> f32 {
        self.engine_opt()
            .map_or(1.0, |engine| engine.get_master_gain())
    }

    pub fn set_master_gain(&self, value: f32) -> bool {
        self.engine_opt().is_some_and(|engine| {
            crate::master_edit_host::set_master_gain(
                engine,
                self.undo_state,
                self.log_state,
                value,
                true,
            )
        })
    }

    pub fn set_master_gain_diagnostic_json(&self, value: f32) -> String {
        if !value.is_finite() || !(0.0..=2.0).contains(&value) {
            return "{\"code\":\"master_gain_out_of_range\",\"retryable\":false}".to_owned();
        }
        if self.set_master_gain(value) {
            return serde_json::json!({"ok": true, "master_gain": value}).to_string();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "master_gain_rejected",
                "master gain update was rejected",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
