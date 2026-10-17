//! The compositor: a layer stack from [`jvid_core::eval::video_at`] blended over the project
//! background into one project-sized frame, at preview scale for the viewer or full size for export.

use jvid_core::eval::Layer;
use wgpu::util::DeviceExt;

use crate::preview::{place, scaled, to_clip};

/// A layer's opacity param, 0..1; 1 when unset.
pub fn opacity(layer: &Layer) -> f32 {
    layer
        .params
        .get("opacity")
        .map_or(1.0, |&o| o.clamp(0.0, 1.0) as f32)
}

/// The composited frame format: premultiplied RGBA, which is straight when the background is opaque.
///
/// Blending is deliberately in gamma space: the stored values are non-linear sRGB and are mixed
/// as-is (an `Unorm`, not `UnormSrgb`, target), matching most NLEs' default and the decoded frames.
/// shortcut: gamma-space blending darkens soft edges and cross-fades; switch to linear light
/// (an `Rgba16Float` target, decode to linear before blending) as a follow-up.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// GPU state for compositing; the output texture is kept while the frame size stays the same.
pub struct Compositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    target: Option<wgpu::Texture>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_layer"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_layer"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            target: None,
        }
    }

    /// Composites `layers` (top first, as `video_at` returns them, each an RGBA image such as
    /// [`crate::preview::Preview::image`] with its [`opacity`]) over `background` (straight RGBA)
    /// into a `size` frame at 1/`scale` (1 for export), each fitted whole and centred.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        scale: u32,
        background: [u8; 4],
        layers: &[(&wgpu::Texture, f32)],
    ) -> &wgpu::Texture {
        let (w, h) = scaled(size.0, size.1, scale);
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.width(), t.height()) != (w, h))
        {
            self.target = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("composite"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        let target = self.target.as_ref().unwrap();
        let view_rect = [0.0, 0.0, w as f32, h as f32];
        // shortcut: a uniform and bind group per layer per frame; pool them if playback (#32) profiles hot.
        let binds: Vec<_> = layers
            .iter()
            .rev()
            .filter(|(_, o)| *o > 0.0)
            .map(|(tex, o)| {
                let rect = place([tex.width() as f32, tex.height() as f32], view_rect, None);
                let mut p = to_clip(rect, view_rect).to_vec();
                p.extend([*o, 0.0, 0.0, 0.0]);
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("layer"),
                    contents: &p.iter().flat_map(|f| f.to_ne_bytes()).collect::<Vec<_>>(),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(tex.format().remove_srgb_suffix()),
                    ..Default::default()
                });
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: uniform.as_entire_binding(),
                        },
                    ],
                })
            })
            .collect();
        let a = background[3] as f64 / 255.0;
        let c = |i: usize| background[i] as f64 / 255.0 * a;
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let view = target.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c(0),
                            g: c(1),
                            b: c(2),
                            a,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for bind in &binds {
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
        target
    }
}

/// Copies an RGBA8 texture back to the CPU as tight rows, blocking until the GPU is done.
pub fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    assert_eq!(
        tex.format().block_copy_size(None),
        Some(4),
        "read_back expects 4 bytes/px, got {:?}",
        tex.format()
    );
    let row = (tex.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * tex.height()) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        tex.size(),
    );
    queue.submit([encoder.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let data = buf.slice(..).get_mapped_range().unwrap();
    data.chunks(row as usize)
        .flat_map(|r| &r[..tex.width() as usize * 4])
        .copied()
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
