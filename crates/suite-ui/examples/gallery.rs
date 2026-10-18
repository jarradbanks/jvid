//! Every suite-ui widget in every state: `cargo run -p suite-ui --example gallery`.

use eframe::egui;
use suite_ui::panel::Panel;
use suite_ui::theme::{self, icon};
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{
    Button, ButtonKind, Checkbox, DragField, Dropdown, SectionHeader, Segmented, State, TextInput,
    Timecode,
};

fn main() -> eframe::Result {
    eframe::run_native(
        "suite-ui gallery",
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

const STATES: [(&str, State); 5] = [
    ("Default", State::Default),
    ("Hover", State::Hover),
    ("Focus-visible", State::Focus),
    ("Pressed", State::Pressed),
    ("Disabled", State::Disabled),
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
                Panel::tabbed(&["Widgets", "Effects", "Text"], &mut tab)
                    .focused(true)
                    .actions(&[(icon::PLUS, "Add"), (icon::ELLIPSIS, "More")])
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| states(ui, g));
                    });
                g.tab = tab;
            });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::GUTTER;
                ui.allocate_ui(egui::vec2(360.0, 200.0), |ui| {
                    Panel::titled("Inspector").show(ui, |ui| inspector(ui, g));
                });
                ui.allocate_ui(egui::vec2(360.0, 120.0), |ui| {
                    let labels = [
                        "Source", "Program", "Audio", "Markers", "History", "Captions",
                    ];
                    Panel::tabbed(&labels, &mut g.many_tabs).show(ui, |ui| {
                        ui.label("Tabs below 64px move into the chevron menu.");
                    });
                });
                ui.allocate_ui(egui::vec2(360.0, ui.available_height()), |ui| {
                    Panel::titled("Type and Colour").show(ui, type_and_colour);
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
                ui.label(egui::RichText::new(name).color(color::INK_3));
            }
            ui.label(egui::RichText::new("Loading / Error").color(color::INK_3));
            ui.end_row();

            for kind in [
                ButtonKind::Primary,
                ButtonKind::Secondary,
                ButtonKind::Ghost,
                ButtonKind::Destructive,
            ] {
                ui.label(format!("{kind:?}"));
                let label = if kind == ButtonKind::Destructive {
                    "Delete"
                } else {
                    "Export"
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

            ui.label("Icon");
            for (_, s) in STATES {
                ui.add(Button::icon(icon::PLUS, "Add").state(s));
            }
            ui.add(Button::icon(icon::PLUS, "Add").on(true));
            ui.end_row();

            ui.label("Input");
            for (_, s) in STATES {
                ui.add(
                    TextInput::new(&mut g.text)
                        .prefix("W")
                        .hint("1920")
                        .width(120.0)
                        .state(s),
                );
            }
            ui.add(
                TextInput::new(&mut g.text)
                    .prefix("W")
                    .hint("1920")
                    .width(120.0)
                    .error(Some("Must be at least 2")),
            );
            ui.end_row();

            ui.label("Search");
            for (_, s) in STATES {
                ui.add(TextInput::search(&mut g.search).width(120.0).state(s));
            }
            ui.end_row();

            ui.label("Numeric");
            for (_, s) in STATES {
                ui.add(DragField::new(&mut g.value).prefix("X").state(s));
            }
            ui.end_row();

            ui.label("Dropdown");
            let options = ["1/2", "Full", "1/4"];
            for (_, s) in STATES {
                ui.add(Dropdown::new(&mut g.choice, &options).width(96.0).state(s));
            }
            ui.end_row();

            ui.label("Segmented");
            for (_, s) in STATES {
                ui.add(Segmented::new(&mut g.segment, &["Fit", "Fill", "1:1"]).state(s));
            }
            ui.end_row();

            ui.label("Checkbox");
            for (_, s) in STATES {
                let mut on = g.checked || s == State::Pressed;
                ui.add(Checkbox::new(&mut on, "Snap").state(s));
            }
            ui.add(Checkbox::new(&mut g.checked, "Live"));
            ui.end_row();

            ui.label("Section");
            for (_, s) in STATES {
                ui.allocate_ui(egui::vec2(120.0, size::HEADER), |ui| {
                    let mut open = g.open;
                    ui.add(SectionHeader::new("Transform", &mut open).state(s));
                });
            }
            ui.end_row();
        });
}

fn inspector(ui: &mut egui::Ui, g: &mut Gallery) {
    ui.add(SectionHeader::new("Transform", &mut g.open));
    if g.open {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S4;
            ui.allocate_ui_with_layout(
                egui::vec2(size::INSPECTOR_LABEL, size::CONTROL),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.label(egui::RichText::new("Position").color(color::INK_2)),
            );
            ui.add(DragField::new(&mut g.value).prefix("X"));
            ui.add(DragField::new(&mut g.value).prefix("Y"));
        });
    }
    ui.horizontal(|ui| {
        ui.add(Timecode::new(g.frame, 30));
        ui.add(Timecode::new(g.frame, 30).small());
        if ui.add(Button::secondary("+1s")).clicked() {
            g.frame += 30;
        }
    });
}

fn type_and_colour(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Display 22").font(theme::display()));
    ui.label(egui::RichText::new("Headline 16").font(theme::headline()));
    ui.label(egui::RichText::new("Title 14").font(theme::title()));
    ui.label(egui::RichText::new("Body strong 12").font(theme::body_strong()));
    ui.label("Body 12 · 0123456789 holds still");
    ui.label(
        egui::RichText::new("Small 10 · shortcut hint")
            .font(theme::small())
            .color(color::INK_3),
    );
    ui.add_space(space::S8);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(space::S4, space::S4);
        let names = ["Video", "Title", "Still", "Audio"];
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
