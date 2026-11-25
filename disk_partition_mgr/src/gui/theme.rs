//! Giao diện: bảng màu, font tiếng Việt, style chung.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(11, 15, 21);
pub const PANEL: Color32 = Color32::from_rgb(17, 22, 31);
pub const CARD: Color32 = Color32::from_rgb(23, 30, 41);
pub const CARD_HOVER: Color32 = Color32::from_rgb(31, 40, 54);
pub const BORDER: Color32 = Color32::from_rgb(43, 53, 69);
pub const TEXT: Color32 = Color32::from_rgb(228, 235, 244);
pub const MUTED: Color32 = Color32::from_rgb(139, 153, 173);
pub const ACCENT: Color32 = Color32::from_rgb(56, 189, 248);
pub const ACCENT_DEEP: Color32 = Color32::from_rgb(14, 116, 144);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);
pub const RED_DEEP: Color32 = Color32::from_rgb(127, 29, 29);
pub const PURPLE: Color32 = Color32::from_rgb(167, 139, 250);
pub const BLUE: Color32 = Color32::from_rgb(96, 165, 250);

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = Color32::from_rgb(8, 11, 16);
    style.visuals.faint_bg_color = CARD;
    style.visuals.code_bg_color = Color32::from_rgb(9, 13, 19);
    style.visuals.selection.bg_fill = ACCENT_DEEP;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.warn_fg_color = AMBER;
    style.visuals.error_fg_color = RED;
    style.visuals.override_text_color = Some(TEXT);

    let w = &mut style.visuals.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.noninteractive.corner_radius = CornerRadius::same(6);

    w.inactive.bg_fill = CARD;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.inactive.corner_radius = CornerRadius::same(6);

    w.hovered.bg_fill = CARD_HOVER;
    w.hovered.weak_bg_fill = CARD_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_DEEP);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.hovered.corner_radius = CornerRadius::same(6);

    w.active.bg_fill = ACCENT_DEEP;
    w.active.weak_bg_fill = ACCENT_DEEP;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.active.corner_radius = CornerRadius::same(6);

    w.open.bg_fill = CARD_HOVER;
    w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT_DEEP);

    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 9.0;

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(21.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();

    ctx.set_style(style);
}

/// Nạp font hệ thống Windows để hiển thị đầy đủ dấu tiếng Việt.
fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\tahoma.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\calibri.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "perfectds".to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "perfectds".to_owned());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("perfectds".to_owned());
            ctx.set_fonts(fonts);
            return;
        }
    }
}
