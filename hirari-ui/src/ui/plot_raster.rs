//! Backend-neutral CPU rasterizers for the dense plot surfaces.
//!
//! These turn a [`PlotFrame`] into RGBA pixels. Both the Slint WGPU notifier
//! and the AmeUI Canvas upload path consume the same bytes, so the draw step
//! stays identical while the windowing backend is migrated.

use crate::ui::gpu_canvas::PlotFrame;

pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 168;

pub fn waveform_rgba(frame: &PlotFrame) -> Vec<u8> {
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let index = ((y * WIDTH + x) * 4) as usize;
            let grid = if x % 96 == 0 || y % 42 == 0 { 18 } else { 9 };
            pixels[index] = 8;
            pixels[index + 1] = 13;
            pixels[index + 2] = grid;
            pixels[index + 3] = 255;
        }
    }
    let samples = frame.waveform_min_max.len().max(1) as u32;
    for (bucket, pair) in frame.waveform_min_max.iter().enumerate() {
        let x = (bucket as u32 * WIDTH / samples).min(WIDTH - 1);
        let min_y = ((1.0 - pair[1].clamp(-1.0, 1.0)) * 0.5 * (HEIGHT - 1) as f32) as u32;
        let max_y = ((1.0 - pair[0].clamp(-1.0, 1.0)) * 0.5 * (HEIGHT - 1) as f32) as u32;
        for y in min_y.min(max_y)..=min_y.max(max_y).min(HEIGHT - 1) {
            let index = ((y * WIDTH + x) * 4) as usize;
            pixels[index] = 139;
            pixels[index + 1] = 124;
            pixels[index + 2] = 255;
            pixels[index + 3] = 235;
        }
    }
    pixels
}

pub fn spectrum_rgba(frame: &PlotFrame) -> Vec<u8> {
    let mut pixels = vec![0u8; (WIDTH * 96 * 4) as usize];
    for (index, value) in frame.spectrum.iter().enumerate() {
        let x = (index as u32 * WIDTH / frame.spectrum.len().max(1) as u32).min(WIDTH - 1);
        let height = (value.clamp(0.0, 1.0) * 92.0) as u32;
        for y in 0..height {
            let row = 95 - y;
            let offset = ((row * WIDTH + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&[59, 198, 200, 220]);
        }
    }
    pixels
}

pub fn meter_rgba(frame: &PlotFrame) -> Vec<u8> {
    let width = 48u32;
    let height = 128u32;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for (index, value) in frame.meters.iter().take(2).enumerate() {
        let x = 6 + index as u32 * 20;
        let level = (value.clamp(0.0, 1.0) * 116.0) as u32;
        for y in 0..level {
            let row = height - 1 - y;
            let offset = ((row * width + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(if *value > 0.9 {
                &[217, 74, 74, 255]
            } else {
                &[53, 211, 154, 255]
            });
            for dx in 1..=7 {
                let fill = ((row * width + x + dx) * 4) as usize;
                pixels[fill..fill + 4].copy_from_slice(if *value > 0.9 {
                    &[217, 74, 74, 255]
                } else {
                    &[53, 211, 154, 255]
                });
            }
        }
    }
    pixels
}

pub fn piano_rgba(frame: &PlotFrame) -> Vec<u8> {
    let width = WIDTH;
    let height = 72u32;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for note in &frame.piano_notes {
        let x = (note[0].clamp(0.0, 1.0) * width as f32) as u32;
        let note_width = (note[1].clamp(0.002, 1.0) * width as f32).max(3.0) as u32;
        let y = ((1.0 - note[2].clamp(0.0, 1.0)) * (height - 10) as f32) as u32;
        let alpha = (120.0 + note[3].clamp(0.0, 1.0) * 120.0) as u8;
        for yy in y..(y + 6).min(height) {
            for xx in x..(x + note_width).min(width) {
                let offset = ((yy * width + xx) * 4) as usize;
                pixels[offset..offset + 4].copy_from_slice(&[139, 124, 255, alpha]);
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::{meter_rgba, piano_rgba, spectrum_rgba, waveform_rgba, HEIGHT, WIDTH};
    use crate::ui::gpu_canvas::PlotFrame;

    #[test]
    fn waveform_texture_has_stable_rgba_dimensions() {
        let frame = PlotFrame {
            revision: 1,
            waveform_min_max: vec![[-0.8, 0.8]],
            spectrum: vec![],
            meters: vec![],
            piano_notes: vec![],
        };
        assert_eq!(waveform_rgba(&frame).len(), (WIDTH * HEIGHT * 4) as usize);
    }

    #[test]
    fn dense_plot_textures_have_expected_dimensions() {
        let frame = PlotFrame {
            revision: 2,
            waveform_min_max: vec![[-0.5, 0.5]],
            spectrum: vec![0.2, 0.8, 0.4],
            meters: vec![0.3, 0.7],
            piano_notes: vec![[0.1, 0.2, 0.6, 0.9]],
        };
        assert_eq!(spectrum_rgba(&frame).len(), (WIDTH * 96 * 4) as usize);
        assert_eq!(meter_rgba(&frame).len(), 48 * 128 * 4);
        assert_eq!(piano_rgba(&frame).len(), (WIDTH * 72 * 4) as usize);
    }

    #[test]
    fn piano_texture_contains_note_pixels() {
        let empty = PlotFrame::default();
        let with_note = PlotFrame {
            piano_notes: vec![[0.25, 0.12, 0.62, 1.0]],
            ..PlotFrame::default()
        };
        assert_ne!(piano_rgba(&empty), piano_rgba(&with_note));
    }
}
