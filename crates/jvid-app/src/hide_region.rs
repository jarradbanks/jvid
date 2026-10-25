//! Hide Region: Duplicate's paste of a video clip onto the track above it, with Add Mask's
//! rectangle and Add Effect's Gaussian Blur, as one undo step. The copy is selected so its
//! mask can be placed on the Program view.

use jvid_core::history::{Command, Created, History};
use jvid_core::model::{ClipId, MaskShape, TrackKind};
use jvid_render::effect::Shader;

/// The blur's effect stem, a bundled example.
const BLUR: &str = "gaussian_blur";

fn id() -> egui::Id {
    egui::Id::new("hide region")
}

/// Asks the app to hide a region of `clip`, as the Inspector's button does.
pub fn request(ctx: &egui::Context, clip: ClipId) {
    ctx.data_mut(|d| d.insert_temp(id(), clip));
}

/// The clip a [`request`] named, once.
pub fn requested(ctx: &egui::Context) -> Option<ClipId> {
    ctx.data_mut(|d| d.remove_temp(id()))
}

/// The Blur shader from the effects folder, `None` when it doesn't load.
pub fn blur() -> Option<std::sync::Arc<Shader>> {
    crate::effects::with(|l| l.get(BLUR).and_then(|e| e.clone().ok()))
}

/// Hides a region of video clip `id` with `blur`, returning the copy; `None` changes nothing.
pub fn apply(history: &mut History, id: ClipId, blur: &Shader) -> Option<ClipId> {
    let project = history.project();
    let seq = project.sequence();
    let (track, c) = seq.clip(id)?;
    let video: Vec<_> = seq
        .tracks()
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .collect();
    let below = video.iter().position(|t| t.id == track)?;
    // The track above when it's free under the clip, so nothing there is overwritten.
    let above = below.checked_sub(1).map(|i| video[i]).filter(|t| {
        !t.locked
            && !t.hidden
            && t.clips()
                .iter()
                .all(|o| o.end() <= c.start || o.start >= c.end())
    });
    let mut copy = c.clone();
    copy.link = None;
    // Only the new mask, so the blur shows nowhere else.
    copy.transform.masks = vec![crate::inspector::new_mask(
        MaskShape::Rectangle,
        crate::masks::fitted(project, c),
    )];
    copy.keyframes
        .retain(|k, _| crate::inspector::mask_param(k).is_none());
    copy.transform
        .effects
        .push(crate::inspector::new_effect(BLUR, blur));
    copy.transform.drop_stale_mask_refs();
    let above = above.map(|t| t.id);
    let txn = history.begin();
    let created = (|| {
        let track = match above {
            Some(t) => t,
            None => {
                let add = Command::AddTrack {
                    kind: TrackKind::Video,
                    name: String::new(),
                };
                let Ok(Created::Track(t)) = history.apply(add) else {
                    return None;
                };
                let index = below;
                history.apply(Command::MoveTrack { id: t, index }).ok()?;
                t
            }
        };
        let at = copy.start;
        let paste = Command::Paste {
            clips: vec![(track, copy)],
            at,
        };
        match history.apply(paste) {
            Ok(Created::Clip(id)) => Some(id),
            _ => None,
        }
    })();
    if txn {
        match created {
            Some(_) => history.commit(),
            None => history.rollback(),
        }
    }
    created
}

#[cfg(test)]
mod tests {
    use eframe::egui_wgpu::wgpu;
    use jvid_core::model::{NewClip, Project, ProjectSettings};
    use jvid_core::time::Time;
    use jvid_render::composite::{self, Compositor};
    use wgpu::util::DeviceExt;

    use super::*;

    fn blur() -> Shader {
        let (_, text) = jvid_render::effect::BUNDLED
            .iter()
            .find(|(f, _)| *f == "gaussian_blur.wgsl")
            .unwrap();
        jvid_render::effect::parse(text).unwrap()
    }

    /// A 64x32 project with one solid clip on V1, and, with `busy`, a clip on V2 over it.
    fn project(busy: bool) -> (History, ClipId) {
        let mut h = History::new(Project::default());
        let settings = ProjectSettings {
            width: 64,
            height: 32,
            ..h.project().settings().clone()
        };
        h.apply(Command::SetSettings(settings)).unwrap();
        let mut clip = |name: &str| {
            let Ok(Created::Asset(asset)) = h.apply(Command::AddSolid {
                name: name.into(),
                color: [1.0; 4],
            }) else {
                panic!("solid")
            };
            let kind = TrackKind::Video;
            let Ok(Created::Track(track)) = h.apply(Command::AddTrack {
                kind,
                name: "".into(),
            }) else {
                panic!("track")
            };
            let clip = NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(1000),
                start: Time(0),
                link: None,
            };
            let Ok(Created::Clip(c)) = h.apply(Command::AddClip { track, clip }) else {
                panic!("clip")
            };
            c
        };
        let c = clip("A");
        if busy {
            clip("B");
        }
        (h, c)
    }

    #[test]
    fn hide_region_is_one_undo_step_onto_a_free_track_above() {
        for busy in [false, true] {
            let (mut h, c) = project(busy);
            let before = h.project().clone();
            let copy = apply(&mut h, c, &blur()).expect("hidden");
            let seq = h.project().sequence();
            let video: Vec<_> = seq.tracks().iter().map(|t| t.id).collect();
            let (above, top) = seq.clip(copy).unwrap();
            let (below, orig) = seq.clip(c).unwrap();
            let i = video.iter().position(|t| *t == below).unwrap();
            assert_eq!(video[i - 1], above, "busy {busy}: straight above");
            assert_eq!(
                video.len(),
                2 + busy as usize,
                "busy {busy}: one track added"
            );
            assert_eq!((top.start, top.end()), (orig.start, orig.end()));
            assert_eq!(top.transform.masks.len(), 1);
            assert_eq!(top.transform.masks[0].shape, MaskShape::Rectangle);
            assert_eq!(top.transform.effects[0].shader, BLUR);
            assert!(orig.transform.masks.is_empty() && orig.transform.effects.is_empty());
            h.undo();
            assert_eq!(h.project(), &before, "busy {busy}: one step");
        }
    }

    #[test]
    fn hide_region_blurs_only_inside_the_mask() {
        let Some((device, queue)) = crate::playback::tests::device() else {
            return;
        };
        let (mut h, c) = project(false);
        let copy = apply(&mut h, c, &blur()).unwrap();
        let seq = h.project().sequence();
        let (top, orig) = (seq.clip(copy).unwrap().1, seq.clip(c).unwrap().1);
        // A 4 px checkerboard stands in for the clip's picture.
        let (w, h) = (64u32, 32u32);
        let data: Vec<u8> = (0..w * h)
            .flat_map(|i| [((i % w / 4 + i / w / 4) % 2 * 255) as u8, 0, 0, 255])
            .collect();
        let src = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: composite::FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Default::default(),
            &data,
        );
        let mut comp = Compositor::new(&device);
        comp.set_effect(&device, BLUR, Some(&blur())).unwrap();
        let mut render = |layers: &[(&wgpu::Texture, _)]| {
            let tex = comp.render(&device, &queue, (w, h), 1, [0, 0, 0, 255], layers);
            composite::read_back(&device, &queue, tex)
        };
        let plain = render(&[(&src, orig.transform.clone())]);
        let out = render(&[
            (&src, top.transform.clone()),
            (&src, orig.transform.clone()),
        ]);
        let red = |rgba: &[u8], x: u32, y: u32| rgba[((y * w + x) * 4) as usize] as i32;
        // The mask is x 16..48, y 8..24; one pixel of antialiasing either side.
        for (x, y) in (0..w).flat_map(|x| (0..h).map(move |y| (x, y))) {
            if !(15..49).contains(&x) || !(7..25).contains(&y) {
                assert_eq!(red(&out, x, y), red(&plain, x, y), "outside at ({x}, {y})");
            }
        }
        // Well inside, the checkerboard averages towards mid red.
        for (x, y) in (24..40).flat_map(|x| (12..20).map(move |y| (x, y))) {
            let r = red(&out, x, y);
            assert!((r - 128).abs() < 40, "blurred at ({x}, {y}): {r}");
        }
    }
}
