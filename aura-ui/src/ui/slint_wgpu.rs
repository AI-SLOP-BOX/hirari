//! Same-window WGPU bridge for dense plot imagery.
//!
//! Slint owns the window and input tree. Its WGPU rendering notifier exposes
//! the exact device and queue used for that frame, so a texture created here is
//! imported back into the Slint scene without a second native window.

use super::gpu_canvas::{PlotFrame, PlotFrameStore};
use crate::slint_ui::AppWindow;
use slint::ComponentHandle;
use slint::wgpu_29::wgpu::util::DeviceExt;
use std::cell::RefCell;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 168;

fn waveform_rgba(frame: &PlotFrame) -> Vec<u8> {
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

fn spectrum_rgba(frame: &PlotFrame) -> Vec<u8> {
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

fn meter_rgba(frame: &PlotFrame) -> Vec<u8> {
    let width = 48u32;
    let height = 128u32;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for (index, value) in frame.meters.iter().take(2).enumerate() {
        let x = 6 + index as u32 * 20;
        let level = (value.clamp(0.0, 1.0) * 116.0) as u32;
        for y in 0..level {
            let row = height - 1 - y;
            let offset = ((row * width + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(if *value > 0.9 { &[217, 74, 74, 255] } else { &[53, 211, 154, 255] });
            for dx in 1..=7 {
                let fill = ((row * width + x + dx) * 4) as usize;
                pixels[fill..fill + 4].copy_from_slice(if *value > 0.9 { &[217, 74, 74, 255] } else { &[53, 211, 154, 255] });
            }
        }
    }
    pixels
}

fn piano_rgba(frame: &PlotFrame) -> Vec<u8> {
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

pub fn install(ui: &AppWindow, store: PlotFrameStore) -> Result<(), String> {
    let weak = ui.as_weak();
    let last_revision = RefCell::new(0u64);
    let last_waveform = RefCell::new(Vec::<[f32; 2]>::new());
    let last_spectrum = RefCell::new(Vec::<f32>::new());
    let last_meters = RefCell::new(Vec::<f32>::new());
    let last_piano = RefCell::new(Vec::<[f32; 4]>::new());
    ui.window()
        .set_rendering_notifier(move |state, graphics_api| {
            if !matches!(state, slint::RenderingState::BeforeRendering) {
                return;
            }
            let slint::GraphicsAPI::WGPU29 { device, queue, .. } = graphics_api else {
                return;
            };
            let frame = store.snapshot();
            let waveform_changed = frame.waveform_min_max != *last_waveform.borrow();
            let spectrum_changed = frame.spectrum != *last_spectrum.borrow();
            let meters_changed = frame.meters != *last_meters.borrow();
            let piano_changed = frame.piano_notes != *last_piano.borrow();
            if frame.revision == *last_revision.borrow()
                || (!waveform_changed && !spectrum_changed && !meters_changed && !piano_changed)
            {
                return;
            }
            let make_texture = |label: &'static str, width: u32, height: u32, pixels: &[u8]| {
                device.create_texture_with_data(
                queue,
                &slint::wgpu_29::wgpu::TextureDescriptor {
                    label: Some(label),
                    size: slint::wgpu_29::wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: slint::wgpu_29::wgpu::TextureDimension::D2,
                    format: slint::wgpu_29::wgpu::TextureFormat::Rgba8Unorm,
                    usage: slint::wgpu_29::wgpu::TextureUsages::TEXTURE_BINDING | slint::wgpu_29::wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                slint::wgpu_29::wgpu::util::TextureDataOrder::default(),
                pixels,
                )
            };
            if let Some(ui) = weak.upgrade() {
                if waveform_changed {
                    let texture = make_texture("aura-slnt-waveform", WIDTH, HEIGHT, &waveform_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_waveform_image(image);
                        *last_waveform.borrow_mut() = frame.waveform_min_max.clone();
                    }
                }
                if spectrum_changed {
                    let texture = make_texture("aura-slnt-spectrum", WIDTH, 96, &spectrum_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_spectrum_image(image);
                        *last_spectrum.borrow_mut() = frame.spectrum.clone();
                    }
                }
                if meters_changed {
                    let texture = make_texture("aura-slnt-meter", 48, 128, &meter_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_meter_image(image);
                        *last_meters.borrow_mut() = frame.meters.clone();
                    }
                }
                if piano_changed {
                    let texture = make_texture("aura-slnt-piano", WIDTH, 72, &piano_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_piano_image(image);
                        *last_piano.borrow_mut() = frame.piano_notes.clone();
                    }
                }
                if waveform_changed || spectrum_changed || meters_changed || piano_changed {
                    *last_revision.borrow_mut() = frame.revision;
                }
            }
        })
        .map_err(|error| format!("could not install Slint WGPU notifier: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{meter_rgba, piano_rgba, spectrum_rgba, waveform_rgba, HEIGHT, WIDTH};
    use crate::ui::gpu_canvas::PlotFrame;

    #[test]
    fn waveform_texture_has_stable_rgba_dimensions() {
        let frame = PlotFrame { revision: 1, waveform_min_max: vec![[-0.8, 0.8]], spectrum: vec![], meters: vec![], piano_notes: vec![] };
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
