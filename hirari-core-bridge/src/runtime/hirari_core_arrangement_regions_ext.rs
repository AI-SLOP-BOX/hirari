//! Region mutation entry points (trim/warp/audio note segments/analysis),
//! split out of hirari_core_arrangement_regions.rs.
use super::*;

impl HirariCore {
    pub fn set_region_trim(&self, tid: u32, rid: u32, start_norm: f32, end_norm: f32) -> bool {
        if !start_norm.is_finite()
            || !end_norm.is_finite()
            || !(0.0..=1.0).contains(&start_norm)
            || !(0.0..=1.0).contains(&end_norm)
            || start_norm >= end_norm
        {
            return false;
        }
        self.engine_opt()
            .is_some_and(|e| e.set_region_trim(tid, rid, start_norm, end_norm))
    }

    pub fn set_region_warp_ratio(&self, tid: u32, rid: u32, ratio: f64) -> bool {
        if !ratio.is_finite() || !(0.5..=2.0).contains(&ratio) {
            return false;
        }
        self.engine_opt()
            .is_some_and(|e| e.set_region_warp_ratio(tid, rid, ratio))
    }
    pub fn set_region_pitch_preserve_warp(&self, tid: u32, rid: u32, enabled: bool) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine_opt()
            .is_some_and(|engine| engine.set_region_pitch_preserve_warp(tid, rid, enabled))
    }

    pub fn set_region_pitch_semitones(&self, tid: u32, rid: u32, semitones: f32) -> bool {
        if !semitones.is_finite() || !(-48.0..=48.0).contains(&semitones) {
            return false;
        }
        self.engine_opt()
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
            || pitch_offset_cents.abs() > crate::dsp_limits::MAX_PITCH_CENTS
            || formant_offset_cents.abs() > crate::dsp_limits::MAX_FORMANT_CENTS
        {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
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
            && pitch_offset_cents.abs() <= crate::dsp_limits::MAX_PITCH_CENTS
            && formant_offset_cents.is_finite()
            && formant_offset_cents.abs() <= crate::dsp_limits::MAX_FORMANT_CENTS;
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
            || pitch_cents.abs() > crate::dsp_limits::MAX_PITCH_CENTS
            || !formant_cents.is_finite()
            || formant_cents.abs() > crate::dsp_limits::MAX_FORMANT_CENTS
        {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
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

    pub fn set_region_audio_note_formant_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        formant_cents: f64,
    ) -> bool {
        if !segment_start_seconds.is_finite() || segment_start_seconds < 0.0
            || !position_seconds.is_finite() || position_seconds < 0.0
            || !formant_cents.is_finite() || formant_cents.abs() > crate::dsp_limits::MAX_FORMANT_CENTS
        {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
            engine.set_region_audio_note_formant_anchor(
                tid, rid, segment_start_seconds, position_seconds, formant_cents)
        })
    }

    pub fn set_region_audio_note_pitch_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        pitch_cents: f64,
    ) -> bool {
        if !segment_start_seconds.is_finite() || segment_start_seconds < 0.0
            || !position_seconds.is_finite() || position_seconds < 0.0
            || !pitch_cents.is_finite() || pitch_cents.abs() > crate::dsp_limits::MAX_PITCH_CENTS
        {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
            engine.set_region_audio_note_pitch_anchor(
                tid, rid, segment_start_seconds, position_seconds, pitch_cents)
        })
    }

    pub fn move_region_audio_note_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        old_position_seconds: f64,
        new_position_seconds: f64,
        value_cents: f64,
        edit_formant: bool,
    ) -> bool {
        let limit = if edit_formant {
            crate::dsp_limits::MAX_FORMANT_CENTS
        } else {
            crate::dsp_limits::MAX_PITCH_CENTS
        };
        if !segment_start_seconds.is_finite() || segment_start_seconds < 0.0
            || !old_position_seconds.is_finite() || old_position_seconds < 0.0
            || !new_position_seconds.is_finite() || new_position_seconds < 0.0
            || !value_cents.is_finite() || value_cents.abs() > limit
        {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
            engine.move_region_audio_note_anchor(
                tid, rid, segment_start_seconds, old_position_seconds,
                new_position_seconds, value_cents, edit_formant)
        })
    }

    pub fn clear_region_audio_note_segments(&self, tid: u32, rid: u32) -> bool {
        self.engine_opt()
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
        self.engine_opt().is_some_and(|engine| {
            engine.warp_region_audio_note_segment(tid, rid, segment_start, new_start, new_end)
        })
    }

    pub fn remove_region_audio_note_segment(&self, tid: u32, rid: u32, segment_start: f64) -> bool {
        if !segment_start.is_finite() || segment_start < 0.0 {
            return false;
        }
        self.engine_opt()
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
        if !self.start_region_audio_note_analysis(tid, rid, sample_rate) {
            return false;
        }
        for _ in 0..5000 {
            match self.region_audio_note_analysis_status(tid, rid) {
                2 => return self.finalize_region_audio_note_analysis(tid, rid),
                3 => return true,
                4 => return false,
                _ => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
        false
    }

    /// Starts pitch detection on a bounded background worker. The caller must
    /// poll status and finalize the completed result on the control/UI thread
    /// so its mutation is recorded in Undo/Redo safely.
    pub fn start_region_audio_note_analysis(&self, tid: u32, rid: u32, sample_rate: f64) -> bool {
        if tid == 0
            || rid == 0
            || !sample_rate.is_finite()
            || !crate::dsp_limits::is_supported_sample_rate(sample_rate)
        {
            return false;
        }
        self.engine_opt()
            .is_some_and(|engine| engine.start_region_audio_note_analysis(tid, rid, sample_rate))
    }

    pub fn region_audio_note_analysis_status(&self, tid: u32, rid: u32) -> u8 {
        self.engine_opt().map_or(0, |engine| {
            engine.region_audio_note_analysis_status(tid, rid)
        })
    }

    pub fn finalize_region_audio_note_analysis(&self, tid: u32, rid: u32) -> bool {
        self.engine_opt()
            .is_some_and(|engine| engine.finalize_region_audio_note_analysis(tid, rid))
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
        self.engine_opt()
            .is_some_and(|e| e.set_region_loop_count(tid, rid, count))
    }
}
