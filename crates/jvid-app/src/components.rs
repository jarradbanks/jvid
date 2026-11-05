//! Optional speech models, offered once and available later from Help. Editing never
//! depends on this dialog or a download. Uses the same verified downloader as subtitles.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use egui::RichText;
use jvid_jobs::{Handle, Pool};
use jvid_transcribe::Error;
use jvid_transcribe::models::{self, Model};
use suite_ui::dialog::action_footer;
use suite_ui::theme;
use suite_ui::tokens::{color, radius, space};
use suite_ui::widgets::{Button, State};

use crate::i18n::{tr, verbatim};

struct ModelDownload {
    model: Model,
    job: Handle,
    result: Receiver<Result<PathBuf, Error>>,
}

#[derive(Default)]
pub struct Components {
    pub open: bool,
    jobs: Option<Pool>,
    download: Option<ModelDownload>,
    error: Option<String>,
}

impl Components {
    pub fn offer(&mut self) {
        self.open =
            jvid_paths::config_dir().is_some_and(|d| !d.join("components-offered").is_file());
    }

    /// Includes cancelled jobs until their bounded network wait has unwound. The
    /// updater must wait for this before exiting to run an installer.
    pub fn is_busy(&self) -> bool {
        self.jobs.as_ref().is_some_and(|jobs| !jobs.is_idle())
    }

    fn cancel(&mut self) {
        if let Some(download) = self.download.take() {
            download.job.cancel();
        }
    }

    fn close(&mut self) {
        self.cancel();
        self.open = false;
        if let Some(dir) = jvid_paths::config_dir().filter(|_| !cfg!(test))
            && let Err(e) = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(dir.join("components-offered"), b"1\n"))
        {
            tracing::warn!("can't remember optional component offer: {e}");
        }
    }

    fn start(&mut self, model: Model, dir: &std::path::Path, ctx: &egui::Context) {
        self.cancel();
        self.error = None;
        let jobs = self.jobs.get_or_insert_with(|| Pool::new(1));
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let job = model.download(jobs, dir, move |result| {
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.download = Some(ModelDownload {
            model,
            job,
            result: rx,
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        self.show_at(ctx, models::dir());
    }

    fn show_at(&mut self, ctx: &egui::Context, dir: Option<PathBuf>) {
        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if !self.open {
            return;
        }
        if let Some(download) = &self.download
            && let Ok(result) = download.result.try_recv()
        {
            self.error = result.err().map(|e| e.to_string());
            self.download = None;
        }
        let mut start = None;
        let mut cancel = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("optional_components"))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ctx, |ui| {
                ui.set_width(460.0_f32.min((ctx.content_rect().width() - 48.0).max(1.0)));
                ui.label(RichText::new(tr!("components.title")).font(theme::headline()));
                ui.add_space(space::S8);
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 160.0).max(0.0))
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(tr!("components.purpose"));
                        ui.add_space(space::S16);
                        if let Some(dir) = &dir {
                            let status_width = [
                                tr!("components.download"),
                                tr!("components.installed"),
                                tr!("prompt.cancel"),
                            ]
                            .map(|text| {
                                ui.painter()
                                    .layout_no_wrap(
                                        text.to_owned(),
                                        theme::body_strong(),
                                        color::ink(),
                                    )
                                    .size()
                                    .x
                                    + 2.0 * space::S12
                            })
                            .into_iter()
                            .fold(0.0_f32, f32::max);
                            let size_width = ui
                                .painter()
                                .layout_no_wrap(
                                    tr!("export.mb", value = 574),
                                    theme::body(),
                                    color::ink_3(),
                                )
                                .size()
                                .x;
                            let name_width = (ui.available_width()
                                - status_width
                                - size_width
                                - 2.0 * space::S12)
                                .max(40.0);
                            egui::Grid::new("component_models")
                                .min_col_width(0.0)
                                .spacing([space::S12, space::S8])
                                .show(ui, |ui| {
                                    for (model, name) in [
                                        (Model::Base, verbatim("Whisper Base")),
                                        (Model::Small, verbatim("Whisper Small")),
                                        (
                                            Model::LargeV3Turbo,
                                            verbatim("Whisper Large v3 Turbo (Q5)"),
                                        ),
                                    ] {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(name_width, 24.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.set_min_width(name_width);
                                                ui.add(egui::Label::new(name).wrap());
                                            },
                                        );
                                        let size = (model.size() as f64 / 1e6).round().to_string();
                                        ui.label(
                                            RichText::new(tr!("export.mb", value = size))
                                                .color(color::ink_3()),
                                        );
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(status_width, 0.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                if model.is_downloaded(dir) {
                                                    ui.label(
                                                        RichText::new(tr!("components.installed"))
                                                            .color(color::ink_2()),
                                                    );
                                                } else if let Some(download) = &self.download
                                                    && download.model == model
                                                {
                                                    ui.add(
                                                        egui::ProgressBar::new(
                                                            download.job.progress(),
                                                        )
                                                        .desired_width(status_width)
                                                        .show_percentage(),
                                                    )
                                                    .on_hover_text(tr!("subtitles.downloading"));
                                                    cancel = ui
                                                        .add(Button::secondary(tr!(
                                                            "prompt.cancel"
                                                        )))
                                                        .clicked();
                                                } else {
                                                    let state = if self.download.is_none() {
                                                        State::Live
                                                    } else {
                                                        State::Disabled
                                                    };
                                                    if ui
                                                        .add(
                                                            Button::secondary(tr!(
                                                                "components.download"
                                                            ))
                                                            .state(state),
                                                        )
                                                        .clicked()
                                                    {
                                                        start = Some(model);
                                                    }
                                                }
                                            },
                                        );
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(space::S12);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(tr!("components.location", path = dir.display()))
                                        .color(color::ink_3()),
                                )
                                .wrap(),
                            );
                        } else {
                            ui.label(tr!("subtitles.no_folder"));
                        }
                        if let Some(error) = &self.error {
                            ui.add_space(space::S8);
                            ui.label(RichText::new(error).color(color::red()));
                        }
                    });
                ui.add_space(space::S16);
                close = action_footer(ui, [Button::primary(tr!("settings.done"))])[0].clicked();
            });
        if close || modal.should_close() {
            self.close();
        } else if cancel {
            self.cancel();
        } else if let (Some(model), Some(dir)) = (start, dir) {
            self.start(model, &dir, ctx);
        }
    }
}

impl Drop for Components {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn cancelled_transfer_remains_busy_until_its_worker_exits() {
        let (started_tx, started_rx) = channel();
        let (finish_tx, finish_rx) = channel();
        let jobs = Pool::new(1);
        let job = jobs.submit(
            "blocked-model",
            jvid_jobs::Priority::Interactive,
            move |_| {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
            },
        );
        let (_, receiver) = channel();
        let mut components = Components {
            open: false,
            jobs: Some(jobs),
            download: Some(ModelDownload {
                model: Model::Base,
                job: job.clone(),
                result: receiver,
            }),
            error: None,
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        components.cancel();
        assert!(job.is_cancelled());
        assert!(components.is_busy());
        finish_tx.send(()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while components.is_busy() && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        assert!(!components.is_busy());
    }

    #[test]
    fn optional_models_offer_sizes_and_can_be_dismissed_without_download() {
        let mut fresh = true;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, c: &mut Components| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                c.show(ui.ctx());
            },
            Components {
                open: true,
                jobs: None,
                download: None,
                error: None,
            },
        );
        h.run();
        h.get_by_label("Whisper Base");
        h.get_by_label(&tr!("export.mb", value = "148"));
        assert!(h.state().download.is_none());
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(!h.state().open);
        assert!(h.state().download.is_none());
        h.state_mut().open = true;
        h.run();
        h.get_by_label(tr!("components.purpose"));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn models_sizes_and_actions_align_at_narrow_widths_and_scales() {
        for scale in [1.0, 1.5, 2.0] {
            for size in [[360.0, 420.0], [1280.0, 900.0]] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().to_owned();
                let mut fresh = true;
                let mut h = egui_kittest::Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(scale)
                    .build_ui_state(
                        move |ui, c: &mut Components| {
                            if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                                c.show_at(ui.ctx(), Some(path.clone()));
                            }
                        },
                        Components {
                            open: true,
                            jobs: None,
                            download: None,
                            error: None,
                        },
                    );
                h.run();
                let viewport = h.ctx.content_rect();
                let area = h
                    .ctx
                    .memory(|m| m.area_rect(egui::Id::new("optional_components")))
                    .unwrap();
                assert!(
                    viewport.contains_rect(area),
                    "{size:?} at {scale}: {area:?}"
                );
                let names = [
                    "Whisper Base",
                    "Whisper Small",
                    "Whisper Large v3 Turbo (Q5)",
                ];
                let name_rects = names.map(|name| h.get_by_label(name).rect());
                let download_rects: Vec<_> = h
                    .get_all_by_label(tr!("components.download"))
                    .map(|n| n.rect())
                    .collect();
                assert_eq!(download_rects.len(), 3);
                for (name, action) in name_rects.into_iter().zip(download_rects.iter()) {
                    assert!(viewport.contains_rect(name) && viewport.contains_rect(*action));
                    assert!((name.left() - name_rects[0].left()).abs() < 1.0);
                    assert!((action.left() - download_rects[0].left()).abs() < 1.0);
                    assert!(name.right() < action.left());
                }
                let done = h.get_by_label(tr!("settings.done")).rect();
                assert!(done.top() >= name_rects[2].bottom());
                assert!((done.right() - area.right() + space::S16).abs() < 2.0);
                h.get_by_label(tr!("settings.done")).click();
                h.run();
                assert!(!h.state().open && h.state().download.is_none());
            }
        }
    }

    #[test]
    fn optional_components_narrow_snapshot() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let mut fresh = true;
        let mut components = Components {
            open: true,
            jobs: None,
            download: None,
            error: None,
        };
        let mut h = egui_kittest::Harness::builder()
            .with_size([384.0, 420.0])
            .wgpu()
            .build_ui(move |ui| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    components.show_at(ui.ctx(), Some(PathBuf::from("models")));
                }
            });
        h.run();
        h.snapshot("optional_components_narrow");
    }

    #[test]
    #[ignore = "changes global locale; run this test in isolation"]
    fn translated_models_fit_minimum_viewports() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                suite_ui::i18n::set_pseudo(false);
                suite_ui::i18n::set_language("en");
            }
        }
        let _restore = Restore;
        for language in ["de", "fr", "ja", "en"] {
            suite_ui::i18n::set_language(language);
            suite_ui::i18n::set_pseudo(language == "en");
            models_sizes_and_actions_align_at_narrow_widths_and_scales();
        }
    }

    #[test]
    fn active_model_progress_cancel_completion_and_error_remain_live() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_owned();
        let jobs = Pool::new(1);
        let (finish_tx, finish_rx) = channel();
        let job = jobs.submit(
            "component-test-progress",
            jvid_jobs::Priority::Interactive,
            move |ctx| {
                ctx.set_progress(0.4);
                let _ = finish_rx.recv_timeout(Duration::from_secs(5));
            },
        );
        let (result_tx, result_rx) = channel();
        let mut fresh = true;
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            move |ui, c: &mut Components| {
                if !suite_ui::test_support::install_theme(ui, &mut fresh) {
                    c.show_at(ui.ctx(), Some(path.clone()));
                }
            },
            Components {
                open: true,
                jobs: Some(jobs),
                download: Some(ModelDownload {
                    model: Model::Small,
                    job: job.clone(),
                    result: result_rx,
                }),
                error: None,
            },
        );
        h.run_steps(4);
        let small = h.get_by_label("Whisper Small").rect();
        let cancel = h.get_by_label(tr!("prompt.cancel")).rect();
        assert!(cancel.left() > small.right());
        assert_eq!(h.get_all_by_label(tr!("components.download")).count(), 2);
        result_tx.send(Err(Error::Cancelled)).unwrap();
        h.run_steps(2);
        assert!(h.state().download.is_none() && h.state().error.is_some());
        assert_eq!(h.get_all_by_label(tr!("components.download")).count(), 3);
        h.state_mut().download = Some(ModelDownload {
            model: Model::Small,
            job: job.clone(),
            result: channel().1,
        });
        h.run_steps(2);
        h.get_by_label(tr!("prompt.cancel")).click();
        h.run_steps(2);
        assert!(job.is_cancelled());
        assert!(h.state().open && h.state().download.is_none());
        finish_tx.send(()).unwrap();
        let installed = Model::Small.path(dir.path());
        std::fs::File::create(&installed)
            .unwrap()
            .set_len(Model::Small.size())
            .unwrap();
        let (tx, rx) = channel();
        h.state_mut().download = Some(ModelDownload {
            model: Model::Small,
            job: job.clone(),
            result: rx,
        });
        tx.send(Ok(installed)).unwrap();
        h.run_steps(2);
        assert!(h.state().download.is_none() && h.state().error.is_none());
        h.get_by_label(tr!("components.installed"));
    }
}
