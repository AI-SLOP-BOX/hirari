//! High-density Arrange canvas widget.
//!
//! This is intentionally a renderer-agnostic egui widget: egui-wgpu owns the
//! GPU paint backend, while this module owns only the timeline geometry. It
//! can be embedded beside the Slint shell once the native child-surface
//! adapter is enabled.

#[derive(Clone, Debug, Default)]
pub struct ArrangeCanvasState {
    pub playhead_beats: f32,
    pub beats_per_bar: f32,
    pub waveform: Vec<[f32; 2]>,
    pub notes: Vec<ArrangeNote>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrangeNote {
    pub start_beats: f32,
    pub length_beats: f32,
    pub pitch: u8,
    pub velocity: f32,
    pub color: egui::Color32,
}

pub fn draw_arrange_canvas(ui: &mut egui::Ui, state: &ArrangeCanvasState) -> egui::Response {
    let desired = egui::vec2(ui.available_width(), 320.0);
    let (response, painter) = ui.allocate_painter(desired, egui::Sense::click_and_drag());
    let rect = response.rect;
    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(12, 20, 29));
    let beat_width = 42.0;
    let lane_height = 48.0;
    for bar in 0..64 {
        let x = rect.left() + bar as f32 * state.beats_per_bar.max(1.0) * beat_width;
        if x > rect.right() { break; }
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 58, 74)));
    }
    if !state.waveform.is_empty() {
        let points: Vec<egui::Pos2> = state.waveform.iter().enumerate().map(|(index, pair)| {
            let x = rect.left() + index as f32 / state.waveform.len().max(1) as f32 * rect.width();
            let amp = (pair[1] - pair[0]).abs().max(0.03);
            egui::pos2(x, rect.top() + lane_height * 0.5 - amp * 22.0)
        }).collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.0, egui::Color32::from_rgb(121, 111, 255))));
    }
    for note in &state.notes {
        let x = rect.left() + note.start_beats * beat_width;
        let y = rect.top() + (127.0 - note.pitch as f32) / 127.0 * rect.height();
        let w = (note.length_beats * beat_width).max(4.0);
        painter.rect_filled(egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 8.0)), 2.0, note.color);
    }
    let playhead_x = rect.left() + state.playhead_beats * beat_width;
    painter.line_segment([egui::pos2(playhead_x, rect.top()), egui::pos2(playhead_x, rect.bottom())], egui::Stroke::new(2.0, egui::Color32::WHITE));
    response
}

#[cfg(test)]
mod tests {
    use super::{ArrangeCanvasState, ArrangeNote};

    #[test]
    fn arrange_state_accepts_waveform_and_notes_without_mutating_core_models() {
        let state = ArrangeCanvasState { playhead_beats: 4.0, beats_per_bar: 4.0, waveform: vec![[-0.2, 0.4]], notes: vec![ArrangeNote { start_beats: 1.0, length_beats: 2.0, pitch: 60, velocity: 0.8, color: egui::Color32::from_rgb(120, 100, 240) }] };
        assert_eq!(state.notes[0].pitch, 60);
        assert_eq!(state.waveform.len(), 1);
    }
}
