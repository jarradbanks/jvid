//! The compositor: a layer stack from [`jvid_core::eval::video_at`] blended over the project
//! background into one project-sized frame, at preview scale for the viewer or full size for export.

use jvid_core::model::{Blend, Curves, Effect, Mask, Transform};
use wgpu::util::DeviceExt;

use crate::preview::{place, scaled};

/// The four corners of an `image`-sized layer under `t` in a `frame`-sized project, as
/// (clip x, clip y, u, v) in triangle-strip order; `None` when cropped away entirely.
///
/// The image is fitted whole and centred, then cropped, moved so its anchor sits at the frame
/// centre plus position, scaled and rotated clockwise about the anchor, all in project pixels.
pub fn quad(image: [f32; 2], frame: [f32; 2], t: &Transform) -> Option<[[f32; 4]; 4]> {
    let [_, _, w, h] = place(image, [0.0, 0.0, frame[0], frame[1]], None);
    let f = |v: f64| v as f32;
    let [l, top, r, b] = t.crop.map(f);
    let (u, v) = ([l / w, 1.0 - r / w], [top / h, 1.0 - b / h]);
    if u[0] >= u[1] || v[0] >= v[1] {
        return None;
    }
    let (sin, cos) = f(t.rotation).to_radians().sin_cos();
    Some(std::array::from_fn(|i| {
        let (u, v) = (u[i & 1], v[i >> 1]);
        let x = ((u - 0.5) * w - f(t.anchor[0])) * f(t.scale[0]);
        let y = ((v - 0.5) * h - f(t.anchor[1])) * f(t.scale[1]);
        // y is down, so this standard rotation turns clockwise on screen.
        let px = frame[0] / 2.0 + f(t.position[0]) + x * cos - y * sin;
        let py = frame[1] / 2.0 + f(t.position[1]) + x * sin + y * cos;
        [px / frame[0] * 2.0 - 1.0, 1.0 - py / frame[1] * 2.0, u, v]
    }))
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
    /// For non-normal blend modes: replaces pixels with a mix computed against `backdrop`.
    blend: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    target: Option<wgpu::Texture>,
    backdrop: Option<wgpu::Texture>,
    /// Uploaded LUTs by [`Transform::lut`] path, and the stand-in bound for layers without one.
    luts: std::collections::HashMap<String, wgpu::Texture>,
    no_lut: wgpu::Texture,
    /// Bound for layers whose curves are neutral; the shader skips it.
    no_curves: wgpu::Texture,
    /// Bound for layers whose secondary isn't limited by masks; the shader skips it.
    no_cover: wgpu::Texture,
    /// Effect pipelines by [`Effect::shader`], with their parameters in uniform order and pass
    /// count; layers naming an effect not set skip it.
    effects:
        std::collections::HashMap<String, (wgpu::RenderPipeline, Vec<crate::effect::Param>, u32)>,
    /// `src`, `smp`, `fx` at group 0: fixed, so a shader binding anything else fails in `set_effect`.
    effect_layout: wgpu::BindGroupLayout,
    /// Per layer index: a ping-pong texture pair and one uniform per effect pass, reused across
    /// frames.
    fx_pool: Vec<(Vec<wgpu::Texture>, Vec<wgpu::Buffer>)>,
    masks: crate::mask::Masker,
    /// The timeline frame number effects see as `fx.frame`.
    pub frame: u32,
    /// How much of the frame, from the left, draws with no grade: the Program monitor's
    /// before/after. 0 (export's) grades it all and 1 bypasses the grade.
    pub before: f32,
    /// By layer index: the layer is premultiplied, as a nested sequence's frame from another
    /// `Compositor` is. Unlisted layers are straight alpha.
    pub premultiplied: Vec<bool>,
    /// Square-pixel display dimensions by layer, independent of proxy/preview texture sizes.
    /// Missing entries keep the texture dimensions for direct renderer callers.
    pub logical_sizes: Vec<[f32; 2]>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let pipeline = |entry, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            pipeline: pipeline(
                "fs_layer",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            blend: pipeline("fs_blend", None),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            target: None,
            backdrop: None,
            luts: Default::default(),
            no_lut: device.create_texture(&wgpu::TextureDescriptor {
                label: Some("no lut"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }),
            no_curves: device.create_texture(&curve_desc(1, 1)),
            no_cover: device.create_texture(&wgpu::TextureDescriptor {
                label: Some("no cover"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }),
            effects: Default::default(),
            effect_layout: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("effect"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            }),
            fx_pool: Vec::new(),
            masks: crate::mask::Masker::new(device),
            frame: 0,
            before: 0.0,
            premultiplied: Vec::new(),
            logical_sizes: Vec::new(),
        }
    }

    /// Builds `shader` for layers whose effects name `key`, or removes it when `None`. A shader
    /// the device rejects is removed too, so those layers draw as if it weren't there.
    pub fn set_effect(
        &mut self,
        device: &wgpu::Device,
        key: &str,
        shader: Option<&crate::effect::Shader>,
    ) -> Result<(), String> {
        self.effects.remove(key);
        let Some(shader) = shader else {
            return Ok(());
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(key),
            bind_group_layouts: &[Some(&self.effect_layout)],
            ..Default::default()
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(key),
            source: wgpu::ShaderSource::Wgsl(shader.module.as_str().into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(key),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("jvid_vs"),
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
                entry_point: Some("jvid_fs"),
                targets: &[Some(FORMAT.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        // wgpu's native error futures are ready at once; still pending counts as a failure.
        let mut pop = std::pin::pin!(scope.pop());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match pop.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(None) => {
                self.effects.insert(
                    key.to_owned(),
                    (pipeline, shader.params.clone(), shader.passes),
                );
                Ok(())
            }
            std::task::Poll::Ready(Some(e)) => Err(e.to_string()),
            std::task::Poll::Pending => Err("the GPU didn't report whether it compiled".into()),
        }
    }

    /// `tex` with `effects` applied in order into `encoder`, or `None` when none of them is set;
    /// bypassed ones are skipped. One limited to a mask of `masks` (sized as [`Masker::apply`]
    /// takes `sizes`) mixes back into its input by that mask's coverage.
    /// `slot` (the layer index of `layers`) picks the pooled textures and uniforms, so layers
    /// don't share them.
    ///
    /// [`Masker::apply`]: crate::mask::Masker::apply
    #[allow(clippy::too_many_arguments)]
    fn apply_effects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        (slot, layers): (usize, usize),
        tex: &wgpu::Texture,
        effects: &[Effect],
        masks: &[Mask],
        sizes: ([f32; 2], [f32; 2], [f32; 2]),
        logical_size: [f32; 2],
    ) -> Option<wgpu::Texture> {
        if self.fx_pool.len() <= slot {
            self.fx_pool.resize_with(slot + 1, Default::default);
        }
        let (pool, uniforms) = &mut self.fx_pool[slot];
        if pool.first().is_some_and(|p| p.size() != tex.size()) {
            pool.clear();
        }
        // A pooled texture that's neither of `busy`, made when there's none: two ping-pong, and
        // a masked effect's input needs a third.
        let mut free = |busy: [Option<&wgpu::Texture>; 2]| {
            if let Some(t) = pool.iter().find(|t| !busy.contains(&Some(*t))) {
                return t.clone();
            }
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("effect"),
                size: tex.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            pool.push(t.clone());
            t
        };
        let mut out: Option<wgpu::Texture> = None;
        // `i` counts applied passes, so it never skips past `uniforms.len()`.
        let mut i = 0;
        for (k, e) in effects.iter().enumerate().filter(|(_, e)| e.enabled) {
            let Some((pipeline, params, passes)) = self.effects.get(&e.shader) else {
                continue;
            };
            // Its own mask slot past every layer's, so no two coverages share a texture.
            let cover = masks.iter().find(|m| e.mask == Some(m.id)).and_then(|m| {
                let at = layers * (k + 1) + slot;
                let m = std::slice::from_ref(m);
                self.masks
                    .apply(device, queue, encoder, at, tex, m, sizes, true)
            });
            let before = out.clone();
            let keep = cover.as_ref().and(before.as_ref());
            for pass in 0..*passes {
                let input = out.as_ref().unwrap_or(tex);
                let mut p = vec![
                    logical_size[0],
                    logical_size[1],
                    // Exact up to 2^24 frames (over 6 days at 30 fps).
                    self.frame as f32,
                    pass as f32,
                    input.width() as f32,
                    input.height() as f32,
                    0.0,
                    0.0,
                ];
                p.extend(params.iter().map(|q| {
                    let v = e.params.get(&q.name).copied().unwrap_or(q.default);
                    v.clamp(q.min, q.max) as f32
                }));
                p.resize(p.len().next_multiple_of(4), 0.0);
                let bytes: Vec<u8> = p.iter().flat_map(|f| f.to_ne_bytes()).collect();
                if uniforms
                    .get(i)
                    .is_none_or(|u| u.size() != bytes.len() as u64)
                {
                    let u = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("effect"),
                        size: bytes.len() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    match uniforms.get_mut(i) {
                        Some(old) => *old = u,
                        None => uniforms.push(u),
                    }
                }
                queue.write_buffer(&uniforms[i], 0, &bytes);
                let view = input.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(input.format().remove_srgb_suffix()),
                    ..Default::default()
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.effect_layout,
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
                            resource: uniforms[i].as_entire_binding(),
                        },
                    ],
                });
                let next = free([out.as_ref(), keep]);
                let target = next.create_view(&Default::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("effect"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..4, 0..1);
                drop(pass);
                out = Some(next);
                i += 1;
            }
            if let (Some(cover), Some(fx)) = (cover, &out) {
                let next = free([Some(fx), before.as_ref()]);
                let orig = before.as_ref().unwrap_or(tex);
                self.masks.mix(device, encoder, [orig, fx, &cover], &next);
                out = Some(next);
            }
        }
        out
    }

    /// Uploads `lut` for layers whose [`Transform::lut`] is `path`; layers naming a path not
    /// added draw without a LUT.
    pub fn add_lut(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
        lut: &crate::lut::Lut,
    ) {
        let tex = crate::lut::texture(device, queue, lut);
        self.luts.insert(path.to_owned(), tex);
    }

    /// Composites `layers` (top first, as `video_at` returns them, each an RGBA image such as
    /// [`crate::preview::Preview::image`] with its evaluated transform) over `background`
    /// (straight RGBA) into a `size` frame at 1/`scale` (1 for export); see [`quad`].
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        scale: u32,
        background: [u8; 4],
        layers: &[(&wgpu::Texture, Transform)],
    ) -> &wgpu::Texture {
        let (w, h) = scaled(size.0, size.1, scale);
        let mut encoder = device.create_command_encoder(&Default::default());
        let effected: Vec<_> = layers
            .iter()
            .enumerate()
            .map(|(i, (tex, t))| {
                let frame = [size.0 as f32, size.1 as f32];
                let logical = self
                    .logical_sizes
                    .get(i)
                    .copied()
                    .unwrap_or([tex.width() as f32, tex.height() as f32]);
                let [_, _, fw, fh] = place(logical, [0.0, 0.0, frame[0], frame[1]], None);
                let sizes = ([fw, fh], frame, [w as f32, h as f32]);
                // Masks an effect is limited to only limit it.
                let own: Vec<Mask> = (t.masks.iter())
                    .filter(|m| m.id == 0 || !t.effects.iter().any(|e| e.mask == Some(m.id)))
                    .cloned()
                    .collect();
                // Masks that limit the secondary leave the image whole and give their coverage.
                let cover = t.secondary.masked;
                let masked =
                    self.masks
                        .apply(device, queue, &mut encoder, i, tex, &own, sizes, cover);
                // Preserve the pre-existing Full-quality mask input size. A normal alpha
                // mask historically enlarged a smaller source to its fitted project extent;
                // keep that reference fixed even when its sampled texture is now reduced.
                let effect_logical = if !cover && masked.is_some() {
                    let k = (fw / logical[0]).max(1.0);
                    logical.map(|v| (v * k).round())
                } else {
                    logical
                };
                let slot = (i, layers.len());
                let (fx, t) = (&t.effects, &t.masks);
                if cover {
                    let fx = self.apply_effects(
                        device,
                        queue,
                        &mut encoder,
                        slot,
                        tex,
                        fx,
                        t,
                        sizes,
                        effect_logical,
                    );
                    return (fx, masked, effect_logical);
                }
                let tex = masked.as_ref().unwrap_or(tex);
                let fx = self.apply_effects(
                    device,
                    queue,
                    &mut encoder,
                    slot,
                    tex,
                    fx,
                    t,
                    sizes,
                    effect_logical,
                );
                (fx.or(masked), None, effect_logical)
            })
            .collect();
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.width(), t.height()) != (w, h))
        {
            // The sRGB view lets `Preview::show_image` draw the target unchanged.
            let srgb = [FORMAT.add_srgb_suffix()];
            let tex = |usage, view_formats| {
                device.create_texture(&wgpu::TextureDescriptor {
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
                    usage,
                    view_formats,
                })
            };
            self.target = Some(tex(
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                &srgb,
            ));
            self.backdrop = Some(tex(
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                &[],
            ));
        }
        let target = self.target.as_ref().unwrap();
        let backdrop = self
            .backdrop
            .as_ref()
            .unwrap()
            .create_view(&Default::default());
        let frame = [size.0 as f32, size.1 as f32];
        // shortcut: a uniform and bind group per layer per frame; pool them if playback (#32) profiles hot.
        let binds: Vec<_> = layers
            .iter()
            .zip(&effected)
            .enumerate()
            .map(|(i, ((tex, t), (fx, cover, logical)))| {
                let cover = cover.as_ref().unwrap_or(&self.no_cover);
                (
                    fx.as_ref().unwrap_or(tex),
                    t,
                    self.premultiplied.get(i),
                    cover,
                    *logical,
                )
            })
            .rev()
            .filter(|(_, t, ..)| t.opacity > 0.0)
            .filter_map(|(tex, t, pre, cover, logical)| {
                let corners = quad(logical, frame, t)?;
                let mut p = corners.concat();
                let mode = Blend::ALL.iter().position(|b| *b == t.blend).unwrap_or(0);
                let pre = f32::from(u8::from(pre == Some(&true)));
                p.extend([t.opacity.clamp(0.0, 1.0) as f32, mode as f32, pre, 0.0]);
                let g = t.grade();
                let on = g.iter().any(|v| *v != 0.0) || !t.wheels.is_default();
                p.extend(g.map(|v| v as f32));
                let lut = t.lut.as_ref().and_then(|p| self.luts.get(p));
                let mix = lut.map_or(0.0, |_| t.lut_intensity.clamp(0.0, 1.0) as f32);
                let lut = lut.unwrap_or(&self.no_lut);
                p.extend([f32::from(u8::from(on)), mix, lut.width() as f32]);
                for w in t.wheels.all() {
                    p.extend(w.rgb().map(|v| v as f32));
                    p.push(0.0);
                }
                // shortcut: bakes and uploads a 2x256 table per curved layer per frame; cache by curves if it profiles hot.
                let curved = !t.curves.is_neutral();
                let table = curved
                    .then(|| curve_texture(device, queue, &t.curves.bake(), Curves::SIZE as u32));
                let table = table.as_ref().unwrap_or(&self.no_curves);
                p.extend([f32::from(u8::from(curved)), table.width() as f32, 0.0, 0.0]);
                let s = &t.secondary;
                let on = f32::from(u8::from(!s.is_neutral()));
                p.extend([s.hue, s.hue_width, s.softness].map(|v| v as f32));
                p.push(on);
                p.extend([s.sat, s.luma].as_flattened().iter().map(|v| *v as f32));
                p.extend([s.hue_shift, s.saturation, s.exposure].map(|v| v as f32));
                p.extend([
                    f32::from(u8::from(s.masked)),
                    self.before.clamp(0.0, 1.0) * w as f32,
                ]);
                p.extend([0.0; 3]);
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("layer"),
                    contents: &p.iter().flat_map(|f| f.to_ne_bytes()).collect::<Vec<_>>(),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(tex.format().remove_srgb_suffix()),
                    ..Default::default()
                });
                let lut = lut.create_view(&Default::default());
                let curves = table.create_view(&Default::default());
                let cover = cover.create_view(&Default::default());
                let blended = t.blend != Blend::Normal;
                let pipeline = if blended { &self.blend } else { &self.pipeline };
                let mut entries = vec![
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
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&lut),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&curves),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&cover),
                    },
                ];
                if blended {
                    entries.push(wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&backdrop),
                    });
                }
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                });
                Some((bind, blended))
            })
            .collect();
        let a = background[3] as f64 / 255.0;
        let c = |i: usize| background[i] as f64 / 255.0 * a;
        let view = target.create_view(&Default::default());
        let clear = wgpu::LoadOp::Clear(wgpu::Color {
            r: c(0),
            g: c(1),
            b: c(2),
            a,
        });
        // A new pass after each backdrop copy; normal layers share one.
        // shortcut: copies the whole frame per non-normal layer; copy just its bounds if it profiles hot.
        let mut rest = &binds[..];
        let (mut load, mut copied) = (clear, false);
        loop {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            // After a copy, the leading blended layer has its backdrop; before, none may draw.
            let lead = usize::from(copied);
            let n = lead + rest[lead..].iter().take_while(|(_, b)| !b).count();
            for (bind, blended) in &rest[..n] {
                pass.set_pipeline(if *blended {
                    &self.blend
                } else {
                    &self.pipeline
                });
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..4, 0..1);
            }
            drop(pass);
            rest = &rest[n..];
            if rest.is_empty() {
                break;
            }
            encoder.copy_texture_to_texture(
                target.as_image_copy(),
                self.backdrop.as_ref().unwrap().as_image_copy(),
                target.size(),
            );
            (load, copied) = (wgpu::LoadOp::Load, true);
        }
        queue.submit([encoder.finish()]);
        target
    }
}

/// Copies an RGBA8 texture back to the CPU as tight rows, blocking until the GPU is done.
pub fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let (buf, done) = copy_out(device, queue, tex, None, || {});
    map(device, &buf, done, tex.width())
}

/// Double-buffered readback for export: [`Readback::push`] queues a copy of one frame and returns
/// the frame before it, so the GPU renders and copies frame N while the CPU encodes frame N-1.
#[derive(Default)]
pub struct Readback {
    /// Copies in flight, oldest first, with their submission and the frame width.
    pending: std::collections::VecDeque<(wgpu::Buffer, wgpu::SubmissionIndex, u32)>,
    /// Buffers already read, for reuse.
    free: Vec<wgpu::Buffer>,
}

impl Readback {
    /// Queues a copy of `tex` (RGBA8); returns the previous frame's pixels, if any.
    pub fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tex: &wgpu::Texture,
    ) -> Option<Vec<u8>> {
        let (buf, done) = copy_out(device, queue, tex, self.free.pop(), || {});
        self.pending.push_back((buf, done, tex.width()));
        (self.pending.len() > 1)
            .then(|| self.take(device))
            .flatten()
    }

    /// The oldest queued frame, blocking until it's copied; `None` when nothing is queued.
    pub fn take(&mut self, device: &wgpu::Device) -> Option<Vec<u8>> {
        let (buf, done, width) = self.pending.pop_front()?;
        let rgba = map(device, &buf, done, width);
        buf.unmap();
        self.free.push(buf);
        Some(rgba)
    }
}

/// Submits a copy of `tex` into `reuse` when it's the right size, else a new buffer, and starts
/// mapping it, calling `mapped` once it has; returns the buffer and the copy's submission.
pub(crate) fn copy_out(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    reuse: Option<wgpu::Buffer>,
    mapped: impl FnOnce() + Send + 'static,
) -> (wgpu::Buffer, wgpu::SubmissionIndex) {
    assert_eq!(
        tex.format().block_copy_size(None),
        Some(4),
        "read_back expects 4 bytes/px, got {:?}",
        tex.format()
    );
    let row = (tex.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let size = (row * tex.height()) as u64;
    let buf = reuse.filter(|b| b.size() == size).unwrap_or_else(|| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
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
    let done = queue.submit([encoder.finish()]);
    // An Err means the buffer was dropped before the copy landed (export cancelled or app quit).
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| {
        if r.is_ok() {
            mapped();
        }
    });
    (buf, done)
}

/// Waits for the copy `done` into `buf` (not later work) and returns its rows without padding.
fn map(
    device: &wgpu::Device,
    buf: &wgpu::Buffer,
    done: wgpu::SubmissionIndex,
    width: u32,
) -> Vec<u8> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(done),
            timeout: None,
        })
        .unwrap();
    unpad(buf, width)
}

/// The rows of mapped `buf`, a copy of a `width`-wide RGBA8 texture, without padding.
pub(crate) fn unpad(buf: &wgpu::Buffer, width: u32) -> Vec<u8> {
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let data = buf.slice(..).get_mapped_range().unwrap();
    let mut out = Vec::with_capacity(data.len());
    for r in data.chunks(row as usize) {
        out.extend_from_slice(&r[..width as usize * 4]);
    }
    out
}

/// Composites one frame at full `size` (as [`Compositor::render`] at scale 1) and writes the
/// [`read_back`] pixels to `path` as an RGBA PNG, byte for byte.
pub fn export_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    background: [u8; 4],
    layers: &[(&wgpu::Texture, Transform)],
    path: &std::path::Path,
) -> image::ImageResult<()> {
    let mut comp = Compositor::new(device);
    let tex = comp.render(device, queue, size, 1, background, layers);
    let rgba = read_back(device, queue, tex);
    image::save_buffer_with_format(
        path,
        &rgba,
        size.0,
        size.1,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
}

#[cfg(test)]
pub(crate) mod tests;

/// `rows` of baked curve samples, `width` to a row, as an unfilterable float texture the shader
/// reads with `textureLoad`.
fn curve_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rows: &[[f32; 4]],
    width: u32,
) -> wgpu::Texture {
    let data: Vec<u8> = rows
        .iter()
        .flatten()
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let desc = curve_desc(width, rows.len() as u32 / width);
    device.create_texture_with_data(queue, &desc, Default::default(), &data)
}

fn curve_desc(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("curves"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }
}
