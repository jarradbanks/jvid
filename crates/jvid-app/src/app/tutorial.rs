//! Interactive lessons in Help > Tutorials. A lesson is a list of steps, each pointing at an
//! [`anchor`] the editor draws, checked against the project or UI state, and able to do itself
//! (Show Me) through the normal undoable edits. It runs in a sandbox project; the user's
//! project, undo history and path wait in a [`Stash`] until the lesson ends.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui::{
    Align, Align2, CornerRadius, Id, Key, LayerId, Layout, Modifiers, Order, Rect, Stroke, pos2,
    vec2,
};
use jvid_core::history::{Command, History};
use jvid_core::model::{AssetId, Project};
use jvid_core::time::Time;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, State};

use super::{App, Tab};
use crate::i18n::tr;
use crate::import::Importer;
use crate::timeline::Timeline;

/// One lesson. Its `id` is the progress key in `settings.toml`.
pub struct Lesson {
    pub id: &'static str,
    pub title: fn() -> &'static str,
    pub minutes: u32,
    /// The sample clips the sandbox project imports, by file name.
    pub media: &'static [&'static str],
    /// Prepares the starting project once `media` has imported. It can't be undone.
    pub setup: fn(&mut App),
    pub steps: &'static [Step],
}

pub struct Step {
    /// The [`anchor`] the step points at.
    pub anchor: &'static str,
    /// The instruction, one or two short sentences. Shortcuts come from `app.keymap`, so a
    /// remapped key reads right.
    pub text: fn(&App) -> String,
    /// Whether the step's goal holds in the project or UI. Checked every frame; once it passes,
    /// the lesson moves on.
    pub done: fn(&App) -> bool,
    /// Does the step the way the user would.
    pub show_me: fn(&mut App),
    /// Brings the anchor into view when it isn't drawn, such as by switching tabs. The Render
    /// Queue reveals itself.
    pub reveal: Option<fn(&mut App)>,
}

pub const LESSONS: &[Lesson] = &[BASICS];

/// A short test of the engine on a title clip; the real lessons come with the sample media.
const BASICS: Lesson = Lesson {
    id: "basics",
    title: || tr!("tutorial.basics"),
    minutes: 1,
    media: &[],
    setup: |_| {},
    steps: &[
        Step {
            anchor: "bin.titles",
            text: |_| tr!("tutorial.basics_add").into(),
            done: |a| {
                a.history
                    .project()
                    .assets()
                    .iter()
                    .any(|a| a.text.is_some())
            },
            show_me: |a| {
                let [(name, _, text), ..] = crate::media_bin::presets();
                let name = name.into();
                apply(a, Command::AddText { name, text });
                // Where clicking a preset leaves the bin: the new title selected on the Media tab.
                a.bin.selected = a.history.project().assets().last().map(|a| a.id);
                (a.bin.tab, a.bin.query) = (0, String::new());
            },
            reveal: Some(|a| a.bin.tab = 2),
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.basics_place").into(),
            done: |a| first_clip(a).is_some(),
            show_me: |a| {
                let project = a.history.project();
                let Some(asset) = project.assets().iter().find(|a| a.text.is_some()) else {
                    return;
                };
                if let Some(p) = project.placement(asset.id, Time(0), false, None, false) {
                    apply(a, Command::Place(p));
                }
            },
            reveal: None,
        },
        Step {
            anchor: "timeline",
            text: |_| tr!("tutorial.basics_select").into(),
            done: |a| !a.timeline.selected().is_empty(),
            show_me: |a| {
                if let Some(id) = first_clip(a) {
                    a.timeline.select_clip(a.history.project().sequence(), id);
                }
            },
            reveal: None,
        },
        Step {
            anchor: "inspector",
            text: |_| tr!("tutorial.basics_edit").into(),
            done: |a| {
                let preset = &crate::media_bin::presets()[0].2.content;
                let texts = a.history.project().assets().iter();
                texts
                    .filter_map(|a| a.text.as_ref())
                    .any(|t| t.content != *preset)
            },
            show_me: |a| {
                let project = a.history.project();
                let Some(asset) = project.assets().iter().find(|a| a.text.is_some()) else {
                    return;
                };
                let mut text = asset.text.clone().unwrap_or_default();
                text.content = tr!("tutorial.basics_text").into();
                apply(a, Command::SetText { id: asset.id, text });
            },
            reveal: None,
        },
    ],
};

/// Sample clip `name`'s path, written out first if needed.
fn sample(name: &str) -> std::io::Result<PathBuf> {
    crate::samples::path(name)?.ok_or_else(|| std::io::ErrorKind::NotFound.into())
}

fn apply(app: &mut App, c: Command) {
    if let Err(e) = app.history.apply(c) {
        tracing::warn!("lesson edit failed: {e}");
    }
}

fn first_clip(app: &App) -> Option<jvid_core::model::ClipId> {
    let seq = app.history.project().sequence();
    seq.tracks()
        .iter()
        .find_map(|t| t.clips().first().map(|c| c.id))
}

/// Turned on by a running lesson, so [`anchor`] records nothing otherwise.
const RUNNING: &str = "lesson_running";

fn running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(Id::new(RUNNING)).unwrap_or(false))
}

/// Records where anchor `name` was drawn this frame, for a lesson step that points at it.
pub fn anchor(ctx: &egui::Context, name: &'static str, rect: Rect) {
    if !running(ctx) {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let a = d.get_temp_mut_or_default::<Anchors>(Id::NULL);
        if a.pass != pass {
            *a = Anchors {
                pass,
                rects: HashMap::new(),
            };
        }
        a.rects.insert(name, rect);
    });
}

/// Where anchor `name` was drawn this frame, if it was.
pub fn anchor_rect(ctx: &egui::Context, name: &str) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| {
        let a = d.get_temp::<Anchors>(Id::NULL)?;
        (a.pass == pass).then(|| a.rects.get(name).copied())?
    })
}

#[derive(Clone, Default)]
struct Anchors {
    pass: u64,
    rects: HashMap<&'static str, Rect>,
}

/// The lesson running, and the user's project it set aside.
pub struct Run {
    pub lesson: &'static Lesson,
    /// The current step; `steps.len()` once finished.
    pub step: usize,
    /// A passing check moves on. Off after Back, so the step stays until its check fails.
    armed: bool,
    /// The lesson's `setup` ran; the steps wait for it.
    set_up: bool,
    /// The current step's reveal ran. Once per visit, so a missing anchor can't repaint every
    /// frame.
    revealed: bool,
    /// The current step's missing anchor was logged.
    warned: bool,
    /// A step passed its check rather than being skipped.
    completed: bool,
    stash: Stash,
}

impl Run {
    fn go(&mut self, step: usize) {
        self.step = step.min(self.lesson.steps.len());
        (self.revealed, self.warned) = (false, false);
    }
}

struct Stash {
    history: History,
    path: Option<PathBuf>,
    missing: HashSet<PathBuf>,
    match_source: bool,
    importer: Importer,
    /// The playhead, selection and clipboard: clip ids repeat across projects.
    timeline: Timeline,
    bin_tab: usize,
    bin_selected: Option<AssetId>,
}

#[derive(Clone, Copy)]
enum Click {
    Back,
    ShowMe,
    Skip,
    Keep,
    Exit,
}

impl App {
    /// Opens `lesson` in a sandbox project built from its media, setting the user's aside.
    /// Shows an error and starts nothing if a clip can't be written out.
    pub(super) fn start_lesson(&mut self, lesson: &'static Lesson) {
        let media: std::io::Result<Vec<_>> = lesson.media.iter().map(|n| sample(n)).collect();
        let media = match media {
            Ok(m) => m,
            Err(e) => {
                let path = lesson.media.join(", ");
                return self.error(tr!("app.open_failed", path = path, error = e));
            }
        };
        self.end_lesson();
        self.leave_project();
        // Autosave is off in a lesson, so the edits it hasn't written yet are written now.
        if let Some(target) = self.autosave_target() {
            self.autosave.flush(&self.history, &target);
        }
        let (importer, timeline) = (self.importer.sibling(), self.timeline.sibling());
        let stash = Stash {
            history: std::mem::replace(&mut self.history, History::new(Project::default())),
            path: self.path.take(),
            missing: std::mem::take(&mut self.missing),
            match_source: std::mem::replace(&mut self.match_source, true),
            importer: std::mem::replace(&mut self.importer, importer),
            timeline: std::mem::replace(&mut self.timeline, timeline),
            bin_tab: self.bin.tab,
            bin_selected: self.bin.selected.take(),
        };
        self.import(media);
        self.lesson = Some(Run {
            lesson,
            step: 0,
            armed: true,
            set_up: false,
            revealed: false,
            warned: false,
            completed: false,
            stash,
        });
    }

    /// Ends the lesson, if one runs, and puts the user's project back as it was.
    pub(super) fn end_lesson(&mut self) {
        let Some(run) = self.lesson.take() else {
            return;
        };
        self.leave_project();
        // The lesson's playback would run on into the user's timeline.
        if self.preview.playing() {
            self.preview.program_action("shuttle_stop");
        }
        let s = run.stash;
        self.history = s.history;
        self.path = s.path;
        self.missing = s.missing;
        self.match_source = s.match_source;
        self.importer = s.importer;
        self.timeline = s.timeline;
        self.bin.tab = s.bin_tab;
        self.bin.selected = s.bin_selected;
    }

    /// Esc exits the lesson unless a dialog, menu or text field would take it. Runs before
    /// anything else this frame, so it sees what was open last frame.
    pub(super) fn lesson_keys(&mut self, ctx: &egui::Context) {
        if self.lesson.is_none() {
            return;
        }
        let free = ctx.memory(|m| m.top_modal_layer().is_none() && m.focused().is_none())
            && !egui::Popup::is_any_open(ctx);
        if free && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.end_lesson();
        }
    }

    /// Runs the lesson a frame, after the panels drew their anchors: moves on when the step's
    /// check passes, reveals a hidden anchor, and draws the overlay and step card.
    pub(super) fn lesson_frame(&mut self, ctx: &egui::Context) {
        let recorded = running(ctx);
        if recorded != self.lesson.is_some() {
            ctx.data_mut(|d| d.insert_temp(Id::new(RUNNING), self.lesson.is_some()));
            ctx.request_repaint();
        }
        // Without this frame's anchors, a step would look hidden.
        if !recorded {
            return;
        }
        let Some(mut run) = self.lesson.take() else {
            return;
        };
        if !run.set_up {
            if self.importer.is_busy() {
                self.lesson = Some(run);
                return;
            }
            (run.lesson.setup)(self);
            // The starting project isn't an edit: nothing to undo or save.
            self.history = History::new(self.history.project().clone());
            run.set_up = true;
        }
        let steps = run.lesson.steps;
        if let Some(step) = steps.get(run.step) {
            let done = (step.done)(self);
            if done && run.armed {
                run.completed = true;
                self.next_step(&mut run);
                ctx.request_repaint();
            }
            run.armed |= !done;
        }
        let step = steps.get(run.step);
        let target = step.and_then(|s| anchor_rect(ctx, s.anchor));
        if let Some(s) = step.filter(|_| target.is_none()) {
            let queue =
                (s.anchor == Tab::RenderQueue.anchor()).then_some(App::show_queue as fn(&mut App));
            match s.reveal.or(queue) {
                Some(reveal) if !run.revealed => {
                    run.revealed = true;
                    reveal(self);
                    ctx.request_repaint();
                }
                _ if !run.warned => {
                    run.warned = true;
                    let (id, n, anchor) = (run.lesson.id, run.step, s.anchor);
                    tracing::warn!("lesson {id} step {n}: anchor `{anchor}` isn't drawn");
                }
                _ => {}
            }
        }
        if let Some(t) = target {
            dim(ctx, t);
        }
        let text = step.map_or_else(|| tr!("tutorial.complete").into(), |s| (s.text)(self));
        let modal = ctx.memory(|m| m.top_modal_layer().is_some());
        let Some(click) = card(ctx, &run, &text, target, !modal) else {
            self.lesson = Some(run);
            return;
        };
        ctx.request_repaint();
        match click {
            Click::Back => {
                run.go(run.step.saturating_sub(1));
                run.armed = false;
            }
            Click::ShowMe => {
                if let Some(s) = step {
                    (s.show_me)(self);
                }
            }
            Click::Skip => self.next_step(&mut run),
            Click::Keep => {
                self.save(true);
            }
            Click::Exit => {
                self.lesson = Some(run);
                return self.end_lesson();
            }
        }
        self.lesson = Some(run);
    }

    fn next_step(&mut self, run: &mut Run) {
        run.go(run.step + 1);
        run.armed = true;
        // Skipping every step isn't finishing the lesson.
        if run.step == run.lesson.steps.len() && run.completed {
            self.lesson_done(run.lesson.id);
        }
    }

    /// Marks lesson `id` done in Help > Tutorials and saves it to `settings.toml`.
    fn lesson_done(&mut self, id: &str) {
        if self.tutorials_done.iter().any(|d| d == id) {
            return;
        }
        self.tutorials_done.push(id.into());
        let Some(path) = &self.settings_path else {
            return;
        };
        let done = self.tutorials_done.iter().map(|d| d.as_str().into());
        let value = toml::Value::Array(done.collect());
        if let Err(e) = crate::settings_file::save_value_to(path, "tutorials_done", value) {
            tracing::warn!("couldn't save tutorial progress: {e}");
        }
    }

    /// The Help menu's Tutorials rows: each lesson with its length, and a check once done.
    pub(super) fn tutorials_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(tr!("menu.tutorials"), |ui| {
            for lesson in LESSONS {
                let done = self.tutorials_done.iter().any(|d| d == lesson.id);
                let check = if done {
                    color::ink_2()
                } else {
                    egui::Color32::TRANSPARENT
                };
                let mark = egui::RichText::new(theme::icon::CHECK)
                    .font(theme::icon_font())
                    .color(check);
                let row = egui::Button::new((mark, (lesson.title)()))
                    .shortcut_text(tr!("tutorial.minutes", count = lesson.minutes));
                if ui.add(row).clicked() {
                    self.start_lesson(lesson);
                }
            }
        });
    }
}

/// Over the panels, under windows, menus and the step card.
const DIM: Order = Order::Background;

/// Dims the window around `target`, outlined in Light Blue. Painted only, so every click
/// still reaches the editor.
fn dim(ctx: &egui::Context, target: Rect) {
    let screen = ctx.content_rect();
    let hole = target.expand(space::S4).intersect(screen);
    let painter = ctx.layer_painter(LayerId::new(DIM, Id::new("lesson_dim")));
    let shade = color::ground().gamma_multiply(0.8);
    for r in [
        Rect::from_min_max(screen.min, pos2(screen.max.x, hole.top())),
        Rect::from_min_max(pos2(screen.min.x, hole.bottom()), screen.max),
        Rect::from_min_max(pos2(screen.min.x, hole.top()), hole.left_bottom()),
        Rect::from_min_max(hole.right_top(), pos2(screen.max.x, hole.bottom())),
    ] {
        painter.rect_filled(r, 0, shade);
    }
    painter.rect_stroke(
        hole,
        CornerRadius::same(radius::PANEL),
        Stroke::new(2.0, color::light_blue()),
        egui::StrokeKind::Inside,
    );
}

const CARD_W: f32 = 320.0;
/// Roughly a card's height, to tell whether it fits above or below the target.
const CARD_H: f32 = 140.0;

/// The step card beside `target`, or centred when there is none: the lesson and step, the
/// instruction `text` and the buttons. Returns the button clicked, never while not `live`.
fn card(
    ctx: &egui::Context,
    run: &Run,
    text: &str,
    target: Option<Rect>,
    live: bool,
) -> Option<Click> {
    let screen = ctx.content_rect().shrink(space::S16);
    let gap = space::S12;
    let (at, pivot) = match target {
        Some(t) if t.right() + gap + CARD_W <= screen.right() => {
            (pos2(t.right() + gap, t.top()), Align2::LEFT_TOP)
        }
        Some(t) if t.left() - gap - CARD_W >= screen.left() => {
            (pos2(t.left() - gap, t.top()), Align2::RIGHT_TOP)
        }
        Some(t) if t.top() - gap - CARD_H >= screen.top() => {
            (pos2(t.center().x, t.top() - gap), Align2::CENTER_BOTTOM)
        }
        Some(t) if t.bottom() + gap + CARD_H <= screen.bottom() => {
            (pos2(t.center().x, t.bottom() + gap), Align2::CENTER_TOP)
        }
        _ => (screen.center(), Align2::CENTER_CENTER),
    };
    let steps = run.lesson.steps;
    let step = steps.get(run.step);
    let mut click = None;
    egui::Area::new(Id::new("lesson_card"))
        .order(Order::Foreground)
        .interactable(live)
        .fixed_pos(at.max(screen.min))
        .pivot(pivot)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(color::raised())
                .inner_margin(space::S12)
                .show(ui, |ui| {
                    ui.set_width(CARD_W);
                    ui.horizontal(|ui| {
                        let title = egui::RichText::new((run.lesson.title)())
                            .font(theme::body_strong())
                            .color(color::ink());
                        ui.label(title);
                        if step.is_some() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let n = tr!("tutorial.step", n = run.step + 1, total = steps.len());
                                let n = egui::RichText::new(n)
                                    .font(theme::small())
                                    .color(color::ink_3());
                                ui.label(n);
                            });
                        }
                    });
                    ui.add_space(space::S8);
                    let text = egui::RichText::new(text)
                        .font(theme::body())
                        .color(color::ink());
                    ui.add(egui::Label::new(text).wrap());
                    ui.add_space(space::S12);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(space::S4, space::S4);
                        let mut button = |b: Button<'_>, c: Click| {
                            if ui.add(b).clicked() {
                                click = Some(c);
                            }
                        };
                        if step.is_some() {
                            button(Button::secondary(tr!("tutorial.show_me")), Click::ShowMe);
                            let back = if run.step == 0 {
                                State::Disabled
                            } else {
                                State::Live
                            };
                            button(Button::ghost(tr!("tutorial.back")).state(back), Click::Back);
                            button(Button::ghost(tr!("tutorial.skip_step")), Click::Skip);
                        } else {
                            button(Button::secondary(tr!("tutorial.keep_project")), Click::Keep);
                        }
                        button(Button::ghost(tr!("tutorial.exit")), Click::Exit);
                    });
                });
        });
    click
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// A fresh temp folder for test `tag`.
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jvid-tutorial-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn harness() -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1280.0, 720.0])
            .build_eframe(|cc| App::new(cc, &Default::default()));
        let state = h.state_mut();
        state.on_welcome = false;
        state.settings_path = None;
        state.untitled = None;
        h.run();
        h
    }

    fn step(h: &Harness<'_, App>) -> Option<usize> {
        h.state().lesson.as_ref().map(|r| r.step)
    }

    /// Prepared by `setup` with an extra video track; its first step points at the closed
    /// Render Queue and its second at nothing.
    const PREPARED: Lesson = Lesson {
        id: "prepared",
        title: || "Prepared",
        minutes: 1,
        media: &[],
        setup: |a| {
            let kind = jvid_core::model::TrackKind::Video;
            let name = String::new();
            apply(a, Command::AddTrack { kind, name });
        },
        steps: &[
            Step {
                anchor: "render_queue",
                text: |_| String::new(),
                done: |_| false,
                show_me: |_| {},
                reveal: None,
            },
            Step {
                anchor: "nowhere",
                text: |_| String::new(),
                done: |_| false,
                show_me: |_| {},
                reveal: None,
            },
        ],
    };

    /// Runs frames until the sandbox's media has imported and the card has caught up.
    fn settle(h: &mut Harness<'_, App>) {
        for _ in 0..500 {
            h.run();
            if !h.state().importer.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run();
    }

    /// Where anchor `name` was drawn in the last frame run.
    fn drawn(h: &Harness<'_, App>, name: &str) -> Option<Rect> {
        let last = h.ctx.cumulative_pass_nr() - 1;
        h.ctx.data(|d| {
            let a = d.get_temp::<Anchors>(Id::NULL)?;
            (a.pass == last).then(|| a.rects.get(name).copied())?
        })
    }

    fn click(h: &mut Harness<'_, App>, label: &str) {
        h.get_by_label(label).click();
        h.run();
        settle(h);
    }

    #[test]
    fn every_lesson_runs_by_show_me_and_draws_each_steps_anchor() {
        for lesson in LESSONS {
            let mut h = harness();
            h.state_mut().start_lesson(lesson);
            for (i, step) in lesson.steps.iter().enumerate() {
                settle(&mut h);
                let at = h.state().lesson.as_ref().map(|r| r.step);
                assert_eq!(at, Some(i), "{}: on step {i}", lesson.id);
                // A renamed or removed control fails here instead of in the lesson.
                let at = drawn(&h, step.anchor);
                assert!(
                    at.is_some(),
                    "{} step {i}: `{}` never drawn",
                    lesson.id,
                    step.anchor
                );
                click(&mut h, tr!("tutorial.show_me"));
                assert!(
                    (step.done)(h.state()),
                    "{} step {i}: Show Me didn't pass",
                    lesson.id
                );
            }
            settle(&mut h);
            let at = h.state().lesson.as_ref().map(|r| r.step);
            assert_eq!(at, Some(lesson.steps.len()), "{}: finished", lesson.id);
            assert!(h.state().tutorials_done.iter().any(|d| d == lesson.id));
            click(&mut h, tr!("tutorial.exit"));
            assert!(h.state().lesson.is_none());
        }
    }

    #[test]
    fn exiting_halfway_leaves_the_project_and_undo_history_as_before() {
        use jvid_core::model::TrackKind;
        let mut h = harness();
        let user = PathBuf::from("/m/mine.jvid");
        let add = |kind| Command::AddTrack {
            kind,
            name: String::new(),
        };
        let edit = |h: &mut History| {
            h.apply(add(TrackKind::Video)).unwrap();
            h.apply(add(TrackKind::Audio)).unwrap();
            h.undo();
        };
        let state = h.state_mut();
        edit(&mut state.history);
        state.path = Some(user.clone());
        let mut expect = History::new(Project::default());
        edit(&mut expect);
        let before = state.history.project().clone();

        state.start_lesson(&LESSONS[0]);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        settle(&mut h);
        assert_eq!(h.state().lesson.as_ref().map(|r| r.step), Some(1));
        assert_ne!(h.state().history.project(), &before);
        click(&mut h, tr!("tutorial.exit"));

        let state = h.state_mut();
        assert!(state.lesson.is_none());
        assert_eq!(state.path, Some(user));
        assert_eq!(state.history.project(), &before);
        assert_eq!(state.history.is_dirty(), expect.is_dirty());
        // The same redo, then the same two undos, and nothing past them.
        for step in [History::redo, History::undo, History::undo, History::undo] {
            assert_eq!(step(&mut state.history), step(&mut expect));
            assert_eq!(state.history.project(), expect.project());
        }
        assert!(!state.history.can_undo());
    }

    #[test]
    fn back_stays_on_a_done_step_and_skip_moves_on() {
        let mut h = harness();
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        let step = |h: &Harness<'_, App>| h.state().lesson.as_ref().map(|r| r.step);
        assert_eq!(step(&h), Some(1));
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0), "a done step waits after Back");
        click(&mut h, tr!("tutorial.skip_step"));
        click(&mut h, tr!("tutorial.skip_step"));
        assert_eq!(step(&h), Some(2));
    }

    #[test]
    fn a_hidden_anchor_is_revealed() {
        let mut h = harness();
        h.state_mut().bin.tab = 3;
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        assert_eq!(h.state().bin.tab, 2);
        assert!(drawn(&h, "bin.titles").is_some());
        click(&mut h, tr!("tutorial.exit"));
        assert_eq!(h.state().bin.tab, 3, "the bin's tab is put back");
    }

    #[test]
    fn help_tutorials_starts_a_lesson_and_progress_saves_to_settings() {
        let dir = std::env::temp_dir().join(format!("jvid-tutorials-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let mut h = harness();
        h.state_mut().settings_path = Some(path.clone());
        h.get_by_label(tr!("menu.help")).click();
        h.run();
        h.get_by_label_contains(tr!("menu.tutorials")).click();
        h.run();
        let title = (LESSONS[0].title)();
        h.get_by_label_contains(title).click();
        h.run();
        assert_eq!(
            h.state().lesson.as_ref().map(|r| r.lesson.id),
            Some("basics")
        );
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        for _ in &LESSONS[0].steps[1..] {
            click(&mut h, tr!("tutorial.skip_step"));
        }
        let saved = crate::settings_file::load_from(&path);
        assert_eq!(saved.tutorials_done, vec!["basics".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn skipping_every_step_isnt_finishing() {
        let mut h = harness();
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        for _ in LESSONS[0].steps {
            click(&mut h, tr!("tutorial.skip_step"));
        }
        assert_eq!(step(&h), Some(LESSONS[0].steps.len()));
        assert!(h.state().tutorials_done.is_empty());
    }

    #[test]
    fn saving_in_a_lesson_keeps_the_untitled_autosave() {
        let dir = tmp("untitled");
        let untitled = dir.join("Untitled.jvid");
        let auto = crate::autosave::slot(&untitled, 1);
        std::fs::write(&auto, "the user's work").unwrap();
        let mut h = harness();
        h.state_mut().untitled = Some(untitled);
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        // Where Keep Project and Ctrl+S both land once the save dialog picks a file.
        assert!(h.state_mut().save_to(dir.join("kept.jvid")));
        assert!(auto.exists(), "Keep Project deleted the untitled autosave");
        h.key_press_modifiers(Modifiers::COMMAND, Key::S);
        h.run();
        assert!(auto.exists(), "Ctrl+S deleted the untitled autosave");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn starting_a_lesson_writes_the_pending_autosave() {
        let dir = tmp("flush");
        let user = dir.join("mine.jvid");
        let mut h = harness();
        let state = h.state_mut();
        state.path = Some(user.clone());
        let kind = jvid_core::model::TrackKind::Audio;
        let name = String::new();
        state
            .history
            .apply(Command::AddTrack { kind, name })
            .unwrap();
        let before = state.history.project().clone();
        h.run();
        h.state_mut().start_lesson(&LESSONS[0]);
        let saved = crate::project_file::load(&crate::autosave::slot(&user, 1)).unwrap();
        assert_eq!(saved, before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_selection_made_in_a_lesson_doesnt_survive_exit() {
        let mut h = harness();
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        for _ in 0..3 {
            click(&mut h, tr!("tutorial.show_me"));
        }
        assert!(!h.state().timeline.selected().is_empty());
        assert!(h.state().bin.selected.is_some());
        click(&mut h, tr!("tutorial.exit"));
        assert!(h.state().timeline.selected().is_empty());
        assert_eq!(h.state().bin.selected, None);
    }

    #[test]
    fn show_me_selects_the_new_title_like_a_click() {
        let mut h = harness();
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        click(&mut h, tr!("tutorial.show_me"));
        let state = h.state();
        let title = state
            .history
            .project()
            .assets()
            .iter()
            .find(|a| a.text.is_some());
        assert_eq!(state.bin.selected, title.map(|a| a.id));
        assert_eq!(state.bin.tab, 0);
    }

    #[test]
    fn back_reveals_again() {
        let mut h = harness();
        h.state_mut().bin.tab = 3;
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        assert_eq!(h.state().bin.tab, 2);
        click(&mut h, tr!("tutorial.show_me"));
        assert_eq!(step(&h), Some(1));
        h.state_mut().bin.tab = 3;
        click(&mut h, tr!("tutorial.back"));
        assert_eq!(step(&h), Some(0));
        assert_eq!(
            h.state().bin.tab,
            2,
            "Back to step 0 reveals the Text tab again"
        );
    }

    #[test]
    fn setup_prepares_a_project_that_cant_be_undone() {
        let mut h = harness();
        let tracks = |h: &Harness<'_, App>| h.state().history.project().sequence().tracks().len();
        h.state_mut().start_lesson(&PREPARED);
        settle(&mut h);
        let empty = History::new(Project::default());
        assert_eq!(tracks(&h), empty.project().sequence().tracks().len() + 1);
        assert!(!h.state().history.can_undo());
        assert!(!h.state().history.is_dirty());
    }

    #[test]
    fn the_render_queue_reveals_itself_and_a_missing_anchor_is_logged() {
        let mut h = harness();
        assert!(h.state().dock.find_tab(&Tab::RenderQueue).is_none());
        h.state_mut().start_lesson(&PREPARED);
        settle(&mut h);
        assert!(drawn(&h, "render_queue").is_some());
        let warned = |h: &Harness<'_, App>| h.state().lesson.as_ref().is_some_and(|r| r.warned);
        assert!(!warned(&h));
        click(&mut h, tr!("tutorial.skip_step"));
        assert!(warned(&h), "the missing `nowhere` anchor is logged");
    }

    #[test]
    fn a_sample_that_cant_be_found_starts_nothing() {
        const BROKEN: Lesson = Lesson {
            media: &["not-a-sample.mp4"],
            ..PREPARED
        };
        let mut h = harness();
        h.state_mut().start_lesson(&BROKEN);
        assert!(h.state().lesson.is_none());
        assert!(matches!(
            h.state().prompts.as_slice(),
            [super::super::Prompt::Error(_)]
        ));
    }

    #[test]
    fn esc_exits_unless_a_dialog_is_open() {
        let mut h = harness();
        h.state_mut().start_lesson(&LESSONS[0]);
        settle(&mut h);
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().lesson.is_none(), "Esc exits");

        // A card shown after Project Settings opened sits above it, but neither its Exit nor
        // Esc leave the sandbox under the dialog, whose Apply would land on the user's project.
        let under_settings = |h: &mut Harness<'_, App>| {
            h.state_mut().end_lesson();
            let state = h.state_mut();
            state.settings_dialog.open(state.history.project());
            h.run();
            h.state_mut().start_lesson(&LESSONS[0]);
            settle(h);
        };
        under_settings(&mut h);
        h.key_press(Key::Escape);
        h.run();
        assert!(
            h.state().lesson.is_some(),
            "Esc closes the dialog, not the lesson"
        );
        under_settings(&mut h);
        click(&mut h, tr!("tutorial.exit"));
        assert!(
            h.state().lesson.is_some(),
            "the card is inert under a dialog"
        );
    }

    #[test]
    fn no_anchors_are_recorded_without_a_lesson() {
        let h = harness();
        assert!(h.ctx.data(|d| d.get_temp::<Anchors>(Id::NULL)).is_none());
    }

    #[test]
    fn the_dim_stays_under_windows() {
        assert!(DIM < Order::Middle);
    }
}
