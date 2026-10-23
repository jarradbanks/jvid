//! Every suite-ui widget in every state: `cargo run -p suite-ui --example gallery`.

use eframe::egui;
use suite_ui::i18n::{self, Catalog, verbatim};
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{
    Button, ButtonKind, Checkbox, DragField, Dropdown, SectionHeader, Segmented, State, TextInput,
    Timecode,
};

/// The gallery's demo labels; the widgets' own strings are in suite-ui's catalog.
pub static CATALOG: Catalog = Catalog::new([
    include_str!("../i18n/gallery/en.toml"),
    include_str!("../i18n/gallery/es.toml"),
]);

macro_rules! tr {
    ($($args:tt)+) => {
        suite_ui::tr!(CATALOG, $($args)+)
    };
}

fn main() -> eframe::Result {
    i18n::set_language(i18n::resolve(i18n::SYSTEM));
    eframe::run_native(
        verbatim("suite-ui gallery"),
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 860.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(Gallery::new()))
        }),
    )
}

#[derive(Default)]
pub struct Gallery {
    tab: usize,
    many_tabs: usize,
    text: String,
    search: String,
    value: f64,
    choice: usize,
    segment: usize,
    checked: bool,
    open: bool,
    frame: i64,
}

impl Gallery {
    pub fn new() -> Self {
        Self {
            open: true,
            frame: 30 * 72 + 8,
            ..Self::default()
        }
    }
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        gallery(ui, self);
    }
}

const STATES: [(i18n::Label, State); 5] = [
    (|| tr!("state.default"), State::Default),
    (|| tr!("state.hover"), State::Hover),
    (|| tr!("state.focus"), State::Focus),
    (|| tr!("state.pressed"), State::Pressed),
    (|| tr!("state.disabled"), State::Disabled),
];

/// The whole gallery, also rendered by the snapshot test.
pub fn gallery(ui: &mut egui::Ui, g: &mut Gallery) {
    ui.painter()
        .rect_filled(ui.ctx().content_rect(), 0.0, color::GROUND);
    let area = ui.ctx().content_rect().shrink(space::GUTTER);
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = space::GUTTER;
            let w = ui.available_width();
            let h = ui.available_height();
            ui.allocate_ui(egui::vec2(w - 360.0 - space::GUTTER, h), |ui| {
                let mut tab = g.tab;
                let tabs = [
                    tr!("panel.widgets"),
                    tr!("panel.effects"),
                    tr!("panel.text"),
                ];
                Panel::tabbed(&tabs, &mut tab)
                    .focused(true)
                    .actions(&[
                        (icon::PLUS, tr!("panel.add")),
                        (icon::ELLIPSIS, tr!("panel.more")),
                    ])
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| states(ui, g));
                    });
                g.tab = tab;
            });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::GUTTER;
                ui.allocate_ui(egui::vec2(360.0, 200.0), |ui| {
                    Panel::titled(tr!("panel.inspector")).show(ui, |ui| inspector(ui, g));
                });
                ui.allocate_ui(egui::vec2(360.0, 120.0), |ui| {
                    let labels = [
                        tr!("panel.source"),
                        tr!("panel.program"),
                        tr!("panel.audio"),
                        tr!("panel.markers"),
                        tr!("panel.history"),
                        tr!("panel.captions"),
                    ];
                    Panel::tabbed(&labels, &mut g.many_tabs).show(ui, |ui| {
                        ui.label(tr!("panel.overflow"));
                    });
                });
                ui.allocate_ui(egui::vec2(360.0, ui.available_height()), |ui| {
                    Panel::titled(tr!("panel.type_and_colour")).show(ui, type_and_colour);
                });
            });
        });
    });
}

fn states(ui: &mut egui::Ui, g: &mut Gallery) {
    egui::Grid::new("states")
        .spacing([space::S16, space::S12])
        .show(ui, |ui| {
            ui.label("");
            for (name, _) in STATES {
                ui.label(egui::RichText::new(name()).color(color::INK_3));
            }
            ui.label(egui::RichText::new(tr!("state.loading_error")).color(color::INK_3));
            ui.end_row();

            for kind in [
                ButtonKind::Primary,
                ButtonKind::Secondary,
                ButtonKind::Ghost,
                ButtonKind::Destructive,
            ] {
                ui.label(match kind {
                    ButtonKind::Primary => tr!("row.primary"),
                    ButtonKind::Secondary => tr!("row.secondary"),
                    ButtonKind::Ghost => tr!("row.ghost"),
                    ButtonKind::Destructive => tr!("row.destructive"),
                });
                let label = if kind == ButtonKind::Destructive {
                    tr!("demo.delete")
                } else {
                    tr!("demo.export")
                };
                for (_, s) in STATES {
                    ui.add(Button::new(kind, label).state(s));
                }
                ui.add(
                    Button::new(kind, label)
                        .with_icon(icon::CHECK)
                        .loading(true),
                );
                ui.end_row();
            }

            ui.label(tr!("row.icon"));
            for (_, s) in STATES {
                ui.add(Button::icon(icon::PLUS, tr!("panel.add")).state(s));
            }
            ui.add(Button::icon(icon::PLUS, tr!("panel.add")).on(true));
            ui.end_row();

            ui.label(tr!("row.input"));
            for (_, s) in STATES {
                ui.add(
                    TextInput::new(&mut g.text)
                        .prefix(tr!("demo.width_prefix"))
                        .hint("1920")
                        .width(120.0)
                        .state(s),
                );
            }
            ui.add(
                TextInput::new(&mut g.text)
                    .prefix(tr!("demo.width_prefix"))
                    .hint("1920")
                    .width(120.0)
                    .error(Some(tr!("demo.min_error"))),
            );
            ui.end_row();

            ui.label(tr!("row.search"));
            for (_, s) in STATES {
                ui.add(TextInput::search(&mut g.search).width(120.0).state(s));
            }
            ui.end_row();

            ui.label(tr!("row.numeric"));
            for (_, s) in STATES {
                ui.add(DragField::new(&mut g.value).prefix(verbatim("X")).state(s));
            }
            ui.end_row();

            ui.label(tr!("row.dropdown"));
            let options = ["1/2", tr!("demo.full"), "1/4"];
            for (_, s) in STATES {
                ui.add(Dropdown::new(&mut g.choice, &options).width(96.0).state(s));
            }
            ui.end_row();

            ui.label(tr!("row.segmented"));
            let segments = [tr!("demo.fit"), tr!("demo.fill"), "1:1"];
            for (_, s) in STATES {
                ui.add(Segmented::new(&mut g.segment, &segments).state(s));
            }
            ui.end_row();

            ui.label(tr!("row.checkbox"));
            for (_, s) in STATES {
                let mut on = g.checked || s == State::Pressed;
                ui.add(Checkbox::new(&mut on, tr!("demo.snap")).state(s));
            }
            ui.add(Checkbox::new(&mut g.checked, tr!("demo.live")));
            ui.end_row();

            ui.label(tr!("row.section"));
            for (_, s) in STATES {
                ui.allocate_ui(egui::vec2(120.0, size::HEADER), |ui| {
                    let mut open = g.open;
                    ui.add(SectionHeader::new(tr!("demo.transform"), &mut open).state(s));
                });
            }
            ui.end_row();
        });
}

fn inspector(ui: &mut egui::Ui, g: &mut Gallery) {
    ui.add(SectionHeader::new(tr!("demo.transform"), &mut g.open));
    if g.open {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S4;
            ui.allocate_ui_with_layout(
                egui::vec2(size::INSPECTOR_LABEL, size::CONTROL),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.label(egui::RichText::new(tr!("demo.position")).color(color::INK_2)),
            );
            ui.add(DragField::new(&mut g.value).prefix(verbatim("X")));
            ui.add(DragField::new(&mut g.value).prefix(verbatim("Y")));
        });
    }
    ui.horizontal(|ui| {
        ui.add(Timecode::new(g.frame, 30));
        ui.add(Timecode::new(g.frame, 30).small());
        if ui.add(Button::secondary(tr!("demo.plus_second"))).clicked() {
            g.frame += 30;
        }
    });
}

fn type_and_colour(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tr!("type.display")).font(theme::display()));
    ui.label(egui::RichText::new(tr!("type.headline")).font(theme::headline()));
    ui.label(egui::RichText::new(tr!("type.title")).font(theme::title()));
    ui.label(egui::RichText::new(tr!("type.body_strong")).font(theme::body_strong()));
    ui.label(tr!("type.body"));
    ui.label(
        egui::RichText::new(tr!("type.small"))
            .font(theme::small())
            .color(color::INK_3),
    );
    ui.add_space(space::S8);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(space::S4, space::S4);
        let names = [
            tr!("clip.video"),
            tr!("clip.title"),
            tr!("clip.still"),
            tr!("clip.audio"),
        ];
        for (clip, name) in color::CLIPS.into_iter().zip(names) {
            egui::Frame::new()
                .fill(clip)
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(name)
                            .font(theme::body_strong())
                            .color(color::INK),
                    );
                });
        }
    });
}
