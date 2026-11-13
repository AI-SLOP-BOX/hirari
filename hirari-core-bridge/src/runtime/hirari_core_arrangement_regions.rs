#[derive(Clone, Copy)]
struct ArrangementRegionState {
    track_id: u32,
    region_id: u32,
    start_sample: u64,
    sync_group: u32,
}

fn arrangement_region_states(core: &HirariCore) -> Option<Vec<ArrangementRegionState>> {
    let layout: serde_json::Value = serde_json::from_str(&core.get_project_layout_json()).ok()?;
    let tracks = layout.as_array()?;
    let mut states = Vec::new();
    for track in tracks {
        let Some(track_id) = track
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
        else {
            continue;
        };
        let Some(regions) = track.get("regions").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for region in regions {
            let Some(region_id) = region
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            else {
                continue;
            };
            let Some(start_sample) = region.get("start").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let sync_group = region
                .get("sync_group")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(0);
            states.push(ArrangementRegionState {
                track_id,
                region_id,
                start_sample,
                sync_group,
            });
        }
    }
    Some(states)
}

fn shifted_sample(sample: u64, delta: i128) -> Option<u64> {
    if delta >= 0 {
        sample.checked_add(u64::try_from(delta).ok()?)
    } else {
        sample.checked_sub(u64::try_from(-delta).ok()?)
    }
}

impl HirariCore {
    pub(crate) fn move_region_to_sample(&self, tid: u32, rid: u32, start_sample: u64) -> bool {
        tid != 0
            && rid != 0
            && self
                .engine_opt()
                .is_some_and(|engine| engine.move_region(tid, rid, start_sample as f64))
    }

    pub(crate) fn split_region_at_sample_with_right_id(
        &self,
        tid: u32,
        rid: u32,
        split_sample: u64,
    ) -> u32 {
        if tid == 0 || rid == 0 {
            return 0;
        }
        self.engine_opt().map_or(0, |engine| {
            engine.split_region_with_right_id(tid, rid, split_sample as f64)
        })
    }

    pub fn move_region(&self, tid: u32, rid: u32, start: f64) -> bool {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return false;
        }
        let Some(engine) = self.engine_opt() else {
            return false;
        };
        let Some(states) = arrangement_region_states(self) else {
            return false;
        };
        let Some(region) = states
            .iter()
            .find(|region| region.track_id == tid && region.region_id == rid)
        else {
            return false;
        };
        let start_sample = self.beats_to_samples(start);
        let delta = i128::from(start_sample) - i128::from(region.start_sample);
        let mut notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes.clone(),
            Err(_) => return false,
        };
        let mut changed_notes = false;
        for note in &mut notes {
            if note.track_id == tid && note.region_id == rid {
                let Some(next_start) = shifted_sample(note.start_sample, delta) else {
                    return false;
                };
                note.start_sample = next_start;
                if note.validate().is_err() {
                    return false;
                }
                changed_notes = true;
            }
        }
        let owns_transaction = !engine.is_undo_transaction_active();
        if owns_transaction {
            self.begin_undo_transaction("Move Region");
        }
        if !engine.move_region(tid, rid, start_sample as f64) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        if changed_notes && !self.replace_midi_note_contracts(notes, true) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        !owns_transaction || self.end_undo_transaction()
    }
    pub fn move_region_sync_group(&self, tid: u32, rid: u32, start: f64) -> bool {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return false;
        }
        let Some(engine) = self.engine_opt() else {
            return false;
        };
        let Some(states) = arrangement_region_states(self) else {
            return false;
        };
        let Some(anchor) = states
            .iter()
            .find(|region| region.track_id == tid && region.region_id == rid)
        else {
            return false;
        };
        if anchor.sync_group == 0 {
            return false;
        }
        let group_regions = states
            .iter()
            .filter(|region| region.sync_group == anchor.sync_group)
            .map(|region| (region.track_id, region.region_id))
            .collect::<std::collections::HashSet<_>>();
        let start_sample = self.beats_to_samples(start);
        let delta = i128::from(start_sample) - i128::from(anchor.start_sample);
        let mut notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes.clone(),
            Err(_) => return false,
        };
        let mut changed_notes = false;
        for note in &mut notes {
            if group_regions.contains(&(note.track_id, note.region_id)) {
                let Some(next_start) = shifted_sample(note.start_sample, delta) else {
                    return false;
                };
                note.start_sample = next_start;
                if note.validate().is_err() {
                    return false;
                }
                changed_notes = true;
            }
        }
        let owns_transaction = !engine.is_undo_transaction_active();
        if owns_transaction {
            self.begin_undo_transaction("Move Region Sync Group");
        }
        if !engine.move_region_sync_group(tid, rid, start_sample as f64) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        if changed_notes && !self.replace_midi_note_contracts(notes, true) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        !owns_transaction || self.end_undo_transaction()
    }
    pub fn split_region(&self, tid: u32, rid: u32, beat: f64) -> bool {
        if tid == 0 || rid == 0 || !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let split_sample = self.beats_to_samples(beat.max(0.0));
        let Some(engine) = self.engine_opt() else {
            return false;
        };
        let owns_transaction = !engine.is_undo_transaction_active();
        if owns_transaction {
            self.begin_undo_transaction("Split Region");
        }
        let right_region_id = engine.split_region_with_right_id(tid, rid, split_sample as f64);
        if right_region_id == 0 {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }

        // Keep MIDI authoring ownership aligned with the new arrangement
        // regions. Notes beginning at or after the cut belong to the right
        // clip; notes that began before it retain their identity and timing.
        let mut notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes.clone(),
            Err(_) => {
                if owns_transaction {
                    let _ = self.abort_undo_transaction();
                }
                return false;
            }
        };
        let mut changed = false;
        for note in &mut notes {
            if note.track_id == tid && note.region_id == rid && note.start_sample >= split_sample {
                note.region_id = right_region_id;
                changed = true;
            }
        }
        if changed && !self.replace_midi_note_contracts(notes, true) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        if owns_transaction && !self.end_undo_transaction() {
            return false;
        }
        true
    }
    pub fn duplicate_region(&self, tid: u32, rid: u32, start: f64) -> u32 {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return 0;
        }
        let Some(engine) = self.engine_opt() else {
            return 0;
        };
        let Some(states) = arrangement_region_states(self) else {
            return 0;
        };
        let Some(source) = states
            .iter()
            .find(|region| region.track_id == tid && region.region_id == rid)
        else {
            return 0;
        };
        let start_sample = self.beats_to_samples(start);
        let delta = i128::from(start_sample) - i128::from(source.start_sample);
        let source_notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes
                .iter()
                .filter(|note| note.track_id == tid && note.region_id == rid)
                .cloned()
                .collect::<Vec<_>>(),
            Err(_) => return 0,
        };
        let has_source_notes = !source_notes.is_empty();
        let owns_transaction = !engine.is_undo_transaction_active();
        if owns_transaction {
            self.begin_undo_transaction("Duplicate Region");
        }
        let duplicate_id = engine.duplicate_region(tid, rid, start_sample);
        if duplicate_id == 0 {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return 0;
        }
        let mut notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes.clone(),
            Err(_) => {
                if owns_transaction {
                    let _ = self.abort_undo_transaction();
                }
                return 0;
            }
        };
        for mut note in source_notes {
            let Some(next_start) = shifted_sample(note.start_sample, delta) else {
                if owns_transaction {
                    let _ = self.abort_undo_transaction();
                }
                return 0;
            };
            note.region_id = duplicate_id;
            note.start_sample = next_start;
            if note.validate().is_err() {
                if owns_transaction {
                    let _ = self.abort_undo_transaction();
                }
                return 0;
            }
            notes.push(note);
        }
        if notes.len() > 100_000
            || (has_source_notes && !self.replace_midi_note_contracts(notes, true))
        {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return 0;
        }
        if owns_transaction && !self.end_undo_transaction() {
            return 0;
        }
        duplicate_id
    }
    pub fn remove_region(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        let Some(engine) = self.engine_opt() else {
            return false;
        };
        let mut notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes.clone(),
            Err(_) => return false,
        };
        let before_len = notes.len();
        notes.retain(|note| note.track_id != tid || note.region_id != rid);
        let changed_notes = notes.len() != before_len;
        let owns_transaction = !engine.is_undo_transaction_active();
        if owns_transaction {
            self.begin_undo_transaction("Remove Region");
        }
        if !engine.remove_region(tid, rid) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        if changed_notes && !self.replace_midi_note_contracts(notes, true) {
            if owns_transaction {
                let _ = self.abort_undo_transaction();
            }
            return false;
        }
        !owns_transaction || self.end_undo_transaction()
    }
    pub fn set_region_muted(&self, tid: u32, rid: u32, muted: bool) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine_opt()
            .is_some_and(|engine| engine.set_region_muted(tid, rid, muted))
    }
    pub fn set_region_comp_ranges(
        &self,
        tid: u32,
        rid: u32,
        ranges: &[(u64, u64, u64, u64)],
        managed: bool,
    ) -> bool {
        // Admissibility is owned by the Rust core (hirari_region_comp_ranges_validate).
        // This layer used to re-implement part of it and silently skipped the
        // ordering and region-bounds checks, so an out-of-order batch could be
        // accepted here and rejected one layer down.
        if tid == 0 || rid == 0 || ranges.len() > 4096 {
            return false;
        }
        let mut packed = Vec::with_capacity(ranges.len().saturating_mul(4));
        for (start, end, fade_in, fade_out) in ranges {
            packed.extend_from_slice(&[*start, *end, *fade_in, *fade_out]);
        }
        self.engine_opt().is_some_and(|engine| {
            engine.set_region_comp_ranges(tid, rid, packed.as_slice(), managed)
        })
    }
    pub fn append_region_processing(
        &self,
        tid: u32,
        rid: u32,
        step_id: u32,
        operation: &str,
        parameter: f32,
        enabled: bool,
    ) -> bool {
        // Trivial, region-independent rejections only. Everything else
        // (operation size, parameter finiteness, the 4096-step cap) is
        // adjudicated by the Rust store the region owns.
        if tid == 0 || rid == 0 || step_id == 0 || operation.is_empty() {
            return false;
        }
        self.engine_opt()
            .is_some_and(|engine| {
                engine.append_region_processing(tid, rid, step_id, operation, parameter, enabled)
            })
    }
    pub fn clear_region_processing(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine_opt()
            .is_some_and(|engine| engine.clear_region_processing(tid, rid))
    }
    pub fn set_region_gain(&self, tid: u32, rid: u32, gain: f32) -> bool {
        if !gain.is_finite() || !(-24.0..=24.0).contains(&gain) {
            return false;
        }
        self.engine_opt()
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
        if tid == 0 || rid == 0 || start >= end || !gain.is_finite() || !(0.0..=4.0).contains(&gain)
        {
            return false;
        }
        self.engine_opt().is_some_and(|e| {
            e.set_region_range_edit(tid, rid, start, end, gain, fade_in as u64, fade_out as u64)
        })
    }
    pub fn clear_region_range_edits(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine_opt()
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
        self.engine_opt()
            .is_some_and(|e| e.set_region_fades(tid, rid, fade_in, fade_out))
    }
    pub fn set_region_reverse(&self, tid: u32, rid: u32, reverse: bool) -> bool {
        self.engine_opt()
            .is_some_and(|e| e.set_region_reverse(tid, rid, reverse))
    }

    pub fn set_region_sync_group(&self, tid: u32, rid: u32, group: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine_opt()
            .is_some_and(|e| e.set_region_sync_group(tid, rid, group))
    }

    /// Apply one transient map to the selected region and every audio member
    /// of its sync group. Marker creation and application stay non-destructive
    /// and are recorded as one native undo transaction.
    pub fn quantize_region_sync_group(
        &self,
        tid: u32,
        rid: u32,
        strength: f32,
        grid_index: u8,
        swing: f32,
    ) -> bool {
        if tid == 0
            || rid == 0
            || !strength.is_finite()
            || !(0.0..=1.0).contains(&strength)
            || !swing.is_finite()
            || !(-1.0..=1.0).contains(&swing)
        {
            return false;
        }
        const GRID_BEATS: [f64; 13] = [
            1.0,
            0.5,
            0.25,
            0.125,
            0.0625,
            2.0 / 3.0,
            1.0 / 3.0,
            1.0 / 6.0,
            1.0 / 12.0,
            1.5,
            0.75,
            0.375,
            0.1875,
        ];
        let Some(grid_beats) = GRID_BEATS.get(grid_index as usize).copied() else {
            return false;
        };
        self.engine_opt().is_some_and(|engine| {
            engine.quantize_region_sync_group(tid, rid, strength, grid_beats, swing)
        })
    }

    pub fn region_quantize_status(&self, tid: u32, rid: u32) -> u8 {
        self.engine_opt()
            .map_or(0, |engine| engine.region_quantize_status(tid, rid))
    }

    pub fn finalize_region_quantize(&self, tid: u32, rid: u32) -> bool {
        tid != 0
            && rid != 0
            && self
                .engine_opt()
                .is_some_and(|engine| engine.finalize_region_quantize(tid, rid))
    }

    /// Analyze one or more selected takes against a reference take and queue
    /// non-destructive source-to-timeline warps. All regions must have the same
    /// unwarped sample span; final application is a separate UI-thread step.
    pub fn align_regions_to_reference(
        &self,
        reference_track: u32,
        reference_region: u32,
        targets: &[(u32, u32)],
    ) -> bool {
        if reference_track == 0 || reference_region == 0 || targets.is_empty() || targets.len() > 32
        {
            return false;
        }
        let mut target_tids = Vec::with_capacity(targets.len());
        let mut target_rids = Vec::with_capacity(targets.len());
        for &(track, region) in targets {
            if track == 0 || region == 0 || (track == reference_track && region == reference_region)
            {
                return false;
            }
            target_tids.push(track);
            target_rids.push(region);
        }
        let mut sorted = targets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != targets.len() {
            return false;
        }
        self.engine_opt().is_some_and(|engine| {
            engine.align_regions_to_reference(
                reference_track,
                reference_region,
                &target_tids,
                &target_rids,
            )
        })
    }

    pub fn region_alignment_status(&self, reference_track: u32, reference_region: u32) -> u8 {
        if reference_track == 0 || reference_region == 0 {
            return 0;
        }
        self.engine_opt().map_or(0, |engine| {
            engine.region_alignment_status(reference_track, reference_region)
        })
    }

    pub fn finalize_region_alignment(&self, reference_track: u32, reference_region: u32) -> bool {
        reference_track != 0
            && reference_region != 0
            && self.engine_opt().is_some_and(|engine| {
                engine.finalize_region_alignment(reference_track, reference_region)
            })
    }
}

#[path = "hirari_core_arrangement_regions_ext.rs"]
mod regions_ext;
