//! Video preview frame synchronization.

use crate::slint_ui::*;
use aura_core_bridge::AuraCore;

pub(crate) fn update_video_preview(ui: &AppWindow, core: &AuraCore, last_video_revision: &mut u64) {
    // Video is a control-rate preview, not an audio-thread task.
    // Refresh it while playing and while stopped/scrubbing so the
    // monitor always reflects the current transport position.
    if ui.get_show_video() {
        let _ =
            core.request_video_frame(core.get_playhead() as f64 / core.get_sample_rate().max(1.0));
        let revision = core.get_video_frame_revision();
        if revision != 0 && revision != *last_video_revision {
            *last_video_revision = revision;
            let frame = core.get_video_frame();
            if frame.len() >= 8 {
                let width = u32::from_le_bytes(frame[0..4].try_into().unwrap_or([0; 4]));
                let height = u32::from_le_bytes(frame[4..8].try_into().unwrap_or([0; 4]));
                let expected = (width as usize)
                    .saturating_mul(height as usize)
                    .saturating_mul(4);
                if width > 0
                    && height > 0
                    && width <= 8192
                    && height <= 8192
                    && frame.len() == expected + 8
                {
                    let rgba: Vec<_> = frame[8..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| slint::Rgba8Pixel::new(p[0], p[1], p[2], p[3]))
                        .collect();
                    let mut pixel_buffer =
                        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
                    pixel_buffer.make_mut_slice().copy_from_slice(&rgba);
                    ui.set_video_frame(slint::Image::from_rgba8(pixel_buffer));
                }
            }
        }
    }
}
