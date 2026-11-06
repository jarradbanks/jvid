//! Program annotations: clip insertion, project-space endpoint editing and style controls.
//! All changes are ordinary history commands; viewer state never enters project content.

use std::collections::HashSet;

use egui::{Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use jvid_core::annotations::{Annotation, AnnotationKind, AnnotationStyle};
use jvid_core::history::{Command, GestureKey, History};
use jvid_core::keyframe::{Easing, Keyframe, Value};
use jvid_core::model::{Clip, ClipId, Edge, ModelError, Project, Transform};
use jvid_core::time::{FLICKS_PER_SECOND, Time};
use suite_ui::theme;
use suite_ui::themes::DARK;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{ColorField, DragField, KeyButton, Keyed};

use crate::i18n::{tr, verbatim};
use crate::timeline::Edit;
use crate::viewer_geometry::ViewerGeometry;

pub fn kind(action: &str) -> Option<AnnotationKind> {
    Some(match action {
        "add_arrow" => AnnotationKind::Arrow,
        "add_rectangle" => AnnotationKind::Rectangle,
        "add_callout" => AnnotationKind::Callout,
        _ => return None,
    })
}

pub fn create(history: &mut History, kind: AnnotationKind, at: Time) -> Result<ClipId, ModelError> {
    let name = match kind {
        AnnotationKind::Arrow => tr!("action.add_arrow"),
        AnnotationKind::Rectangle => tr!("action.add_rectangle"),
        AnnotationKind::Callout => tr!("action.add_callout"),
    };
    let size = history.project().settings();
    let w = f64::from(size.width);
    let h = f64::from(size.height);
    let number = history
        .project()
        .assets()
        .iter()
        .filter_map(|a| a.annotation.as_ref())
        .filter(|a| a.kind == AnnotationKind::Callout)
        .map(|a| a.number)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .min(9999);
    let annotation = Annotation {
        kind,
        number,
        points: match kind {
            AnnotationKind::Arrow => [[-w * 0.15, 0.0], [w * 0.15, 0.0]],
            _ => [[-w * 0.15, -h * 0.15], [w * 0.15, h * 0.15]],
        },
        ..Annotation::default()
    };
    let (command, clip) = jvid_core::annotations::create_command(
        history.project(),
        name.into(),
        annotation,
        at,
        Time(i64::from(jvid_core::settings::DEFAULT_STILL_SECS) * FLICKS_PER_SECOND),
    )?;
    history.apply(command)?;
    Ok(clip)
}

/// Source project point through the same fitted-frame transform as compositor::quad.
pub fn to_screen(g: ViewerGeometry, t: &Transform, p: [f64; 2]) -> Pos2 {
    let (sin, cos) = t.rotation.to_radians().sin_cos();
    let x = (p[0] - t.anchor[0]) * t.scale[0];
    let y = (p[1] - t.anchor[1]) * t.scale[1];
    g.project_to_screen(pos2(
        (t.position[0] + x * cos - y * sin) as f32,
        (t.position[1] + x * sin + y * cos) as f32,
    ))
}

pub fn from_screen(g: ViewerGeometry, t: &Transform, p: Pos2) -> Option<[f64; 2]> {
    if t.scale.iter().any(|s| !s.is_finite() || s.abs() < 1e-9) {
        return None;
    }
    let p = g.screen_to_project(p);
    let (x, y) = (
        f64::from(p.x) - t.position[0],
        f64::from(p.y) - t.position[1],
    );
    let (sin, cos) = t.rotation.to_radians().sin_cos();
    Some([
        (x * cos + y * sin) / t.scale[0] + t.anchor[0],
        (-x * sin + y * cos) / t.scale[1] + t.anchor[1],
    ])
}

fn geometry(project: &Project, body: Rect, frame: Rect) -> ViewerGeometry {
    let size = project.settings();
    ViewerGeometry::from_frame(body, frame, vec2(size.width as f32, size.height as f32))
}

/// Actual raster content, intersected with crop before transforming. Generic selection
/// handles and View Selection use this same box, including rotated/nonuniformly scaled clips.
pub fn corners(
    project: &Project,
    c: &Clip,
    x: &Transform,
    frame: Rect,
    at: Time,
) -> Option<[Pos2; 4]> {
    let a = project
        .asset(c.asset)?
        .annotation
        .as_ref()?
        .with(&c.annotation_keys(at));
    let size = project.settings();
    let [x0, y0, x1, y1] = jvid_render::annotations::drawn_bounds(&a, (size.width, size.height))?;
    let (w, h) = (f64::from(size.width), f64::from(size.height));
    let left = f64::from(x0).max(x.crop[0]);
    let top = f64::from(y0).max(x.crop[1]);
    let right = f64::from(x1).min(w - x.crop[2]);
    let bottom = f64::from(y1).min(h - x.crop[3]);
    if left >= right || top >= bottom {
        return None;
    }
    let g = geometry(project, frame, frame);
    Some(
        [[left, top], [right, top], [left, bottom], [right, bottom]]
            .map(|p| to_screen(g, x, [p[0] - w / 2.0, p[1] - h / 2.0])),
    )
}

pub fn hit(project: &Project, c: &Clip, t: Time, frame: Rect, pos: Pos2) -> bool {
    let Some(base) = project.asset(c.asset).and_then(|a| a.annotation.as_ref()) else {
        return false;
    };
    let x = c.transform_at(t);
    let g = geometry(project, frame, frame);
    let Some(p) = from_screen(g, &x, pos) else {
        return false;
    };
    let [w, h] = [
        f64::from(project.settings().width),
        f64::from(project.settings().height),
    ];
    let [left, top, right, bottom] = x.crop;
    if p[0] < -w / 2.0 + left
        || p[0] > w / 2.0 - right
        || p[1] < -h / 2.0 + top
        || p[1] > h / 2.0 - bottom
    {
        return false;
    }
    let scale = f64::from(g.scale()) * x.scale[0].abs().min(x.scale[1].abs());
    jvid_render::annotations::hit(&base.with(&c.annotation_keys(t)), p, 5.0 / scale.max(1e-9))
}

/// One annotation property edit. A single key represents a static per-clip override;
/// animation keys stay in source coordinates and use the existing clip source mapping.
fn property(c: &Clip, name: &str, value: Value, t: Time, animate: bool) -> Command {
    let mut keys = c.keyframes.clone();
    let list = keys.entry(name.into()).or_default();
    // One key is a static override. Opting into animation seeds a constant curve at
    // both visible source edges so later edits can key the playhead without ambiguity.
    if animate && list.len() < 2 && c.source_out.0 - c.source_in.0 > 1 {
        let held = list.first().map_or(value, |k| k.value);
        *list = [c.source_in, Time(c.source_out.0 - 1)]
            .map(|time| Keyframe {
                time,
                value: held,
                easing: Easing::Linear,
            })
            .to_vec();
    }
    let animated = animate || list.len() > 1;
    let time = if animated {
        c.source_at(t)
    } else {
        c.source_in
    };
    if !animated {
        list.clear();
    }
    match list.binary_search_by_key(&time, |k| k.time) {
        Ok(i) => list[i].value = value,
        Err(i) => list.insert(
            i,
            Keyframe {
                time,
                value,
                easing: Easing::Linear,
            },
        ),
    }
    Command::SetClipProps(vec![(c.id, c.transform.clone(), keys)])
}

fn gesture(ui: &Ui, r: &Response) -> Option<GestureKey> {
    let id = egui::Id::new("annotation drags");
    if r.drag_started() {
        ui.data_mut(|d| *d.get_temp_mut_or(id, 0u64) += 1);
    }
    let count = ui.data(|d| d.get_temp(id).unwrap_or(0u64));
    r.dragged()
        .then_some(0x616e_6e6f_0000 ^ r.id.value() ^ (count << 16))
}

/// Endpoint hit targets are 16 logical points at every DPI, zoom and clip scale.
/// Returns whether an endpoint owns the pointer, so generic transform handles can yield.
pub fn show(
    ui: &Ui,
    project: &Project,
    selected: &HashSet<ClipId>,
    at: Time,
    frame: Rect,
) -> (Option<Edit>, bool) {
    if selected.len() != 1
        || ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle) || i.modifiers.alt)
    {
        return (None, false);
    }
    let Some((track, c)) = selected
        .iter()
        .next()
        .and_then(|id| project.sequence().clip(*id))
    else {
        return (None, false);
    };
    if project.sequence().track(track).is_none_or(|t| t.locked)
        || !c.enabled
        || at < c.start
        || at >= c.end()
    {
        return (None, false);
    }
    let Some(base) = project.asset(c.asset).and_then(|a| a.annotation.as_ref()) else {
        return (None, false);
    };
    let annotation = base.with(&c.annotation_keys(at));
    let x = c.transform_at(at);
    let body = crate::preview::viewer_rect(ui);
    let g = geometry(project, body, frame);
    let mut edit = None;
    let mut busy = false;
    for (i, p) in annotation.points.into_iter().enumerate() {
        let centre = to_screen(g, &x, p);
        let rect = Rect::from_center_size(centre, vec2(16.0, 16.0)).intersect(body);
        let r = ui
            .interact(
                rect,
                ui.id().with(("annotation endpoint", c.id.0, i)),
                Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        r.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::DragValue,
                true,
                tr!("annotations.endpoint", point = i + 1),
            )
        });
        busy |= r.hovered() || r.dragged();
        let key = gesture(ui, &r);
        if r.dragged()
            && let Some(to) = r.interact_pointer_pos().and_then(|p| from_screen(g, &x, p))
        {
            edit = Some((
                property(c, Annotation::POINT_KEYS[i], Value::Vec2(to), at, false),
                key,
            ));
        }
        ui.painter().with_clip_rect(body).circle(
            centre,
            4.0,
            egui::Color32::BLACK,
            Stroke::new(1.0, DARK.light_blue),
        );
    }
    (edit, busy)
}

fn label(name: &str) -> &'static str {
    match name {
        "annotation_width" => tr!("annotations.width"),
        "annotation_arrow_head" => tr!("annotations.arrow_head"),
        "annotation_callout_radius" => tr!("annotations.radius"),
        "annotation_stroke_color" => tr!("annotations.stroke"),
        "annotation_fill_color" => tr!("annotations.fill"),
        "annotation_text_color" => tr!("annotations.text_color"),
        _ => tr!("annotations.number"),
    }
}

/// Narrow Inspector hook. Existing Video controls remain available for transform/effects.
pub fn inspector(ui: &mut Ui, clips: &[&Clip], base: &Annotation, at: Time) -> Option<Edit> {
    let first = *clips.first()?;
    let shown = base.with(&first.annotation_keys(at));
    ui.label(
        egui::RichText::new(tr!("annotations.title"))
            .font(theme::body_strong())
            .color(color::ink()),
    );
    let mut edit = None;
    let names = AnnotationStyle::KEYS
        .into_iter()
        .chain(["annotation_number"]);
    for name in names {
        if (name == "annotation_number"
            || name == "annotation_callout_radius"
            || name == "annotation_fill_color"
            || name == "annotation_text_color")
            && base.kind != AnnotationKind::Callout
        {
            continue;
        }
        if name == "annotation_arrow_head" && base.kind != AnnotationKind::Arrow {
            continue;
        }
        ui.push_id(name, |ui| {
            ui.horizontal(|ui| {
                let row_label = ui
                    .add_sized(
                        [size::INSPECTOR_LABEL, 24.0],
                        egui::Label::new(label(name)).truncate(),
                    )
                    .on_hover_text(label(name));
                let mut value = shown.get(name).expect("annotation field");
                let room = (ui.available_width() - 24.0 - space::S4).max(24.0);
                let r = match &mut value {
                    Value::Scalar(v) => ui.add(
                        DragField::new(v)
                            .range(if name == "annotation_number" {
                                1.0..=9999.0
                            } else {
                                0.1..=10_000.0
                            })
                            .width(room)
                            .decimals(0),
                    ),
                    Value::Color(v) => {
                        let mut rgba = jvid_render::solid::rgba8(*v);
                        let r = ui
                            .allocate_ui(vec2(room, 24.0), |ui| {
                                ui.add(ColorField::new(&mut rgba, label(name)))
                            })
                            .inner;
                        if r.changed() {
                            *v = rgba.map(|v| f64::from(v) / 255.0);
                        }
                        r
                    }
                    _ => unreachable!(),
                }
                .labelled_by(row_label.id);
                let g = gesture(ui, &r);
                if r.changed() {
                    edit = Some((
                        Command::Batch(
                            clips
                                .iter()
                                .map(|c| property(c, name, value, at, false))
                                .collect(),
                        ),
                        g,
                    ));
                }
                let keyed = match first.keyframes.get(name) {
                    Some(keys) if keys.iter().any(|k| k.time == first.source_at(at)) => Keyed::Here,
                    Some(keys) if keys.len() > 1 => Keyed::Animated,
                    _ => Keyed::Static,
                };
                if ui
                    .add(KeyButton::diamond(keyed, tr!("annotations.keyframe")))
                    .clicked()
                {
                    edit = Some((
                        Command::Batch(
                            clips
                                .iter()
                                .map(|c| property(c, name, value, at, true))
                                .collect(),
                        ),
                        None,
                    ));
                }
            });
        });
    }
    for (name, label) in [
        ("duration", tr!("annotations.duration")),
        ("entrance", tr!("annotations.entrance")),
        ("exit", tr!("annotations.exit")),
    ] {
        ui.push_id(name, |ui| {
            ui.horizontal(|ui| {
                let row_label = ui
                    .add_sized(
                        [size::INSPECTOR_LABEL, 24.0],
                        egui::Label::new(label).truncate(),
                    )
                    .on_hover_text(label);
                let duration = first.end() - first.start;
                let old = match name {
                    "entrance" => first.transform.fade_in,
                    "exit" => first.transform.fade_out,
                    _ => duration,
                };
                let mut seconds = old.0 as f64 / FLICKS_PER_SECOND as f64;
                let r = ui.add(
                    DragField::new(&mut seconds)
                        .range(if name == "duration" {
                            0.001..=86400.0
                        } else {
                            0.0..=duration.0 as f64 / FLICKS_PER_SECOND as f64
                        })
                        .speed(0.01)
                        .decimals(2)
                        .suffix(verbatim("s"))
                        .width(ui.available_width()),
                );
                let r = r.labelled_by(row_label.id);
                let g = gesture(ui, &r);
                if r.changed() {
                    let time = Time((seconds * FLICKS_PER_SECOND as f64).round() as i64);
                    let command = if name == "duration" {
                        Command::Trim {
                            trims: clips
                                .iter()
                                .map(|c| (c.id, Edge::Tail, time - (c.end() - c.start)))
                                .collect(),
                            ripple: false,
                        }
                    } else {
                        Command::SetClipProps(
                            clips
                                .iter()
                                .map(|c| {
                                    let mut x = c.transform.clone();
                                    if name == "entrance" {
                                        x.fade_in = time;
                                    } else {
                                        x.fade_out = time;
                                    }
                                    (c.id, x, c.keyframes.clone())
                                })
                                .collect(),
                        )
                    };
                    edit = Some((command, g));
                }
            });
        });
    }
    ui.add_space(space::S8);
    edit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer_geometry::View;

    #[test]
    fn rotated_scaled_placement_roundtrips_common_mapping() {
        let body = Rect::from_min_size(pos2(30.0, 70.0), vec2(480.0, 280.0));
        for dpi in [1.0, 1.5, 2.0] {
            for zoom in [None, Some(0.5), Some(2.0)] {
                let g = ViewerGeometry::new(
                    body,
                    vec2(1920.0, 1080.0),
                    zoom,
                    dpi,
                    View {
                        pan: vec2(-200.0, 100.0),
                        ..Default::default()
                    },
                );
                for rotation in [-135.0, 0.0, 37.0, 90.0] {
                    let x = Transform {
                        rotation,
                        scale: [0.7, -1.3],
                        position: [130.0, -45.0],
                        anchor: [-11.0, 23.0],
                        ..Default::default()
                    };
                    for p in [[-200.0, 70.0], [700.0, -300.0]] {
                        let back = from_screen(g, &x, to_screen(g, &x, p)).unwrap();
                        assert!((back[0] - p[0]).abs() < 0.002 && (back[1] - p[1]).abs() < 0.002);
                    }
                    let q = jvid_render::composite::quad([1920.0, 1080.0], [1920.0, 1080.0], &x)
                        .unwrap();
                    let screen = to_screen(g, &x, [-960.0, -540.0]);
                    let expected = g.frame.min
                        + vec2(
                            (q[0][0] + 1.0) / 2.0 * g.frame.width(),
                            (1.0 - q[0][1]) / 2.0 * g.frame.height(),
                        );
                    assert!(screen.distance(expected) < 0.001);
                }
            }
        }
    }

    #[test]
    fn style_edits_undo_reopen_and_program_export_are_identical() {
        use crate::playback::{DEFAULT_BUDGET, Gpu, Playback};
        use jvid_render::composite::read_back;
        let Some((device, queue)) = crate::playback::tests::device() else {
            return;
        };
        let mut history = History::new(Project::default());
        let mut settings = history.project().settings().clone();
        settings.width = 320;
        settings.height = 180;
        history.apply(Command::SetSettings(settings)).unwrap();
        let arrow = create(&mut history, AnnotationKind::Arrow, Time(0)).unwrap();
        let callout = create(&mut history, AnnotationKind::Callout, Time(0)).unwrap();
        let before = history.project().clone();
        let c = history.project().sequence().clip(arrow).unwrap().1;
        history
            .apply(property(
                c,
                "annotation_width",
                Value::Scalar(14.0),
                Time(0),
                false,
            ))
            .unwrap();
        let changed = history.project().clone();
        history.undo();
        assert_eq!(history.project(), &before);
        history.redo();
        assert_eq!(history.project(), &changed);
        let c = history.project().sequence().clip(callout).unwrap().1;
        let mut x = c.transform.clone();
        x.rotation = 31.0;
        x.scale = [0.8, 1.3];
        x.fade_in = Time(FLICKS_PER_SECOND);
        x.fade_out = Time(FLICKS_PER_SECOND);
        history
            .apply(Command::SetClipProps(vec![(
                callout,
                x,
                c.keyframes.clone(),
            )]))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("annotations.jvid");
        crate::project_file::save(history.project(), &file).unwrap();
        let reopened = crate::project_file::load(&file).unwrap();
        assert_eq!(&reopened, history.project());
        let mut playback = Playback::from_files(DEFAULT_BUDGET);
        playback.transport.duration = reopened.sequence().duration();
        let mut preview = Gpu::new(&device);
        let mut export = crate::export::FrameRenderer::new(&device);
        for time in [
            Time(0),
            Time(FLICKS_PER_SECOND / 2),
            Time(2 * FLICKS_PER_SECOND),
        ] {
            playback.transport.seek(time, 0);
            let shown = playback.park(&reopened).expect("generated frame ready");
            let preview = read_back(
                &device,
                &queue,
                preview.draw(&device, &queue, reopened.settings(), 1, &shown),
            );
            let exported = read_back(
                &device,
                &queue,
                export.render(&reopened, time, &device, &queue).unwrap(),
            );
            assert_eq!(preview, exported, "Program and video export at {time:?}");
        }
    }

    #[test]
    fn style_animation_is_opt_in_and_survives_reopen() {
        let mut h = History::new(Project::default());
        let id = create(&mut h, AnnotationKind::Arrow, Time(0)).unwrap();
        let c = h.project().sequence().clip(id).unwrap().1;
        h.apply(property(
            c,
            "annotation_width",
            Value::Scalar(10.0),
            Time(FLICKS_PER_SECOND),
            false,
        ))
        .unwrap();
        let c = h.project().sequence().clip(id).unwrap().1;
        assert_eq!(c.keyframes["annotation_width"].len(), 1);
        h.apply(property(
            c,
            "annotation_width",
            Value::Scalar(10.0),
            Time(0),
            true,
        ))
        .unwrap();
        let c = h.project().sequence().clip(id).unwrap().1;
        h.apply(property(
            c,
            "annotation_width",
            Value::Scalar(20.0),
            Time(2 * FLICKS_PER_SECOND),
            false,
        ))
        .unwrap();
        let c = h.project().sequence().clip(id).unwrap().1;
        assert_eq!(
            c.value_at("annotation_width", Time(FLICKS_PER_SECOND)),
            Some(Value::Scalar(15.0))
        );
        let json = jvid_core::file::to_json(h.project(), std::path::Path::new("/project")).unwrap();
        assert_eq!(
            jvid_core::file::from_json(&json, std::path::Path::new("/project"), |_| false).unwrap(),
            *h.project()
        );
    }

    struct Editor {
        preview: crate::preview::Preview,
        history: History,
        clip: ClipId,
        fresh: bool,
        frame: Rect,
    }

    impl eframe::App for Editor {
        fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
            if suite_ui::test_support::install_theme(ui, &mut self.fresh) {
                return;
            }
            egui::Panel::right("annotation inspector")
                .exact_size(240.0)
                .show(ui, |ui| {
                    let c = self.history.project().sequence().clip(self.clip).unwrap().1;
                    let base = self
                        .history
                        .project()
                        .asset(c.asset)
                        .unwrap()
                        .annotation
                        .as_ref()
                        .unwrap();
                    if let Some((command, gesture)) = inspector(ui, &[c], base, Time(0)) {
                        match gesture {
                            Some(g) => self.history.apply_coalesced(g, command),
                            None => self.history.apply(command),
                        }
                        .unwrap();
                    }
                });
            egui::CentralPanel::default().show(ui, |ui| {
                self.frame = self
                    .preview
                    .project_rect(ui, self.history.project().settings());
                self.preview.show_program(
                    ui,
                    self.history.project(),
                    Time(0),
                    None,
                    false,
                    &crate::keymap::Keymap::default(),
                );
                let (edit, _) = show(
                    ui,
                    self.history.project(),
                    &HashSet::from([self.clip]),
                    Time(0),
                    self.frame,
                );
                if let Some((command, gesture)) = edit {
                    match gesture {
                        Some(g) => self.history.apply_coalesced(g, command),
                        None => self.history.apply(command),
                    }
                    .unwrap();
                }
            });
        }
    }

    #[test]
    fn narrow_inspector_and_endpoint_drag_at_two_dpi_scales() {
        use egui_kittest::{Harness, kittest::Queryable};
        if !suite_ui::test_support::gpu() {
            return;
        }
        let previous_language = suite_ui::i18n::language();
        if let Ok(language) = std::env::var("JVID_N_UI_LANGUAGE") {
            suite_ui::i18n::set_language(&language);
        }
        for (dpi, kind) in [1.0, 1.5].into_iter().flat_map(|dpi| {
            [
                AnnotationKind::Arrow,
                AnnotationKind::Rectangle,
                AnnotationKind::Callout,
            ]
            .map(|kind| (dpi, kind))
        }) {
            let mut history = History::new(Project::default());
            let mut settings = history.project().settings().clone();
            settings.width = 640;
            settings.height = 360;
            history.apply(Command::SetSettings(settings)).unwrap();
            let clip = create(&mut history, kind, Time(0)).unwrap();
            let c = history.project().sequence().clip(clip).unwrap().1;
            let mut x = c.transform.clone();
            x.rotation = 27.0;
            x.scale = [0.8, 1.1];
            history
                .apply(Command::SetClipProps(vec![(clip, x, c.keyframes.clone())]))
                .unwrap();
            let mut h = Harness::builder()
                .with_size([900.0, 540.0])
                .with_pixels_per_point(dpi)
                .wgpu()
                .build_eframe(move |cc| Editor {
                    preview: crate::preview::Preview::new(cc.wgpu_render_state.as_ref()),
                    history,
                    clip,
                    fresh: true,
                    frame: Rect::NOTHING,
                });
            for _ in 0..3 {
                h.step();
                h.render().unwrap();
            }
            assert!(h.query_by_label(tr!("annotations.title")).is_some());
            let state = h.state();
            let c = state.history.project().sequence().clip(clip).unwrap().1;
            let base = state
                .history
                .project()
                .asset(c.asset)
                .unwrap()
                .annotation
                .as_ref()
                .unwrap();
            let g = geometry(state.history.project(), state.frame, state.frame);
            let from = to_screen(g, &c.transform_at(Time(0)), base.points[1]);
            let before = state.history.project().clone();
            h.hover_at(from);
            h.run();
            h.drag_at(from);
            h.run();
            for step in 1..=4 {
                h.hover_at(from + vec2(12.0, -6.0) * step as f32);
                h.run();
            }
            h.drop_at(from + vec2(48.0, -24.0));
            h.run();
            let after = h.state().history.project().clone();
            assert_ne!(after, before);
            h.state_mut().history.undo();
            assert_eq!(h.state().history.project(), &before, "one undo per drag");
            h.state_mut().history.redo();
            assert_eq!(h.state().history.project(), &after);
            if let Some(dir) = std::env::var_os("JVID_N_UI_ARTIFACTS") {
                std::fs::create_dir_all(&dir).unwrap();
                h.step();
                h.render()
                    .unwrap()
                    .save(
                        std::path::PathBuf::from(dir)
                            .join(format!("annotation-{kind:?}-{dpi}.png")),
                    )
                    .unwrap();
            }
        }
        suite_ui::i18n::set_language(previous_language);
    }
}
