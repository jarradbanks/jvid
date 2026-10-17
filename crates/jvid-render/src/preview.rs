//! The preview viewport: YUV frames uploaded and converted to RGB on the GPU, then drawn
//! fitted or zoomed inside the viewer.

use wgpu::util::DeviceExt;

/// A YUV 4:2:0 frame (I420): tight rows, chroma `(w+1)/2 × (h+1)/2`.
pub struct Yuv<'a> {
    pub width: u32,
    pub height: u32,
    pub planes: [&'a [u8]; 3],
    /// BT.709 matrix when true, BT.601 when false.
    pub bt709: bool,
    pub full_range: bool,
}

/// The YUV→RGB affine transform as the shader takes it: `rgb[i] = dot(rows[i].xyz, yuv) + rows[i].w`,
/// with 8-bit codes read as 0..1.
pub fn yuv_to_rgb(bt709: bool, full_range: bool) -> [[f32; 4]; 3] {
    let (kr, kb) = if bt709 {
        (0.2126, 0.0722)
    } else {
        (0.299, 0.114)
    };
    let kg = 1.0 - kr - kb;
    // Y' = ys·y + yo and Cb, Cr = cs·c + co, from codes over 255.
    let (ys, yo, cs) = if full_range {
        (1.0, 0.0, 1.0)
    } else {
        (255.0 / 219.0, -16.0 / 219.0, 255.0 / 224.0)
    };
    let co = -128.0 / 255.0 * cs;
    let (rv, gu, gv, bu) = (
        2.0 * (1.0 - kr),
        -2.0 * kb * (1.0 - kb) / kg,
        -2.0 * kr * (1.0 - kr) / kg,
        2.0 * (1.0 - kb),
    );
    [
        [ys, 0.0, rv * cs, yo + rv * co],
        [ys, gu * cs, gv * cs, yo + (gu + gv) * co],
        [ys, bu * cs, 0.0, yo + bu * co],
    ]
}

/// Where a `frame`-sized image goes in `view` (x, y, w, h), as (x, y, w, h). `None` fits it whole,
/// letterboxed or pillarboxed; `Some(z)` shows `z` view units per frame pixel. Always centred.
pub fn place(frame: [f32; 2], view: [f32; 4], zoom: Option<f32>) -> [f32; 4] {
    let [fw, fh] = frame;
    let [vx, vy, vw, vh] = view;
    let z = zoom.unwrap_or_else(|| (vw / fw).min(vh / fh));
    let (w, h) = (fw * z, fh * z);
    [vx + (vw - w) / 2.0, vy + (vh - h) / 2.0, w, h]
}

/// `rect` (x, y, w, h) in `view`'s clip space: left, top, right, bottom, with y up.
pub fn to_clip(rect: [f32; 4], view: [f32; 4]) -> [f32; 4] {
    let x = |px: f32| (px - view[0]) / view[2] * 2.0 - 1.0;
    let y = |py: f32| 1.0 - (py - view[1]) / view[3] * 2.0;
    [
        x(rect[0]),
        y(rect[1]),
        x(rect[0] + rect[2]),
        y(rect[1] + rect[3]),
    ]
}

/// Converted frame size at a preview scale divisor (1, 2 or 4).
pub fn scaled(width: u32, height: u32, scale: u32) -> (u32, u32) {
    (width.div_ceil(scale).max(1), height.div_ceil(scale).max(1))
}

/// GPU state for one preview: the converted image and the pipelines that make and draw it.
pub struct Preview {
    convert: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// Where the blit draws, in clip space; see [`to_clip`].
    dst: wgpu::Buffer,
    image: Option<(wgpu::Texture, wgpu::BindGroup)>,
    /// The target is sRGB, so the blit samples through an sRGB view to keep the codes unchanged.
    srgb: bool,
}

const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Preview {
    /// `target` is the format of the pass [`Preview::paint`] draws into (single-sample, no depth).
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let pipeline = |src: &str, vs: &str, fs: &str, format: wgpu::TextureFormat, strip: bool| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(fs),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: if strip {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(format.into())],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            convert: pipeline(
                include_str!("convert.wgsl"),
                "vs_full",
                "fs_convert",
                IMAGE_FORMAT,
                false,
            ),
            blit: pipeline(
                include_str!("blit.wgsl"),
                "vs_blit",
                "fs_blit",
                target,
                true,
            ),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            dst: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview dst"),
                contents: &f32s(&[-1.0, 1.0, 1.0, -1.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            image: None,
            srgb: target.is_srgb(),
        }
    }

    /// Uploads `frame` and converts it to RGB at 1/`scale` of its size, replacing the image.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Yuv, scale: u32) {
        let (cw, ch) = (frame.width.div_ceil(2), frame.height.div_ceil(2));
        // shortcut: new plane textures per frame; keep them across frames once playback (#25) uploads 30+ a second.
        let planes: [_; 3] = std::array::from_fn(|i| {
            let (w, h) = if i == 0 {
                (frame.width, frame.height)
            } else {
                (cw, ch)
            };
            let p = frame.planes[i];
            let tex = texture(device, w, h, wgpu::TextureFormat::R8Unorm, &[]);
            queue.write_texture(
                tex.as_image_copy(),
                p,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: None,
                },
                tex.size(),
            );
            tex.create_view(&Default::default())
        });
        let rows = yuv_to_rgb(frame.bt709, frame.full_range);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("yuv to rgb"),
            contents: &f32s(rows.as_flattened()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (w, h) = scaled(frame.width, frame.height, scale);
        let srgb_view = [IMAGE_FORMAT.add_srgb_suffix()];
        let image = texture(device, w, h, IMAGE_FORMAT, &srgb_view);
        let convert = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.convert.get_bind_group_layout(0),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&planes[0])),
                entry(1, wgpu::BindingResource::TextureView(&planes[1])),
                entry(2, wgpu::BindingResource::TextureView(&planes[2])),
                entry(3, wgpu::BindingResource::Sampler(&self.sampler)),
                entry(4, uniform.as_entire_binding()),
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let view = image.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("yuv to rgb"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.convert);
            pass.set_bind_group(0, &convert, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);

        let view = image.create_view(&wgpu::TextureViewDescriptor {
            format: Some(if self.srgb {
                srgb_view[0]
            } else {
                IMAGE_FORMAT
            }),
            ..Default::default()
        });
        let blit = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.blit.get_bind_group_layout(0),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&view)),
                entry(1, wgpu::BindingResource::Sampler(&self.sampler)),
                entry(2, self.dst.as_entire_binding()),
            ],
        });
        self.image = Some((image, blit));
    }

    /// The converted RGB image, once a frame has been uploaded.
    pub fn image(&self) -> Option<&wgpu::Texture> {
        self.image.as_ref().map(|(t, _)| t)
    }

    /// Sets where [`Preview::paint`] draws, in the pass viewport's clip space ([`to_clip`]).
    pub fn set_dst(&self, queue: &wgpu::Queue, clip: [f32; 4]) {
        queue.write_buffer(&self.dst, 0, &f32s(&clip));
    }

    /// Draws the image into the current viewport; nothing before the first upload.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some((_, bind)) = &self.image {
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}

fn texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    view_formats: &[wgpu::TextureFormat],
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats,
    })
}

fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

#[cfg(test)]
mod tests;
