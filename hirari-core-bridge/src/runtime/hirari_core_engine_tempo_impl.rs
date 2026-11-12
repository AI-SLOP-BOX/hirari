//! Tempo/time-signature edit dispatch implementations, split out of hirari_core_engine_tempo.rs.
use super::*;

impl HirariCore {
    /// The engine's Rust-owned tempo map, undo history and diagnostic ring,
    /// plus the sample rate every beat/sample conversion below uses.
    ///
    /// A zero tempo address is exactly the deleted `!m_engine` branch of the
    /// AudioEngine shims: it means the engine handle was never created, so an
    /// operation returns what those branches returned -- false for mutators,
    /// an empty vector for the snapshot getters, no work for the clears.
    #[inline]
    fn tempo_edit(&self) -> Option<EditHandles> {
        if self.engine_none() || self.tempo_state == 0 {
            return None;
        }
        Some(EditHandles {
            tempo: self.tempo_state as *mut _,
            undo: self.undo_state as *mut _,
            log: self.log_state as *const _,
            sample_rate: self
                .engine_opt()
                .map_or(0.0, |engine| engine.get_sample_rate()),
        })
    }

    /// Bumps the revision the UI polls to learn that the project changed,
    /// exactly where the deleted shims did: once, after an edit that
    /// succeeded -- and for the two clears, whenever the engine existed at
    /// all, because their C++ bodies could refuse a value without saying so
    /// and the shim marked the project anyway.
    #[inline]
    fn mark_project_changed(&self) {
        if let Some(engine) = self.engine_opt() {
            engine.mark_project_state_changed();
        }
    }

    /// `(beat, bpm, ramp)` triples over every tempo event, as the engine
    /// packed them.
    pub(crate) fn tempo_read_events(&self) -> Vec<f64> {
        let Some(h) = self.tempo_edit() else {
            return Vec::new();
        };
        let events = read_events(h.tempo);
        let mut packed = Vec::with_capacity(events.len() * 3);
        for event in events {
            packed.push(samples_to_beats(h, event.sample_pos));
            packed.push(event.bpm);
            packed.push(if event.ramp { 1.0 } else { 0.0 });
        }
        packed
    }

    /// `(beat, numerator, denominator)` triples over every time signature.
    pub(crate) fn tempo_read_signatures(&self) -> Vec<f64> {
        let Some(h) = self.tempo_edit() else {
            return Vec::new();
        };
        let events = read_signatures(h.tempo);
        let mut packed = Vec::with_capacity(events.len() * 3);
        for event in events {
            packed.push(samples_to_beats(h, event.sample_pos));
            packed.push(f64::from(event.numerator));
            packed.push(f64::from(event.denominator));
        }
        packed
    }

    /// Current transport BPM, straight from the map the audio thread reads.
    ///
    /// Two defaults have to survive the move: no engine object at all answered
    /// `0.0` (the Core wrapper), and an engine object without an engine behind
    /// it answered `120.0f` (the shim).
    pub(crate) fn tempo_read(&self) -> f32 {
        match self.tempo_edit() {
            Some(h) => unsafe { hirari_tempo_map_get_current_bpm(h.tempo) },
            None if !self.engine_none() => 120.0,
            None => 0.0,
        }
    }

    /// Drops every time signature, leaving the map's default in place.
    pub(crate) fn tempo_clear_signatures(&self) {
        let Some(h) = self.tempo_edit() else {
            return;
        };
        unsafe {
            hirari_tempo_map_clear_time_signatures(h.tempo);
        }
        self.mark_project_changed();
    }

    /// Resets the map to a single event at `initial_bpm`. A BPM outside
    /// 20..=300 leaves the map untouched -- the C++ body said nothing about
    /// it -- and the project is still marked changed, as the shim did.
    pub(crate) fn tempo_clear_events(&self, initial_bpm: f64) {
        let Some(h) = self.tempo_edit() else {
            return;
        };
        if initial_bpm.is_finite() && (20.0..=300.0).contains(&initial_bpm) {
            unsafe {
                hirari_tempo_map_clear(h.tempo, h.sample_rate, initial_bpm);
                hirari_tempo_map_set_current_bpm(h.tempo, initial_bpm as f32);
            }
        }
        self.mark_project_changed();
    }

    /// Writes a time signature at `beat` and records the pair as one entry.
    pub(crate) fn tempo_write_signature(
        &self,
        beat: f64,
        numerator: u8,
        denominator: u8,
    ) -> bool {
        if !beat.is_finite()
            || beat < 0.0
            || numerator == 0
            || numerator > 32
            || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32)
        {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        if !h.sample_rate.is_finite() || h.sample_rate <= 0.0 {
            return false;
        }
        let before = signature_at(h.tempo, beat);
        unsafe {
            hirari_tempo_map_add_time_signature(h.tempo, beat, numerator, denominator, h.sample_rate);
        }
        let target = SignatureEvent {
            sample_pos: 0,
            numerator,
            denominator,
            beat,
        };
        record_applied_action(
            h,
            "Time Signature".to_owned(),
            move || apply_signature(h, beat, before),
            move || apply_signature(h, beat, Some(target)),
            false,
        );
        self.mark_project_changed();
        true
    }

    /// Removes the signature at `beat`, with the previous value kept for Undo.
    pub(crate) fn tempo_erase_signature(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        if !h.sample_rate.is_finite() || h.sample_rate <= 0.0 {
            return false;
        }
        let Some(before) = signature_at(h.tempo, beat) else {
            return false;
        };
        perform_action(
            h,
            "Remove Time Signature".to_owned(),
            move || apply_signature(h, beat, Some(before)),
            move || apply_signature(h, beat, None),
        );
        self.mark_project_changed();
        true
    }

    /// Writes a tempo event at `beat`, performing the edit (not merely
    /// recording it) under the position in its history label, as the engine
    /// did with `"Tempo Event " + to_string(sample)`.
    pub(crate) fn tempo_write_event(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() || bpm < 20.0 || bpm > 300.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let sample = beats_to_samples(h, beat);
        let before = event_at(h.tempo, sample);
        let after = TempoEvent {
            sample_pos: sample,
            bpm,
            ramp,
            world_beats: 0.0,
        };
        perform_action(
            h,
            format!("Tempo Event {sample}"),
            move || apply_tempo_event(h, sample, before),
            move || apply_tempo_event(h, sample, Some(after)),
        );
        self.mark_project_changed();
        true
    }

    /// Removes the tempo event at `beat`; beat zero's event may be removed too,
    /// which is why the undo side has to remember whether one was there.
    pub(crate) fn tempo_erase_event(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let sample = beats_to_samples(h, beat);
        let Some(before) = event_at(h.tempo, sample) else {
            return false;
        };
        perform_action(
            h,
            "Remove Tempo Event".to_owned(),
            move || apply_tempo_event(h, sample, Some(before)),
            move || apply_tempo_event(h, sample, None),
        );
        self.mark_project_changed();
        true
    }

    /// Moves an event between two beat positions, refusing positions that are
    /// the same event, the map's first sample, or already occupied.
    pub(crate) fn tempo_move_event(&self, from_beat: f64, to_beat: f64) -> bool {
        if !from_beat.is_finite()
            || !to_beat.is_finite()
            || from_beat <= 0.0
            || to_beat <= 0.0
            || (from_beat - to_beat).abs() < 1.0e-6
        {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let from = beats_to_samples(h, from_beat);
        let to = beats_to_samples(h, to_beat);
        if from == 0 || to == 0 || from == to {
            return false;
        }
        if event_at(h.tempo, from).is_none() || event_at(h.tempo, to).is_some() {
            return false;
        }
        perform_action(
            h,
            "Move Tempo Event".to_owned(),
            move || apply_tempo_move(h, from, to, false),
            move || apply_tempo_move(h, from, to, true),
        );
        self.mark_project_changed();
        true
    }

    /// Sets the ramp target at `beat`: the event at that beat changes BPM and
    /// the event that leads into it takes the ramp, which is why a position
    /// with nothing behind it is refused outright.
    pub(crate) fn tempo_write_transition(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() || bpm < 20.0 || bpm > 300.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let before = read_events(h.tempo);
        if before.is_empty() {
            return false;
        }

        let mut after = before.clone();
        let found = after
            .iter()
            .position(|event| (event.world_beats - beat).abs() <= 0.001);
        let target_beat = found.map_or(beat, |index| after[index].world_beats);
        let index = match found {
            Some(index) => {
                after[index].bpm = bpm;
                index
            }
            None => {
                after.push(TempoEvent {
                    sample_pos: 0,
                    bpm,
                    ramp: false,
                    world_beats: target_beat,
                });
                after.len() - 1
            }
        };

        if target_beat <= 0.001 {
            after[index].world_beats = 0.0;
            after[index].bpm = bpm;
            after[index].ramp = ramp;
        } else {
            let mut previous: Option<usize> = None;
            for (position, event) in after.iter().enumerate() {
                if event.world_beats < target_beat - 0.001
                    && previous.is_none_or(|held| event.world_beats > after[held].world_beats)
                {
                    previous = Some(position);
                }
            }
            let Some(previous) = previous else {
                return false;
            };
            after[previous].ramp = ramp;
        }

        // `partial_cmp(..., Equal)` orders exactly as the C++ `<` comparator
        // did, including treating 0.0 and -0.0 as equal, so equal events stay
        // in the order they were already in.
        after.sort_by(|left, right| {
            left.world_beats
                .partial_cmp(&right.world_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if !replace_events(h, &after) {
            return false;
        }
        unsafe { hirari_tempo_map_set_current_bpm(h.tempo, after[0].bpm as f32) };
        record_applied_action(
            h,
            "Set Tempo Transition".to_owned(),
            move || apply_tempo_transition(h, &before),
            move || apply_tempo_transition(h, &after),
            false,
        );
        self.mark_project_changed();
        true
    }

    /// The ordinary tempo control: clamps into 20..=300 and writes one
    /// history entry, see [`set_tempo_at`].
    pub(crate) fn tempo_write(&self, bpm: f32) -> bool {
        if !bpm.is_finite() {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let playhead = self.engine_opt().map_or(0, |engine| engine.get_playhead());
        if set_tempo_at(h, playhead, bpm) {
            self.mark_project_changed();
            true
        } else {
            false
        }
    }
}

/// C++ entry point for `HirariUnifiedEngine::set_tempo`, kept only because
/// `tests/timeline_bpm_ghost_contract` drives that method straight off the
/// engine singleton. The body is [`set_tempo_at`] either way, so there is one
/// implementation of the single-entry invariant, not two.
#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_edit_set_tempo(
    tempo: *mut std::ffi::c_void,
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    playhead: u64,
    sample_rate: f64,
    bpm: f32,
) -> bool {
    if tempo.is_null() {
        return false;
    }
    set_tempo_at(
        EditHandles {
            tempo,
            undo,
            log,
            sample_rate,
        },
        playhead,
        bpm,
    )
}
