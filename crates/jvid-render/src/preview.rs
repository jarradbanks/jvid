//! The preview viewport: YUV frames uploaded and converted to RGB on the GPU, then drawn
//! fitted or zoomed inside the viewer.

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jvid_core::hdr::Hdr;
use wgpu::util::DeviceExt;

/// A YUV 4:2:0 frame (I420): tight rows, chroma `(w+1)/2 × (h+1)/2`.
pub struct Yuv<'a> {
    pub width: u32,
    pub height: u32,
    pub planes: [&'a [u8]; 3],
    /// BT.709 matrix when true, BT.601 when false.
    pub bt709: bool,
    pub full_range: bool,
    /// PQ or HLG: samples are 10-bit little-endian `u16`s, the matrix is BT.2020, and the
    /// preview is tone-mapped to SDR (see convert.wgsl).
    pub hdr: Option<Hdr>,
}

/// The YUV→RGB affine transform as the shader takes it: `rgb[i] = dot(rows[i].xyz, yuv) + rows[i].w`,
/// with 8-bit codes read as 0..1.
pub fn yuv_to_rgb(bt709: bool, full_range: bool) -> [[f32; 4]; 3] {
    if bt709 {
        matrix(0.2126, 0.0722, full_range)
    } else {
        matrix(0.299, 0.114, full_range)
    }
}

/// [`yuv_to_rgb`] for luma weights `kr` and `kb`.
fn matrix(kr: f32, kb: f32, full_range: bool) -> [[f32; 4]; 3] {
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

/// Cumulative upload allocations, excluding pipelines and decode/composition work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadAllocations {
    pub plane_textures: u64,
    pub conversion_buffers: u64,
    pub converted_images: u64,
    pub bind_groups: u64,
}

/// Owns a converted image until its last consumer releases it. Keep this lease through
/// command encoding and submission (and while retaining a frame); clone the lease rather than
/// the underlying wgpu handle. Submitted reads precede later uploads on the same queue.
#[derive(Clone)]
pub struct Image(Arc<DisplayImage>);

impl Deref for Image {
    type Target = wgpu::Texture;
    fn deref(&self) -> &Self::Target {
        &self.0.texture
    }
}

struct DisplayImage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    blit: wgpu::BindGroup,
    // paint() has no submission boundary/lease in its API, so these outputs are never recycled.
    painted: AtomicBool,
}

struct Inputs {
    key: (u32, u32, bool),
    planes: [wgpu::Texture; 3],
    _views: [wgpu::TextureView; 3],
    uniform: wgpu::Buffer,
    bindings: [wgpu::BindGroup; 3],
}

/// At most three compatible converted outputs retained by a renderer. Leased images evicted
/// from this pool remain alive with their consumers and are never modified again.
const OUTPUT_POOL_LIMIT: usize = 3;

/// GPU state for one preview: the converted image and the pipelines that make and draw it.
pub struct Preview {
    convert: wgpu::RenderPipeline,
    /// [`Self::convert`] for PQ and HLG frames.
    pq: wgpu::RenderPipeline,
    hlg: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// For 10-bit planes, whose two bytes must not be filtered apart; see `deep` in convert.wgsl.
    nearest: wgpu::Sampler,
    /// Where the blit draws, in clip space; see [`to_clip`].
    dst: wgpu::Buffer,
    image: Option<Image>,
    inputs: Option<Inputs>,
    outputs: Vec<Image>,
    /// The target is sRGB, so the blit samples through an sRGB view to keep the codes unchanged.
    srgb: bool,
    allocations: UploadAllocations,
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
        let convert = |fs| {
            pipeline(
                include_str!("convert.wgsl"),
                "vs_full",
                fs,
                IMAGE_FORMAT,
                false,
            )
        };
        Self {
            convert: convert("fs_convert"),
            pq: convert("fs_pq"),
            hlg: convert("fs_hlg"),
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
            nearest: device.create_sampler(&Default::default()),
            dst: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview dst"),
                contents: &f32s(&[-1.0, 1.0, 1.0, -1.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            image: None,
            inputs: None,
            outputs: Vec::new(),
            srgb: target.is_srgb(),
            allocations: UploadAllocations::default(),
        }
    }

    pub fn upload_allocations(&self) -> UploadAllocations {
        self.allocations
    }

    /// Uploads `frame` and converts it to RGB at 1/`scale` of its size, replacing the image.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Yuv, scale: u32) {
        let key = (frame.width, frame.height, frame.hdr.is_some());
        let size = scaled(frame.width, frame.height, scale);
        self.image = None;
        if self.inputs.as_ref().is_none_or(|inputs| inputs.key != key) {
            self.outputs.clear();
            let format = if key.2 {
                wgpu::TextureFormat::Rg8Unorm
            } else {
                wgpu::TextureFormat::R8Unorm
            };
            let planes: [_; 3] = std::array::from_fn(|i| {
                let (w, h) = if i == 0 {
                    (key.0, key.1)
                } else {
                    (key.0.div_ceil(2), key.1.div_ceil(2))
                };
                texture(device, w, h, format, &[])
            });
            let views = planes
                .each_ref()
                .map(|plane| plane.create_view(&Default::default()));
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("yuv to rgb"),
                size: 48,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // Auto layouts belong to their pipeline, even when their entries are identical.
            let bindings = [&self.convert, &self.pq, &self.hlg].map(|pipeline| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("retained YUV inputs"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        entry(0, wgpu::BindingResource::TextureView(&views[0])),
                        entry(1, wgpu::BindingResource::TextureView(&views[1])),
                        entry(2, wgpu::BindingResource::TextureView(&views[2])),
                        entry(
                            3,
                            wgpu::BindingResource::Sampler(if key.2 {
                                &self.nearest
                            } else {
                                &self.sampler
                            }),
                        ),
                        entry(4, uniform.as_entire_binding()),
                    ],
                })
            });
            self.inputs = Some(Inputs {
                key,
                planes,
                _views: views,
                uniform,
                bindings,
            });
            self.allocations.plane_textures += 3;
            self.allocations.conversion_buffers += 1;
            self.allocations.bind_groups += 3;
        }
        self.outputs.retain(|image| {
            (image.width(), image.height()) == size && !image.0.painted.load(Ordering::Relaxed)
        });
        let image = self
            .outputs
            .iter()
            .find(|image| Arc::strong_count(&image.0) == 1)
            .cloned()
            .unwrap_or_else(|| {
                let tex = texture(
                    device,
                    size.0,
                    size.1,
                    IMAGE_FORMAT,
                    &[IMAGE_FORMAT.add_srgb_suffix()],
                );
                let image = self.display_image(device, tex);
                self.allocations.converted_images += 1;
                if self.outputs.len() == OUTPUT_POOL_LIMIT {
                    self.outputs.remove(0);
                }
                self.outputs.push(image.clone());
                image
            });
        let inputs = self.inputs.as_ref().expect("initialized above");
        for (plane, bytes) in inputs.planes.iter().zip(frame.planes) {
            queue.write_texture(
                plane.as_image_copy(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plane.width() * if key.2 { 2 } else { 1 }),
                    rows_per_image: None,
                },
                plane.size(),
            );
        }
        let (rows, pipeline, binding) = match frame.hdr {
            None => (yuv_to_rgb(frame.bt709, frame.full_range), &self.convert, 0),
            Some(hdr) => (
                matrix(0.2627, 0.0593, frame.full_range),
                if hdr == Hdr::Pq { &self.pq } else { &self.hlg },
                if hdr == Hdr::Pq { 1 } else { 2 },
            ),
        };
        queue.write_buffer(&inputs.uniform, 0, &f32s(rows.as_flattened()));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("yuv to rgb"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &image.0.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations::default(),
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &inputs.bindings[binding], &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        self.image = Some(image);
    }

    fn display_image(&mut self, device: &wgpu::Device, texture: wgpu::Texture) -> Image {
        // Conversion writes linear UNORM codes; only the blit samples through sRGB when the
        // destination itself encodes sRGB. These views must remain distinct.
        let linear = texture.create_view(&Default::default());
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(if self.srgb {
                IMAGE_FORMAT.add_srgb_suffix()
            } else {
                IMAGE_FORMAT
            }),
            ..Default::default()
        });
        let blit = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview image"),
            layout: &self.blit.get_bind_group_layout(0),
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&view)),
                entry(1, wgpu::BindingResource::Sampler(&self.sampler)),
                entry(2, self.dst.as_entire_binding()),
            ],
        });
        self.allocations.bind_groups += 1;
        Image(Arc::new(DisplayImage {
            texture,
            view: linear,
            blit,
            painted: AtomicBool::new(false),
        }))
    }

    /// Makes an externally owned composited image the one [`Preview::paint`] draws.
    pub fn show_image(&mut self, device: &wgpu::Device, image: wgpu::Texture) {
        self.outputs.clear();
        self.inputs = None;
        self.image = Some(self.display_image(device, image));
    }

    /// A lease on the converted RGB image. Retain it until all reads have been submitted, or
    /// while keeping an old frame, so later uploads cannot overwrite its pixels.
    pub fn image(&self) -> Option<Image> {
        self.image.clone()
    }

    /// Sets where [`Preview::paint`] draws, in the pass viewport's clip space ([`to_clip`]).
    pub fn set_dst(&self, queue: &wgpu::Queue, clip: [f32; 4]) {
        queue.write_buffer(&self.dst, 0, &f32s(&clip));
    }

    /// Draws the image into the current viewport; nothing before the first upload.
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(image) = &self.image {
            image.0.painted.store(true, Ordering::Relaxed);
            self.paint_image(pass, image);
        }
    }

    /// Draws an explicitly leased image. The caller retains `image` through submission of
    /// this render pass, permitting safe output reuse once that lease is released.
    pub fn paint_image(&self, pass: &mut wgpu::RenderPass<'_>, image: &Image) {
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &image.0.blit, &[]);
        pass.draw(0..4, 0..1);
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
