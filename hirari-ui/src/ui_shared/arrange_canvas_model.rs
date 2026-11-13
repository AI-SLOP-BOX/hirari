//! Arrange-model canvas hit-testing, trim-edge selection, piano roll
//! geometry and note lookup tables; split out of arrange_model.rs.
use super::*;

/// timeline into the slot (the FIT zoom); higher zoom shows a
/// window of it scrolled by `scroll_beats`. The draw step stays a pure
/// function of this struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrangeCanvasGeometry {
    /// Horizontal scale: canvas pixels per beat.
    pub px_per_beat: f32,
    /// Height of the measure/marker ruler band at the top.
    pub ruler_height: f32,
    /// Height of one track lane below the ruler.
    pub lane_height: f32,
    /// Beat step for the faint grid lines (`visible-beat-step`).
    pub beat_step: f32,
    /// Left edge of the visible window, in beats (clamped).
    pub scroll_beats: f32,
    /// Width of the lane-header column at the left (104px).
    pub header_width: f32,
}

impl ArrangeCanvasGeometry {
    /// Canvas x of a beat, in canvas-local pixels. Beats outside the
    /// visible window map off-canvas (negative or past the width) and are
    /// clipped by the renderer; they are not clamped to the edges.
    pub fn x_for_beat(&self, beat: f32) -> f32 {
        self.header_width + (beat - self.scroll_beats) * self.px_per_beat
    }

    /// Beat at a canvas x (click-to-seek inverse of [`Self::x_for_beat`]).
    /// A zero scale (never drawn / collapsed canvas) seeks to beat 0.
    pub fn beat_for_x(&self, x: f32) -> f32 {
        if self.px_per_beat > 0.0 && x.is_finite() {
            ((x - self.header_width) / self.px_per_beat + self.scroll_beats).max(0.0)
        } else {
            0.0
        }
    }

    /// Top y of a lane, in canvas-local pixels.
    pub fn y_for_lane(&self, lane: usize) -> f32 {
        self.ruler_height + lane as f32 * self.lane_height
    }
}

/// Derives the canvas layout for a `width` x `height` slot showing
/// `timeline_end_beats` beats across `lane_count` lanes at `zoom`, scrolled
/// to `scroll_beats`. Zoom and scroll are clamped to what the canvas can
/// show; degenerate sizes yield a zeroed layout rather than NaNs, so a
/// collapsed window is safe.
pub fn arrange_canvas_geometry(
    width: f32,
    height: f32,
    timeline_end_beats: f32,
    lane_count: usize,
    zoom: f32,
    scroll_beats: f32,
) -> ArrangeCanvasGeometry {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let height = if height.is_finite() {
        height.max(0.0)
    } else {
        0.0
    };
    let timeline_end = if timeline_end_beats.is_finite() {
        timeline_end_beats.max(1.0)
    } else {
        1.0
    };
    let zoom = if zoom.is_finite() {
        zoom.clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX)
    } else {
        1.0
    };
    // The timeline area starts after the lane-header column.
    let header_width = 104.0_f32.min(width);
    let timeline_width = (width - header_width).max(0.0);
    let px_per_beat = timeline_width / timeline_end * zoom;
    let visible_beats = if px_per_beat > 0.0 {
        timeline_width / px_per_beat
    } else {
        timeline_end
    };
    let max_scroll = (timeline_end - visible_beats).max(0.0);
    let scroll_beats = if scroll_beats.is_finite() {
        scroll_beats.clamp(0.0, max_scroll)
    } else {
        0.0
    };
    let ruler_height = 24.0_f32.min(height);
    let lane_height = if lane_count == 0 {
        0.0
    } else {
        (height - ruler_height).max(0.0) / lane_count as f32
    };
    // Same adaptive step as the arrange dock: at least one beat, and never so
    // many grid lines that they merge into a solid band.
    let beat_step = (4.0 / px_per_beat.max(f32::EPSILON))
        .max(timeline_end / 2048.0)
        .max(1.0)
        .ceil();
    ArrangeCanvasGeometry {
        px_per_beat,
        ruler_height,
        lane_height,
        beat_step,
        scroll_beats,
        header_width,
    }
}

/// The clip under a canvas point, as `(track_id, clip_id)`. Returns `None`
/// in the ruler band, below the last lane, or on empty lane space, so a
/// click there keeps its seek meaning. This is the hit-test half of clip
/// selection; the drag/trim mutations still belong to a later slice.
pub fn clip_at(
    tracks: &[ArrangeTrack],
    geometry: &ArrangeCanvasGeometry,
    x: f32,
    y: f32,
) -> Option<(u32, i32)> {
    if geometry.lane_height <= 0.0 || y < geometry.ruler_height {
        return None;
    }
    let lane = ((y - geometry.ruler_height) / geometry.lane_height) as usize;
    let track = tracks.get(lane)?;
    let beat = geometry.beat_for_x(x);
    track
        .clips
        .iter()
        .find(|clip| beat >= clip.start_beat && beat < clip.start_beat + clip.length_beats)
        .map(|clip| (track.id, clip.id))
}

/// Which clip edge a canvas x is on, for trim drags. The dock puts
/// an 8px handle on each edge; here the zones are 6px and only exist when
/// the clip is wide enough to hold both without overlapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrimEdge {
    Left,
    Right,
}

pub fn trim_edge_at(
    geometry: &ArrangeCanvasGeometry,
    clip: &ArrangeClip,
    x: f32,
) -> Option<TrimEdge> {
    const EDGE_PX: f32 = 6.0;
    let left = geometry.x_for_beat(clip.start_beat);
    let width = clip.length_beats * geometry.px_per_beat;
    if width <= EDGE_PX * 2.0 {
        return None;
    }
    if (x - left).abs() <= EDGE_PX {
        Some(TrimEdge::Left)
    } else if (x - (left + width)).abs() <= EDGE_PX {
        Some(TrimEdge::Right)
    } else {
        None
    }
}

/// One audio device from the engine's device catalog, as the settings
/// UI lists it. `id` is the `select_audio_device` handle.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: u32,
    pub name: String,
}

/// Parses `list_audio_devices_json`; malformed input yields no devices.
pub fn parse_audio_devices(catalog_json: &str) -> Vec<AudioDevice> {
    #[derive(Deserialize)]
    struct DeviceLayout {
        id: u32,
        #[serde(default)]
        name: String,
    }
    let Ok(devices) = serde_json::from_str::<Vec<DeviceLayout>>(catalog_json) else {
        return Vec::new();
    };
    devices
        .into_iter()
        .map(|device| AudioDevice {
            id: device.id,
            name: device.name,
        })
        .collect()
}

/// One audio-library asset from the preview-audio catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryAsset {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub sample_rate: f64,
    pub size_bytes: u64,
}

/// Parses `preview_audio_catalog_json`; malformed input yields no assets.
pub fn parse_preview_catalog(catalog_json: &str) -> Vec<LibraryAsset> {
    #[derive(Deserialize)]
    struct AssetLayout {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        name: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        sample_rate: f64,
        #[serde(default)]
        size: u64,
    }
    #[derive(Deserialize)]
    struct CatalogLayout {
        #[serde(default)]
        assets: Vec<AssetLayout>,
    }
    let Ok(catalog) = serde_json::from_str::<CatalogLayout>(catalog_json) else {
        return Vec::new();
    };
    catalog
        .assets
        .into_iter()
        .map(|asset| LibraryAsset {
            id: asset.id,
            name: if asset.name.is_empty() {
                "Audio".to_owned()
            } else {
                asset.name
            },
            path: asset.path,
            sample_rate: asset.sample_rate,
            size_bytes: asset.size,
        })
        .collect()
}

/// One MIDI note with its full authoring contract, so piano-roll edits
/// can commit a lossless `replace_midi_note_contracts` (lyric,
/// articulation, vibrato and curve metadata survive an edit, exactly as
/// the roll preserves them through its model).
#[derive(Debug, Clone, PartialEq)]
pub struct PianoNote {
    pub track_id: u32,
    pub region_id: u32,
    pub pitch: u8,
    pub midi_channel: u8,
    pub articulation: u8,
    pub velocity: u8,
    pub start_beat: f32,
    pub length_beats: f32,
    pub lyric: String,
    pub phoneme: String,
    pub pitch_curve_cents: Vec<i16>,
    pub vibrato_depth_cents: u16,
    pub vibrato_rate_millihz: u16,
    pub portamento_samples: u32,
    pub probability: u8,
    pub repeat_count: u16,
}

/// Parses the engine's canonical `midi_notes_json` into notes, sanitized
/// like the piano roll (`sanitize_notes`): pitches and velocities
/// clamped, lengths floored, sorted by track/start/pitch. Malformed
/// input yields no notes.
pub fn parse_midi_notes(notes_json: &str, samples_to_beats: impl Fn(u64) -> f64) -> Vec<PianoNote> {
    fn default_vibrato_rate() -> u16 {
        5_000
    }
    fn default_probability() -> u8 {
        100
    }
    fn default_repeat_count() -> u16 {
        1
    }
    #[derive(Deserialize)]
    struct NoteLayout {
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        #[serde(default)]
        region_id: u32,
        #[serde(default)]
        midi_channel: u8,
        #[serde(default)]
        articulation: u8,
        #[serde(default)]
        lyric: String,
        #[serde(default)]
        phoneme: String,
        #[serde(default)]
        pitch_curve_cents: Vec<i16>,
        #[serde(default)]
        vibrato_depth_cents: u16,
        #[serde(default = "default_vibrato_rate")]
        vibrato_rate_millihz: u16,
        #[serde(default)]
        portamento_samples: u32,
        #[serde(default = "default_probability")]
        probability: u8,
        #[serde(default = "default_repeat_count")]
        repeat_count: u16,
    }
    let Ok(layouts) = serde_json::from_str::<Vec<NoteLayout>>(notes_json) else {
        return Vec::new();
    };
    let mut notes = layouts
        .into_iter()
        .filter_map(|layout| {
            let start_beat = samples_to_beats(layout.start_sample);
            let end_beat =
                samples_to_beats(layout.start_sample.saturating_add(layout.length_samples));
            if !start_beat.is_finite() || !end_beat.is_finite() {
                return None;
            }
            Some(PianoNote {
                track_id: layout.track_id,
                region_id: layout.region_id,
                pitch: layout.pitch.min(127),
                midi_channel: layout.midi_channel.min(15),
                articulation: layout.articulation.min(127),
                velocity: layout.velocity.clamp(1, 127),
                start_beat: start_beat.max(0.0) as f32,
                length_beats: ((end_beat - start_beat).max(0.015625)) as f32,
                lyric: layout.lyric,
                phoneme: layout.phoneme,
                pitch_curve_cents: layout.pitch_curve_cents,
                vibrato_depth_cents: layout.vibrato_depth_cents,
                vibrato_rate_millihz: layout.vibrato_rate_millihz.clamp(500, 20_000),
                portamento_samples: layout.portamento_samples,
                probability: layout.probability.min(100),
                repeat_count: layout.repeat_count.max(1),
            })
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| {
        a.track_id
            .cmp(&b.track_id)
            .then_with(|| a.start_beat.total_cmp(&b.start_beat))
            .then_with(|| a.pitch.cmp(&b.pitch))
    });
    notes
}

/// Index (into `notes`) of the note under a canvas point, if any.
pub fn note_at(notes: &[PianoNote], geometry: &PianoRollGeometry, x: f32, y: f32) -> Option<usize> {
    let pitch = geometry.pitch_at_y(y);
    let beat = geometry.beat_at_x(x);
    notes.iter().position(|note| {
        note.pitch == pitch && beat >= note.start_beat && beat < note.start_beat + note.length_beats
    })
}

/// Pixel layout of one piano-roll canvas, fit-to-width like the arrange
/// canvas. The pitch window fits the visible notes (expanded by two
/// semitones, clamped to 0..127); an empty roll shows a default octave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PianoRollGeometry {
    pub px_per_beat: f32,
    pub px_per_pitch: f32,
    pub top_pitch: u8,
    pub bottom_pitch: u8,
}

impl PianoRollGeometry {
    pub fn x_for_beat(&self, beat: f32) -> f32 {
        beat.max(0.0) * self.px_per_beat
    }

    pub fn y_for_pitch(&self, pitch: u8) -> f32 {
        (self.top_pitch.saturating_sub(pitch) as f32) * self.px_per_pitch
    }

    /// Beat at a canvas x (inverse of [`Self::x_for_beat`]).
    pub fn beat_at_x(&self, x: f32) -> f32 {
        if self.px_per_beat > 0.0 && x.is_finite() {
            (x / self.px_per_beat).max(0.0)
        } else {
            0.0
        }
    }

    /// Pitch at a canvas y (inverse of [`Self::y_for_pitch`]), clamped to
    /// the visible window.
    pub fn pitch_at_y(&self, y: f32) -> u8 {
        if self.px_per_pitch <= 0.0 || !y.is_finite() {
            return self.top_pitch;
        }
        let row = (y / self.px_per_pitch).floor() as i32;
        (i32::from(self.top_pitch) - row)
            .clamp(i32::from(self.bottom_pitch), i32::from(self.top_pitch)) as u8
    }
}

pub fn piano_roll_geometry(
    width: f32,
    height: f32,
    timeline_end_beats: f32,
    notes: &[PianoNote],
) -> PianoRollGeometry {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let height = if height.is_finite() {
        height.max(0.0)
    } else {
        0.0
    };
    let timeline_end = if timeline_end_beats.is_finite() {
        timeline_end_beats.max(1.0)
    } else {
        1.0
    };
    let (top_pitch, bottom_pitch) = match (
        notes.iter().map(|note| note.pitch).min(),
        notes.iter().map(|note| note.pitch).max(),
    ) {
        (Some(low), Some(high)) => {
            let top = high.saturating_add(2).min(127);
            let bottom = low.saturating_sub(2);
            // Always show at least an octave so a lone note has context.
            let bottom = bottom.min(top.saturating_sub(12));
            (top, bottom)
        }
        _ => (72, 60),
    };
    let rows = (top_pitch - bottom_pitch + 1).max(1) as f32;
    PianoRollGeometry {
        px_per_beat: width / timeline_end,
        px_per_pitch: height / rows,
        top_pitch,
        bottom_pitch,
    }
}

#[cfg(test)]
#[path = "arrange_model_tests.rs"]
mod tests;
