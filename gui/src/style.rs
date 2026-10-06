//! Look and layout helpers: one theme, cards, aligned label/value rows.
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Stroke, TextStyle, Vec2};

pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x78, 0x99);
/// Fill for selected and primary things (i3 focused background).
pub const ACCENT_BG: Color32 = Color32::from_rgb(0x28, 0x55, 0x77);
pub const OK: Color32 = Color32::from_rgb(0x5f, 0xd3, 0x8d);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x4c);
pub const BAD: Color32 = Color32::from_rgb(0xf0, 0x6a, 0x6a);
pub const BG: Color32 = Color32::from_rgb(0x0f, 0x10, 0x12);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
const CARD: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x32);
pub const EDGE: Color32 = Color32::from_rgb(0x2a, 0x2c, 0x31);
const INPUT: Color32 = Color32::from_rgb(0x09, 0x0a, 0x0b);
/// Width of the label column in forms.
pub const LABEL_W: f32 = 110.0;
/// Widest a page grows, so lines stay readable on a big window.
pub const PAGE_W: f32 = 760.0;

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = INPUT;
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0_f32, EDGE);
    v.window_corner_radius = CornerRadius::same(0);
    v.menu_corner_radius = CornerRadius::same(0);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x28, 0x55, 0x77, 200);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    let r = CornerRadius::same(0);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xd8, 0xdb, 0xde));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x24, 0x26, 0x2a);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x24, 0x26, 0x2a);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x30, 0x33, 0x38);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x30, 0x33, 0x38);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.bg_fill = ACCENT_BG;
    v.widgets.active.weak_bg_fill = ACCENT_BG;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(8.0, 3.0);
        s.spacing.interact_size.y = 20.0;
        s.spacing.window_margin = Margin::same(16);
        s.spacing.indent = 18.0;
        s.text_styles.insert(TextStyle::Heading, FontId::monospace(14.0));
        s.text_styles.insert(TextStyle::Body, FontId::monospace(11.0));
        s.text_styles.insert(TextStyle::Button, FontId::monospace(11.0));
        s.text_styles.insert(TextStyle::Small, FontId::monospace(9.5));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(11.0));
    });
}

/// A page: title, one line saying what it is for, then the content, centred and not wider than `PAGE_W`.
pub fn page(ui: &mut egui::Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.set_max_width(PAGE_W.min(ui.available_width() - 18.0));
        ui.add_space(4.0);
        ui.label(RichText::new(title).heading().strong());
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).weak());
        }
        ui.add_space(10.0);
        add(ui);
        ui.add_space(24.0);
    });
}

/// A flat group: bold caption over a rule, then the content. No box.
pub fn card(ui: &mut egui::Ui, caption: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.set_width(ui.available_width());
        if let Some(c) = caption {
            ui.label(RichText::new(c).strong());
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 0.0, EDGE);
            ui.add_space(2.0);
        }
        add(ui);
    });
    ui.add_space(14.0);
}

/// `label  [content............]` with the label in a fixed column so rows line up.
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(LABEL_W, 26.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(Vec2::new(LABEL_W, 26.0));
            ui.label(RichText::new(label).weak());
        });
        let w = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new(w, 26.0), Layout::left_to_right(Align::Center).with_main_wrap(true), add).inner
    })
    .inner
}

/// A single-line text field that fills the rest of its row.
pub fn text(ui: &mut egui::Ui, value: &mut String) -> egui::Response {
    ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width()).margin(Margin::symmetric(8, 5)))
}

/// Small grey explanatory text.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
}

/// A coloured one-line message with a leading dot.
pub fn note(ui: &mut egui::Ui, color: Color32, text: impl Into<String>) {
    ui.horizontal_wrapped(|ui| {
        // The dot is drawn: the default font has no bullet glyph.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 18.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.label(RichText::new(text.into()).color(color));
    });
}

/// A button in the accent colour for the main action of a page.
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(ACCENT_BG).min_size(Vec2::new(0.0, 30.0))
}

/// A small plain tag such as a file name or a state; only the text is coloured.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0_f32, EDGE))
        .corner_radius(CornerRadius::same(0))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).small().color(color)).extend());
        });
}

/// A text field with a suggestion list under it, filtered as you type (arrow keys and Enter pick,
/// Escape closes). With `list` the value is a comma separated list and only its last item is completed.
/// Typing anything is still allowed; a value that is not among the options gets a note.
pub fn suggest(ui: &mut egui::Ui, salt: &str, value: &mut String, options: &[String], list: bool) -> bool {
    let id = ui.make_persistent_id(salt);
    let (open_id, sel_id) = (id.with("open"), id.with("sel"));
    let resp = text(ui, value);
    let mut changed = resp.changed();
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    if resp.gained_focus() || resp.changed() {
        open = true;
    }
    let (head, token) = match (list, value.rfind(',')) {
        (true, Some(i)) => (value[..=i].to_string() + " ", value[i + 1..].trim().to_string()),
        _ => (String::new(), value.trim().to_string()),
    };
    if open {
        let q = token.to_lowercase();
        let mut items: Vec<&String> = options.iter().filter(|o| o.to_lowercase().contains(&q) && **o != token).collect();
        items.sort_by_key(|o| !o.to_lowercase().starts_with(&q));
        if items.is_empty() {
            open = false;
        } else {
            let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0).min(items.len() - 1);
            let mut moved = false;
            let mut pick: Option<String> = None;
            if resp.has_focus() || resp.lost_focus() {
                ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                        sel = (sel + 1) % items.len();
                        moved = true;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                        sel = (sel + items.len() - 1) % items.len();
                        moved = true;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                        pick = Some(items[sel].clone());
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                        open = false;
                    }
                });
            }
            let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(resp.rect.left_bottom() + Vec2::new(0.0, 2.0)).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(0)).show(ui, |ui| {
                    ui.set_width(resp.rect.width() - 12.0);
                    egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                        for (n, it) in items.iter().enumerate() {
                            let r = ui.selectable_label(n == sel, it.as_str());
                            if n == sel && moved {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                pick = Some((*it).clone());
                            }
                            if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                                sel = n;
                            }
                        }
                    }));
                });
            });
            if ui.input(|i| i.pointer.any_pressed()) {
                if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                    if !resp.rect.contains(p) && !area.response.rect.contains(p) {
                        open = false;
                    }
                }
            }
            ui.data_mut(|d| d.insert_temp(sel_id, sel));
            if let Some(p) = pick {
                *value = format!("{head}{p}");
                if list {
                    value.push_str(", ");
                }
                changed = true;
                open = false;
                resp.request_focus();
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    let shown = value.trim();
    if !list && !shown.is_empty() && !options.is_empty() && !options.iter().any(|o| o == shown) {
        ui.label(RichText::new("not in the list").small().color(WARN));
    }
    changed
}
