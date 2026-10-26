//! The window's look (2026-10-22 redesign, the user: "less wordy, less in your face"): a restrained dark theme with
//! one accent (gold), rounded cards, coloured status dots, a primary button, small secondary buttons, the step
//! indicator and the page title. Only egui built-ins (no icon font, no image).

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Pos2, Response, RichText, Sense, Stroke, TextStyle, Vec2};
use installer::detect::Level;

pub const ACCENT: Color32 = Color32::from_rgb(0xd4, 0xa8, 0x4a);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x1a, 0x16, 0x0c);
pub const GREEN: Color32 = Color32::from_rgb(0x4c, 0xb8, 0x6a);
pub const AMBER: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x30);
pub const RED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x5a);
pub const MUTED: Color32 = Color32::from_rgb(0x6e, 0x74, 0x7e);

const BG: Color32 = Color32::from_rgb(0x15, 0x17, 0x1b);
const CARD: Color32 = Color32::from_rgb(0x1d, 0x20, 0x26);
const CARD_LINE: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x36);
const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdf, 0xe4);
const WEAK: Color32 = Color32::from_rgb(0x95, 0x9b, 0xa5);

pub fn level_color(l: Level) -> Color32 {
    match l {
        Level::Ok => GREEN,
        Level::Warn => AMBER,
        Level::Fail => RED,
    }
}

pub fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.5));
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.tooltip_width = 460.0;
        // always-visible scroll bars: a page longer than the window shows that it scrolls
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(0x10, 0x12, 0x15);
        v.faint_bg_color = CARD;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = Color32::from_rgb(0x5c, 0x4a, 0x22);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_stroke = Stroke::new(1.0, CARD_LINE);
        let r = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.bg_stroke = Stroke::new(1.0, CARD_LINE);
        for (x, fill, line) in [
            (&mut w.inactive, Color32::from_rgb(0x2a, 0x2e, 0x35), Color32::from_rgb(0x34, 0x39, 0x42)),
            (&mut w.hovered, Color32::from_rgb(0x34, 0x39, 0x42), Color32::from_rgb(0x55, 0x5b, 0x66)),
            (&mut w.active, Color32::from_rgb(0x3d, 0x43, 0x4e), ACCENT),
            (&mut w.open, Color32::from_rgb(0x2a, 0x2e, 0x35), Color32::from_rgb(0x34, 0x39, 0x42)),
        ] {
            x.weak_bg_fill = fill;
            x.bg_fill = fill;
            x.bg_stroke = Stroke::new(1.0, line);
            x.corner_radius = r;
        }
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.noninteractive.corner_radius = r;
        v.weak_text_color = Some(WEAK);
    });
}

/// A rounded card around a group of rows.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui)
        })
        .inner
}

/// A card tinted by a level (notes, reports, failures).
pub fn tinted_card<R>(ui: &mut egui::Ui, level: Level, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let c = level_color(level);
    egui::Frame::new()
        .fill(c.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, c.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            add(ui)
        })
        .inner
}

/// The small label above a card ("ELDEN RING", "OPTIONS").
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text.to_uppercase()).small().strong().color(MUTED));
}

/// The page's title and its one sentence.
pub fn page_title(ui: &mut egui::Ui, title: &str, sentence: &str) {
    ui.label(RichText::new(title).heading().strong().color(TEXT));
    if !sentence.is_empty() {
        ui.label(RichText::new(sentence).color(WEAK));
    }
    ui.add_space(6.0);
}

/// A filled status dot (the row's level).
pub fn dot(ui: &mut egui::Ui, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(10.0, ui.spacing().interact_size.y.min(20.0)), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
    resp
}

/// The prominent button (Next / Install / Play): accent fill.
pub fn primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> Response {
    let b = egui::Button::new(RichText::new(text).strong().color(ACCENT_TEXT).size(15.0)).fill(ACCENT).min_size(Vec2::new(110.0, 34.0)).corner_radius(CornerRadius::same(7));
    ui.add_enabled(enabled, b)
}

/// A big button for the Play actions (primary = accent).
pub fn big(ui: &mut egui::Ui, text: &str, primary: bool, enabled: bool) -> Response {
    let t = RichText::new(text).strong().size(17.0);
    let b = if primary { egui::Button::new(t.color(ACCENT_TEXT)).fill(ACCENT) } else { egui::Button::new(t) };
    ui.add_enabled(enabled, b.min_size(Vec2::new(190.0, 46.0)).corner_radius(CornerRadius::same(8)))
}

/// A secondary action: a smaller button.
pub fn secondary(ui: &mut egui::Ui, text: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).size(13.0)))
}

/// A danger action (Uninstall): red fill.
pub fn danger(ui: &mut egui::Ui, text: &str, enabled: bool) -> Response {
    let b = egui::Button::new(RichText::new(text).strong().color(Color32::WHITE).size(15.0)).fill(Color32::from_rgb(0xb0, 0x3a, 0x3a)).min_size(Vec2::new(110.0, 34.0)).corner_radius(CornerRadius::same(7));
    ui.add_enabled(enabled, b)
}

/// A small rounded chip (shortcut names, last result).
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (r, _) = ui.allocate_exact_size(Vec2::new(7.0, 14.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.0, color);
                ui.label(RichText::new(text).small().color(TEXT));
            });
        })
        .response
}

/// An "i" in a ring with a tooltip: the place for the longer explanations.
pub fn info(ui: &mut egui::Ui, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    let c = if resp.hovered() { ACCENT } else { MUTED };
    let p = ui.painter();
    p.circle_stroke(rect.center(), 7.0, Stroke::new(1.2, c));
    p.text(rect.center() + Vec2::new(0.0, 0.5), egui::Align2::CENTER_CENTER, "i", FontId::proportional(11.0), c);
    resp.on_hover_text(text)
}

/// The step indicator: a dot per page joined by a line; done = accent ring with a tick, current = accent dot,
/// later = grey ring.
pub fn steps(ui: &mut egui::Ui, names: &[&str], current: usize) {
    let w = ui.available_width().min(640.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
    let p = ui.painter();
    let n = names.len().max(2);
    let x0 = rect.left() + 28.0;
    let dx = (rect.width() - 56.0) / (n - 1) as f32;
    let y = rect.top() + 10.0;
    for i in 0..names.len() {
        let x = x0 + dx * i as f32;
        if i + 1 < names.len() {
            let c = if i < current { ACCENT.gamma_multiply(0.7) } else { CARD_LINE };
            p.line_segment([Pos2::new(x + 9.0, y), Pos2::new(x + dx - 9.0, y)], Stroke::new(2.0, c));
        }
        let center = Pos2::new(x, y);
        if i < current {
            p.circle_filled(center, 7.0, ACCENT.gamma_multiply(0.25));
            p.circle_stroke(center, 7.0, Stroke::new(1.5, ACCENT));
            p.line_segment([center + Vec2::new(-3.2, 0.2), center + Vec2::new(-0.8, 2.6)], Stroke::new(1.6, ACCENT));
            p.line_segment([center + Vec2::new(-0.8, 2.6), center + Vec2::new(3.4, -2.4)], Stroke::new(1.6, ACCENT));
        } else if i == current {
            p.circle_filled(center, 8.0, ACCENT);
        } else {
            p.circle_stroke(center, 6.0, Stroke::new(1.5, MUTED));
        }
        let (color, font) = if i == current { (TEXT, FontId::proportional(12.5)) } else { (WEAK, FontId::proportional(12.0)) };
        p.text(Pos2::new(x, y + 13.0), egui::Align2::CENTER_TOP, names[i], font, color);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepIcon {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

/// A step's state as a small painted icon: tick, accent dot, cross, grey ring or dash.
pub fn step_icon(ui: &mut egui::Ui, kind: StepIcon) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match kind {
        StepIcon::Done => {
            p.line_segment([c + Vec2::new(-4.5, 0.0), c + Vec2::new(-1.2, 3.4)], Stroke::new(2.0, GREEN));
            p.line_segment([c + Vec2::new(-1.2, 3.4), c + Vec2::new(4.8, -3.6)], Stroke::new(2.0, GREEN));
        }
        StepIcon::Running => {
            p.circle_filled(c, 5.0, ACCENT);
            p.circle_stroke(c, 7.0, Stroke::new(1.0, ACCENT.gamma_multiply(0.5)));
        }
        StepIcon::Failed => {
            p.line_segment([c + Vec2::new(-4.0, -4.0), c + Vec2::new(4.0, 4.0)], Stroke::new(2.0, RED));
            p.line_segment([c + Vec2::new(-4.0, 4.0), c + Vec2::new(4.0, -4.0)], Stroke::new(2.0, RED));
        }
        StepIcon::Pending => {
            p.circle_stroke(c, 4.5, Stroke::new(1.2, MUTED));
        }
        StepIcon::Skipped => {
            p.line_segment([c + Vec2::new(-4.0, 0.0), c + Vec2::new(4.0, 0.0)], Stroke::new(1.5, MUTED));
        }
    }
    resp
}