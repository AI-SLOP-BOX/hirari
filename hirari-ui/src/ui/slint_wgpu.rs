//! Same-window WGPU bridge for dense plot imagery.
//!
//! Slint owns the window and input tree. Its WGPU rendering notifier exposes
//! the exact device and queue used for that frame, so a texture created here is
//! imported back into the Slint scene without a second native window.
//!
//! The pixel rasterizers live in [`super::plot_raster`] so the AmeUI Canvas
//! path uploads the identical bytes during the backend migration.

use crate::slint_ui::AppWindow;
use crate::ui_shared::gpu_canvas::PlotFrameStore;
use crate::ui_shared::plot_raster::{
    meter_rgba, piano_rgba, spectrum_rgba, waveform_rgba, HEIGHT, WIDTH,
};
use slint::wgpu_29::wgpu::util::DeviceExt;
use slint::ComponentHandle;
use std::cell::RefCell;

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
                        size: slint::wgpu_29::wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: slint::wgpu_29::wgpu::TextureDimension::D2,
                        format: slint::wgpu_29::wgpu::TextureFormat::Rgba8Unorm,
                        usage: slint::wgpu_29::wgpu::TextureUsages::TEXTURE_BINDING
                            | slint::wgpu_29::wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    },
                    slint::wgpu_29::wgpu::util::TextureDataOrder::default(),
                    pixels,
                )
            };
            if let Some(ui) = weak.upgrade() {
                if waveform_changed {
                    let texture = make_texture(
                        "hirari-slnt-waveform",
                        WIDTH,
                        HEIGHT,
                        &waveform_rgba(&frame),
                    );
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_waveform_image(image);
                        *last_waveform.borrow_mut() = frame.waveform_min_max.clone();
                    }
                }
                if spectrum_changed {
                    let texture =
                        make_texture("hirari-slnt-spectrum", WIDTH, 96, &spectrum_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_spectrum_image(image);
                        *last_spectrum.borrow_mut() = frame.spectrum.clone();
                    }
                }
                if meters_changed {
                    let texture = make_texture("hirari-slnt-meter", 48, 128, &meter_rgba(&frame));
                    if let Ok(image) = slint::Image::try_from(texture) {
                        ui.set_gpu_meter_image(image);
                        *last_meters.borrow_mut() = frame.meters.clone();
                    }
                }
                if piano_changed {
                    let texture = make_texture("hirari-slnt-piano", WIDTH, 72, &piano_rgba(&frame));
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
