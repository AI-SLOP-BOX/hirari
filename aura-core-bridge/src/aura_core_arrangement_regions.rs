impl AuraCore {
    pub fn move_region(&self, tid: u32, rid: u32, start: f64) -> bool {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return false;
        }
        let start_samples = self.beats_to_samples(start.max(0.0)) as f64;
        self.engine
            .as_ref()
            .is_some_and(|e| e.move_region(tid, rid, start_samples))
    }
    pub fn split_region(&self, tid: u32, rid: u32, beat: f64) -> bool {
        if tid == 0 || rid == 0 || !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let split_samples = self.beats_to_samples(beat.max(0.0)) as f64;
        self.engine
            .as_ref()
            .is_some_and(|e| e.split_region(tid, rid, split_samples))
    }
    pub fn duplicate_region(&self, tid: u32, rid: u32, start: f64) -> u32 {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return 0;
        }
        self.engine.as_ref().map_or(0, |e| {
            e.duplicate_region(tid, rid, self.beats_to_samples(start))
        })
    }
    pub fn remove_region(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.remove_region(tid, rid))
    }
    pub fn set_region_gain(&self, tid: u32, rid: u32, gain: f32) -> bool {
        if !gain.is_finite() || !(-24.0..=24.0).contains(&gain) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_gain(tid, rid, gain))
    }
    pub fn set_region_range_edit(
        &self,
        tid: u32,
        rid: u32,
        start: u64,
        end: u64,
        gain: f32,
        fade_in: u32,
        fade_out: u32,
    ) -> bool {
        if tid == 0 || rid == 0 || start >= end || !gain.is_finite() || !(0.0..=4.0).contains(&gain) {
            return false;
        }
        self.engine.as_ref().is_some_and(|e| {
            e.set_region_range_edit(tid, rid, start, end, gain, fade_in as u64, fade_out as u64)
        })
    }
    pub fn clear_region_range_edits(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.clear_region_range_edits(tid, rid))
    }
    pub fn set_region_fades(&self, tid: u32, rid: u32, fade_in: f32, fade_out: f32) -> bool {
        if !fade_in.is_finite()
            || !fade_out.is_finite()
            || !(0.0..=1.0).contains(&fade_in)
            || !(0.0..=1.0).contains(&fade_out)
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_fades(tid, rid, fade_in, fade_out))
    }
    pub fn set_region_reverse(&self, tid: u32, rid: u32, reverse: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_reverse(tid, rid, reverse))
    }

    pub fn set_region_trim(&self, tid: u32, rid: u32, start_norm: f32, end_norm: f32) -> bool {
        if !start_norm.is_finite()
            || !end_norm.is_finite()
            || !(0.0..=1.0).contains(&start_norm)
            || !(0.0..=1.0).contains(&end_norm)
            || start_norm >= end_norm
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_trim(tid, rid, start_norm, end_norm))
    }

    pub fn set_region_warp_ratio(&self, tid: u32, rid: u32, ratio: f64) -> bool {
        if !ratio.is_finite() || !(0.25..=4.0).contains(&ratio) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_warp_ratio(tid, rid, ratio))
    }

    pub fn set_region_pitch_semitones(&self, tid: u32, rid: u32, semitones: f32) -> bool {
        if !semitones.is_finite() || !(-48.0..=48.0).contains(&semitones) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_pitch_semitones(tid, rid, semitones))
    }

    /// Upserts a VariAudio-style pitch/formant segment on an audio region.
    /// Segment edits stay non-destructive and are consumed by the native
    /// renderer on the next published region snapshot.
    pub fn set_region_audio_note_segment(
        &self,
        tid: u32,
        rid: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        formant_offset_cents: f64,
    ) -> bool {
        if !start_seconds.is_finite()
            || !end_seconds.is_finite()
            || end_seconds <= start_seconds
            || end_seconds - start_seconds > 24.0 * 60.0
            || !pitch_offset_cents.is_finite()
            || !formant_offset_cents.is_finite()
            || pitch_offset_cents.abs() > 4800.0
            || formant_offset_cents.abs() > 2400.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_region_audio_note_segment(
                tid,
                rid,
                start_seconds,
                end_seconds,
                pitch_offset_cents,
                formant_offset_cents,
            )
        })
    }

    pub fn set_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        formant_offset_cents: f64,
    ) -> String {
        let valid = start_seconds.is_finite()
            && end_seconds.is_finite()
            && end_seconds > start_seconds
            && end_seconds - start_seconds <= 24.0 * 60.0
            && pitch_offset_cents.is_finite()
            && pitch_offset_cents.abs() <= 4800.0
            && formant_offset_cents.is_finite()
            && formant_offset_cents.abs() <= 2400.0;
        if !valid {
            return "{\"code\":\"invalid_audio_note_segment\",\"retryable\":false}".to_owned();
        }
        if self.set_region_audio_note_segment(
            tid,
            rid,
            start_seconds,
            end_seconds,
            pitch_offset_cents,
            formant_offset_cents,
        ) {
            format!(
                "{{\"ok\":true,\"operation\":\"set_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_segment_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn set_region_audio_note_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> bool {
        if !segment_start_seconds.is_finite()
            || segment_start_seconds < 0.0
            || !position_seconds.is_finite()
            || position_seconds < 0.0
            || !pitch_cents.is_finite()
            || pitch_cents.abs() > 4800.0
            || !formant_cents.is_finite()
            || formant_cents.abs() > 2400.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_region_audio_note_anchor(
                tid,
                rid,
                segment_start_seconds,
                position_seconds,
                pitch_cents,
                formant_cents,
            )
        })
    }

    pub fn set_region_audio_note_anchor_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> String {
        if self.set_region_audio_note_anchor(
            tid,
            rid,
            segment_start_seconds,
            position_seconds,
            pitch_cents,
            formant_cents,
        ) {
            format!(
                "{{\"ok\":true,\"operation\":\"set_region_audio_note_anchor\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_anchor_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn clear_region_audio_note_segments(&self, tid: u32, rid: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.clear_region_audio_note_segments(tid, rid))
    }

    pub fn warp_region_audio_note_segment(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        new_start: f64,
        new_end: f64,
    ) -> bool {
        if !segment_start.is_finite()
            || segment_start < 0.0
            || !new_start.is_finite()
            || new_start < 0.0
            || !new_end.is_finite()
            || new_end <= new_start
            || new_end - new_start > 24.0 * 60.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.warp_region_audio_note_segment(tid, rid, segment_start, new_start, new_end)
        })
    }

    pub fn remove_region_audio_note_segment(&self, tid: u32, rid: u32, segment_start: f64) -> bool {
        if !segment_start.is_finite() || segment_start < 0.0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.remove_region_audio_note_segment(tid, rid, segment_start))
    }

    pub fn warp_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        new_start: f64,
        new_end: f64,
    ) -> String {
        if self.warp_region_audio_note_segment(tid, rid, segment_start, new_start, new_end) {
            format!("{{\"ok\":true,\"operation\":\"warp_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}", tid, rid)
        } else {
            "{\"code\":\"audio_note_segment_warp_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn remove_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
    ) -> String {
        if self.remove_region_audio_note_segment(tid, rid, segment_start) {
            format!("{{\"ok\":true,\"operation\":\"remove_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}", tid, rid)
        } else {
            "{\"code\":\"audio_note_segment_not_found\",\"retryable\":false}".to_owned()
        }
    }

    pub fn clear_region_audio_note_segments_diagnostic_json(&self, tid: u32, rid: u32) -> String {
        if self.clear_region_audio_note_segments(tid, rid) {
            format!(
                "{{\"ok\":true,\"operation\":\"clear_region_audio_note_segments\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_segments_not_found\",\"retryable\":false}".to_owned()
        }
    }

    pub fn analyze_region_audio_note_segments(&self, tid: u32, rid: u32, sample_rate: f64) -> bool {
        if !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.analyze_region_audio_note_segments(tid, rid, sample_rate))
    }

    pub fn analyze_region_audio_note_segments_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        sample_rate: f64,
    ) -> String {
        if self.analyze_region_audio_note_segments(tid, rid, sample_rate) {
            format!(
                "{{\"ok\":true,\"operation\":\"analyze_region_audio_note_segments\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_analysis_failed\",\"retryable\":true}".to_owned()
        }
    }

    pub fn set_region_loop_count(&self, tid: u32, rid: u32, count: u32) -> bool {
        if !(1..=1024).contains(&count) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_loop_count(tid, rid, count))
    }
}
