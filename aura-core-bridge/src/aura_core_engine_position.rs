impl AuraCore {
    pub fn set_playhead(&self, pos: u64) {
        if let Some(e) = self.engine.as_ref() {
            e.set_playhead(pos);
            let sample_rate = self.get_sample_rate().round().max(1.0) as u64;
            let clock = crate::production_timeline::MasterClock {
                tick: crate::production_timeline::MasterTick(pos as u128),
                ticks_per_second: sample_rate,
            };
            self.publish_production_event(
                crate::production_events::ProductionEvent::PlayheadMoved { clock },
            );
        }
    }

    pub fn set_playhead_diagnostic_json(&self, pos: u64) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_playhead(pos);
        serde_json::json!({
            "ok": true,
            "operation": "set_playhead",
            "playhead": engine.get_playhead(),
        })
        .to_string()
    }
    pub fn set_test_tone(&self, enabled: bool) {
        if let Some(e) = self.engine.as_ref() {
            e.set_test_tone(enabled);
        }
    }

    pub fn set_test_tone_diagnostic_json(&self, enabled: bool) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_test_tone(enabled);
        serde_json::json!({"ok": true, "operation": "set_test_tone", "enabled": enabled})
            .to_string()
    }

    // Actions
    pub fn set_track_fader(&self, tid: u32, val: f32) {
        self.set_volume(tid, val);
    }
    pub fn set_track_fader_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_volume_diagnostic_json(tid, val)
    }
    pub fn set_track_pan(&self, tid: u32, val: f32) {
        self.set_pan(tid, val);
    }
    pub fn set_track_pan_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_pan_diagnostic_json(tid, val)
    }

    pub fn set_volume(&self, tid: u32, val: f32) -> bool {
        let changed = self.set_track_volume_with_stack(tid, val);
        if changed {
            self.publish_production_event(
                crate::production_events::ProductionEvent::TrackGainChanged {
                    track_id: tid,
                    gain: val,
                },
            );
        }
        changed
    }

    pub fn set_volume_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_track_scalar_diagnostic_json("volume", tid, val, |engine, id, value| {
            engine.set_track_volume(id, value)
        })
    }

    pub fn set_eq_diagnostic_json(
        &self,
        tid: u32,
        low_band: f32,
        low_cut: f32,
        high_band: f32,
        high_cut: f32,
    ) -> String {
        if [low_band, low_cut, high_band, high_cut]
            .iter()
            .any(|value| !value.is_finite())
        {
            return "{\"code\":\"invalid_parameter\",\"message\":\"EQ parameters must be finite\"}"
                .to_owned();
        }
        let ok = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_eq(tid, low_band, low_cut, high_band, high_cut));
        if ok {
            serde_json::json!({"ok":true,"operation":"set_eq","track_id":tid,"low_band":low_band,"low_cut":low_cut,"high_band":high_band,"high_cut":high_cut}).to_string()
        } else {
            serde_json::json!({"code":"eq_rejected","track_id":tid}).to_string()
        }
    }

    /// Apply a bounded gain-staging correction in dB to the current fader
    /// value. This is reversible through the command transaction/undo layer.
    pub fn apply_gain_staging_diagnostic_json(&self, tid: u32, gain_db: f32) -> String {
        if !gain_db.is_finite() || !(-24.0..=24.0).contains(&gain_db) {
            return serde_json::json!({"code":"invalid_parameter","message":"gain staging correction must be within -24..=24 dB","track_id":tid}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\"}".to_owned();
        };
        let before = engine.get_track_volume(tid);
        let after = (before * 10.0_f32.powf(gain_db / 20.0)).clamp(0.0, 2.0);
        if !engine.set_track_volume(tid, after) {
            return serde_json::json!({"code":"track_not_found","track_id":tid}).to_string();
        }
        serde_json::json!({"ok":true,"operation":"apply_gain_staging","track_id":tid,"gain_db":gain_db,"before":before,"after":after}).to_string()
    }
    pub fn set_track_delay_samples(&self, tid: u32, samples: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_delay_samples(tid, samples))
    }
    pub fn track_delay_samples(&self, tid: u32) -> Option<u32> {
        self.engine
            .as_ref()
            .map(|engine| engine.get_track_delay_samples(tid))
    }
    pub fn set_track_delay_samples_diagnostic_json(&self, tid: u32, samples: u32) -> String {
        if samples > 8192 {
            return serde_json::json!({"code":"invalid_parameter","message":"track delay must be within 0..=8192 samples","track_id":tid}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"code":"engine_unavailable","retryable":true}).to_string();
        };
        if engine.set_track_delay_samples(tid, samples) {
            serde_json::json!({"ok":true,"field":"track_delay_samples","track_id":tid,"value":samples}).to_string()
        } else {
            serde_json::json!({"code":"track_not_found_or_rejected","track_id":tid}).to_string()
        }
    }
    pub fn track_volume(&self, tid: u32) -> Option<f32> {
        self.engine.as_ref().and_then(|engine| {
            let value = engine.get_track_volume(tid);
            value.is_finite().then_some(value)
        })
    }
    pub fn set_pan(&self, tid: u32, val: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_track_pan(tid, val))
    }

    pub fn set_pan_diagnostic_json(&self, tid: u32, val: f32) -> String {
        self.set_track_scalar_diagnostic_json("pan", tid, val, |engine, id, value| {
            engine.set_track_pan(id, value)
        })
    }

    fn set_track_scalar_diagnostic_json<F>(
        &self,
        field: &str,
        tid: u32,
        val: f32,
        apply: F,
    ) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine, u32, f32) -> bool,
    {
        if !val.is_finite() {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_parameter",
                format!("track {field} must be finite"),
            )
            .object(format!("track:{tid}"));
            return serde_json::to_string(&error).unwrap_or_else(|_| {
                "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
            });
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine, tid, val) {
            return format!("{{\"ok\":true,\"field\":\"{field}\",\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                format!("track {tid} rejected {field} update"),
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result).unwrap_or_else(|_| {
            "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
        })
    }
}
