//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Republishes lanes, markers, ruler and timeline end from the engine.
    /// Shared by the poll cadence and by clip-move commits, which refresh
    /// immediately instead of waiting for the next layout tick.
    pub(in crate::ameui) fn refresh_arrange_state(&self, core: &HirariCore) {
        // The neutral arrange model is the one layout parser for both
        // surfaces, so this shell and the arranger reduce the
        // same native layout to the same lanes, ruler and timeline.
        let tracks = parse_arrange_tracks(&core.get_project_layout_json(), |samples| {
            core.samples_to_beats(samples)
        });
        let raw_time_signatures = core.get_time_signature_events();
        let timeline_end = arrangement_timeline_end(
            last_clip_end(&tracks),
            &valid_time_signatures(&raw_time_signatures),
        );
        self.markers.set(parse_markers(&core.markers_json()));
        self.step_seq_patterns
            .set(serde_json::from_str(&core.step_sequencer_patterns_json()).unwrap_or_default());
        self.measure_starts.set(arrangement_measure_starts(
            &raw_time_signatures,
            timeline_end,
        ));
        self.timeline_end_beats.set(timeline_end);
        let notes = parse_midi_notes(&core.midi_notes_json(), |samples| {
            core.samples_to_beats(samples)
        });
        if self.piano_track.get() == 0 {
            let default = notes
                .first()
                .map(|note| note.track_id)
                .unwrap_or_else(|| tracks.first().map(|track| track.id).unwrap_or(0));
            self.piano_track.set(default);
        }
        self.piano_notes.set(notes);
        self.tracks.set(tracks);
        self.dirty_layout
            .set(self.dirty_layout.get().wrapping_add(1));
    }
}
