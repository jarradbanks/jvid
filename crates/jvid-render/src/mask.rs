//! Masks: one pass over a masked layer's image, before its effects, that scales its alpha by
//! its [`Mask`]s' combined coverage. Layers without an enabled mask skip it.

use jvid_core::model::{Mask, MaskMode, MaskShape};
use wgpu::util::DeviceExt;

use crate::composite::FORMAT;

// shortcut: fixed uniform arrays, so masks past these are dropped; move the vertices to a
// storage buffer if real paths need more.
const MAX_MASKS: usize = 8;
const MAX_POINTS: usize = 512;
/// Floats in the uniform: `info`, then 3 vec4s per mask, then 2 vertices per vec4.
const FLOATS: usize = 4 + MAX_MASKS * 12 + MAX_POINTS * 2;
/// Straight segments per curved bezier span, at most.
pub const STEPS: usize = 16;

/// A closed bezier path (see [`Mask::points`]) as polygon vertices, `steps` per curved span;
/// straight spans add only their start.
pub fn flatten(p: &[[f64; 2]], steps: usize) -> Vec<[f32; 2]> {
    let n = p.len() / 3;
    let add = |a: [f64; 2], b: [f64; 2]| [a[0] + b[0], a[1] + b[1]];
    let mut out = Vec::new();
    for k in 0..n {
        let (a, out_handle) = (p[3 * k], p[3 * k + 2]);
        let j = (k + 1) % n;
        let (b, in_handle) = (p[3 * j], p[3 * j + 1]);
        let (c1, c2) = (add(a, out_handle), add(b, in_handle));
        let steps = match out_handle == [0.0; 2] && in_handle == [0.0; 2] {
            true => 1,
            false => steps,
        };
        for s in 0..steps {
            let t = s as f64 / steps as f64;
            let r = 1.0 - t;
            let w = [r * r * r, 3.0 * r * r * t, 3.0 * r * t * t, t * t * t];
            let at = |i: usize| w[0] * a[i] + w[1] * c1[i] + w[2] * c2[i] + w[3] * b[i];
            out.push([at(0) as f32, at(1) as f32]);
        }
    }
    out
}

/// The uniform for the enabled of `masks` over an image fitted to `fitted` pixels, `aa`
/// fitted pixels per output pixel; `None` when none of them draws.
fn uniform(masks: &[Mask], fitted: [f32; 2], aa: f32) -> Option<Vec<f32>> {
    let mut u = vec![0.0; FLOATS];
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut n = 0;
    for m in masks.iter().filter(|m| m.enabled) {
        if n == MAX_MASKS {
            break;
        }
        let (shape, rect, first, count) = match (m.shape, &m.points[..]) {
            (MaskShape::Bezier, p) => {
                // A long path shares out what's left of the budget, less smooth but drawn.
                let spans = p.len() / 3;
                let left = MAX_POINTS - points.len();
                if spans == 0 || spans > left {
                    continue;
                }
                let flat = flatten(p, STEPS.min(left / spans));
                if flat.len() < 3 {
                    continue;
                }
                let first = points.len();
                points.extend(&flat);
                (2.0, [0.0; 4], first, flat.len())
            }
            (s, [c, size, ..]) => {
                let half = size.map(|v| (v.abs() / 2.0) as f32);
                let rect = [c[0] as f32, c[1] as f32, half[0], half[1]];
                (f32::from(s == MaskShape::Ellipse), rect, 0, 0)
            }
            _ => continue,
        };
        let head = [
            shape,
            first as f32,
            count as f32,
            m.feather.max(0.0) as f32,
            m.opacity.clamp(0.0, 1.0) as f32,
            f32::from(m.invert),
            f32::from(m.mode == MaskMode::Subtract),
            0.0,
        ];
        let at = 4 + n * 12;
        u[at..at + 8].copy_from_slice(&head);
        u[at + 8..at + 12].copy_from_slice(&rect);
        n += 1;
    }
    if n == 0 {
        return None;
    }
    u[..4].copy_from_slice(&[fitted[0], fitted[1], n as f32, aa]);
    let at = 4 + MAX_MASKS * 12;
    u[at..at + points.len() * 2].copy_from_slice(points.as_flattened());
    Some(u)
}

/// GPU state for masking: the pass, and per layer index a masked image and its uniform,
/// reused across frames.
pub(crate) struct Masker {
    pipeline: wgpu::RenderPipeline,
    /// Mixes an effect's output into its input by a mask's coverage; see [`Masker::mix`].
    mix: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    pool: Vec<Option<(wgpu::Texture, wgpu::Buffer)>>,
    /// Masked in place of an image for coverage alone; made on first use.
    white: Option<wgpu::Texture>,
}

impl Masker {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mask"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mask.wgsl").into()),
        });
        let pipeline = |entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mask"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_mask"),
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
                    targets: &[Some(FORMAT.into())],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            pipeline: pipeline("fs_mask"),
            mix: pipeline("fs_mix"),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            pool: Vec::new(),
            white: None,
        }
    }

    /// `tex` with `masks` applied into `encoder`, or `None` when none draws. The image fits a
    /// `frame`-sized project drawn `target` pixels big, with `fitted` display pixels as its
    /// mask coordinate system. Texture dimensions only determine sampling/allocation; SAR,
    /// source rotation and reduced decoding must not change the stored geometry. A smaller
    /// image is masked at its drawn size so edges stay sharp. `slot` picks the pooled
    /// texture and uniform. With `cover`, the output is the coverage alone, in alpha over white.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        slot: usize,
        tex: &wgpu::Texture,
        masks: &[Mask],
        (fitted, frame, target): ([f32; 2], [f32; 2], [f32; 2]),
        cover: bool,
    ) -> Option<wgpu::Texture> {
        if !masks.iter().any(|m| m.enabled) {
            return None;
        }
        let image = [tex.width() as f32, tex.height() as f32];
        let k = (fitted[0] * target[0] / frame[0] / image[0])
            .max(fitted[1] * target[1] / frame[1] / image[1])
            .max(1.0);
        let (w, h) = ((image[0] * k).round() as u32, (image[1] * k).round() as u32);
        // Fitted pixels per output pixel: the mask texture's, or the drawn frame's when that's
        // coarser (a half-size preview, or a 4K clip in a 1080 project).
        let aa = (fitted[0] / w as f32)
            .max(fitted[1] / h as f32)
            .max(frame[0] / target[0])
            .max(frame[1] / target[1]);
        let data = uniform(masks, fitted, aa)?;
        if self.pool.len() <= slot {
            self.pool.resize_with(slot + 1, || None);
        }
        let (out, buf) = match &mut self.pool[slot] {
            Some((t, b)) if (t.width(), t.height()) == (w, h) => (t, b),
            entry => {
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("mask"),
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
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let b = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("mask"),
                    size: (FLOATS * 4) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let (t, b) = entry.insert((t, b));
                (t, b)
            }
        };
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_ne_bytes()).collect();
        queue.write_buffer(buf, 0, &bytes);
        let src = match cover {
            true => self.white.get_or_insert_with(|| {
                let desc = wgpu::TextureDescriptor {
                    label: Some("white"),
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
                };
                device.create_texture_with_data(queue, &desc, Default::default(), &[255; 4])
            }),
            false => tex,
        };
        let view = src.create_view(&wgpu::TextureViewDescriptor {
            format: Some(src.format().remove_srgb_suffix()),
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                    resource: buf.as_entire_binding(),
                },
            ],
        });
        let target = out.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mask"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..4, 0..1);
        drop(pass);
        Some(out.clone())
    }

    /// Writes `fx` where `cover` (from [`Masker::apply`] with `cover`) covers and `orig`
    /// elsewhere into `out`; all but `cover` are the same size.
    pub fn mix(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        [orig, fx, cover]: [&wgpu::Texture; 3],
        out: &wgpu::Texture,
    ) {
        let view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                format: Some(t.format().remove_srgb_suffix()),
                ..Default::default()
            })
        };
        let views = [fx, orig, cover].map(view);
        let entry = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.mix.get_bind_group_layout(0),
            entries: &[
                entry(0, &views[0]),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                entry(3, &views[1]),
                entry(4, &views[2]),
            ],
        });
        let target = out.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mask mix"),
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
        pass.set_pipeline(&self.mix);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_packs_enabled_masks_and_flattens_paths() {
        let rect = Mask::new(MaskShape::Rectangle, vec![[10.0, -20.0], [-100.0, 50.0]]);
        let off = Mask {
            enabled: false,
            ..rect.clone()
        };
        assert_eq!(uniform(std::slice::from_ref(&off), [1.0; 2], 1.0), None);
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut path: Vec<[f64; 2]> = square
            .iter()
            .flat_map(|v| [*v, [0.0; 2], [0.0; 2]])
            .collect();
        let straight = Mask {
            mode: MaskMode::Subtract,
            ..Mask::new(MaskShape::Bezier, path.clone())
        };
        let u = uniform(&[off, rect, straight], [1920.0, 1080.0], 2.0).unwrap();
        assert_eq!(u[..4], [1920.0, 1080.0, 2.0, 2.0]);
        // The rectangle: half its size, however it was dragged.
        assert_eq!(
            u[4..16],
            [
                0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 10.0, -20.0, 50.0, 25.0
            ]
        );
        // A straight path is its vertices; it subtracts.
        assert_eq!(u[16..24], [2.0, 0.0, 4.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        let at = 4 + MAX_MASKS * 12;
        assert_eq!(u[at..at + 8], [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0]);
        // A curved span flattens into STEPS points that start at its vertex and stay on it.
        path[2] = [5.0, -5.0];
        let flat = flatten(&path, STEPS);
        assert_eq!(flat.len(), 3 + STEPS);
        assert_eq!(flat[0], [0.0, 0.0]);
        assert_eq!(flat[STEPS], [10.0, 0.0]);
        assert!(flat[1..STEPS].iter().all(|p| p[1] < 0.0), "bulges up");
    }

    #[test]
    fn a_path_past_the_vertex_budget_masks_with_fewer_steps() {
        // 64 curved spans would flatten to 1024 vertices at full smoothness.
        let path: Vec<[f64; 2]> = (0..64)
            .flat_map(|k| {
                let (sin, cos) = (k as f64 / 64.0 * std::f64::consts::TAU).sin_cos();
                [
                    [cos * 100.0, sin * 100.0],
                    [sin * 5.0, -cos * 5.0],
                    [-sin * 5.0, cos * 5.0],
                ]
            })
            .collect();
        let m = Mask::new(MaskShape::Bezier, path);
        let u = uniform(&[m], [1920.0, 1080.0], 1.0).unwrap();
        assert_eq!(u[2], 1.0, "the path still masks");
        assert_eq!(u[4..7], [2.0, 0.0, (64 * (MAX_POINTS / 64)) as f32]);
    }
}
