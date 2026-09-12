//! Optional native GPU upload path for dense DAW visualizations.
//!
//! This module deliberately owns no window. Slint remains the window and
//! input shell; a platform surface adapter can later borrow the device and
//! render into a child surface without changing the PlotFrame contract.

use super::gpu_canvas::PlotFrame;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PlotVertex {
    position: [f32; 2],
    color: [f32; 4],
}

fn waveform_vertices(frame: &PlotFrame) -> Vec<PlotVertex> {
    let mut vertices = Vec::with_capacity(frame.waveform_min_max.len() * 2);
    let count = frame.waveform_min_max.len().max(1) as f32;
    for (index, pair) in frame.waveform_min_max.iter().enumerate() {
        let x = index as f32 / (count - 1.0).max(1.0) * 2.0 - 1.0;
        vertices.push(PlotVertex { position: [x, pair[0].clamp(-1.0, 1.0)], color: [0.55, 0.49, 1.0, 0.75] });
        vertices.push(PlotVertex { position: [x, pair[1].clamp(-1.0, 1.0)], color: [0.34, 0.78, 0.82, 0.9] });
    }
    vertices
}

pub struct WgpuCanvas {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    waveform_buffer: wgpu::Buffer,
    spectrum_buffer: wgpu::Buffer,
    meter_buffer: wgpu::Buffer,
    plot_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
}

impl WgpuCanvas {
    pub async fn initialize() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            dx12_shader_compiler: wgpu::Dx12Compiler::Fxc,
            gles_minor_version: wgpu::Gles3MinorVersion::Automatic,
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok_or_else(|| "no compatible native GPU adapter".to_owned())?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("aura-gpu-canvas"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|error| format!("wgpu device creation failed: {error}"))?;
        let usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let waveform_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("aura-waveform-lod"), size: 512 * 8, usage, mapped_at_creation: false });
        let spectrum_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("aura-spectrum"), size: 4096, usage, mapped_at_creation: false });
        let meter_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("aura-meters"), size: 1024, usage, mapped_at_creation: false });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aura-plot-shader"),
            source: wgpu::ShaderSource::Wgsl(r#"
                struct VertexIn { @location(0) position: vec2<f32>, @location(1) color: vec4<f32> }
                struct VertexOut { @builtin(position) position: vec4<f32>, @location(0) color: vec4<f32> }
                @vertex fn vs_main(input: VertexIn) -> VertexOut {
                    var out: VertexOut;
                    out.position = vec4<f32>(input.position, 0.0, 1.0);
                    out.color = input.color;
                    return out;
                }
                @fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> { return input.color; }
            "#.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("aura-plot-layout"), bind_group_layouts: &[], push_constant_ranges: &[] });
        let plot_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aura-waveform-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PlotVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("aura-plot-vertices"), size: 4096 * std::mem::size_of::<PlotVertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Ok(Self { device, queue, waveform_buffer, spectrum_buffer, meter_buffer, plot_pipeline, vertex_buffer })
    }

    pub fn upload_frame(&self, frame: &PlotFrame) {
        let waveform: Vec<f32> = frame.waveform_min_max.iter().flat_map(|pair| *pair).collect();
        let spectrum = frame.spectrum.as_slice();
        let meters = frame.meters.as_slice();
        if !waveform.is_empty() { self.queue.write_buffer(&self.waveform_buffer, 0, bytemuck::cast_slice(&waveform)); }
        if !spectrum.is_empty() { self.queue.write_buffer(&self.spectrum_buffer, 0, bytemuck::cast_slice(spectrum)); }
        if !meters.is_empty() { self.queue.write_buffer(&self.meter_buffer, 0, bytemuck::cast_slice(meters)); }
    }

    /// Render the current waveform into a GPU texture. The texture can be
    /// copied into a native child surface or sampled by a future Slint host.
    pub fn render_waveform(&self, frame: &PlotFrame, width: u32, height: u32) -> Result<wgpu::Texture, String> {
        if width == 0 || height == 0 { return Err("plot target must be non-zero".to_owned()); }
        let vertices = waveform_vertices(frame);
        if vertices.is_empty() { return Err("plot frame has no waveform samples".to_owned()); }
        self.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aura-waveform-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("aura-waveform-encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aura-waveform-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.04, g: 0.06, b: 0.09, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.plot_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use super::waveform_vertices;
    use crate::ui::gpu_canvas::PlotFrame;

    #[test]
    fn waveform_vertices_are_normalized_for_gpu_clip_space() {
        let frame = PlotFrame { revision: 0, waveform_min_max: vec![[-2.0, 2.0], [-0.5, 0.5]], spectrum: vec![], meters: vec![], piano_notes: vec![] };
        let vertices = waveform_vertices(&frame);
        assert_eq!(vertices.len(), 4);
        assert_eq!(vertices[0].position, [-1.0, -1.0]);
        assert_eq!(vertices[1].position, [-1.0, 1.0]);
        assert_eq!(vertices[2].position[0], 1.0);
    }
}
