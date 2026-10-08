//! Palette B — Neutral warm dark.
//!
//! Cobalt `#2448F5` is a fill only: primary buttons, the active tab, progress bars,
//! and the selected-row edge. Links and file paths use the light tint `#8EA2FF`.
//! Never use Cobalt as small text.

use egui::{Color32, FontId, Stroke, TextStyle, Visuals};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    Dark,
    Darker,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    pub app_bg: Color32,
    pub header_bg: Color32,
    pub panel_bg: Color32,
    pub raised: Color32,
    pub tab_text: Color32,
    pub tab_text_active: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub link: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub text_disabled: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub field_bg: Color32,
    pub field_border: Color32,
    pub separator: Color32,
    pub row: Color32,
    pub row_alt: Color32,
    pub row_selected: Color32,
    pub cache_green: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub radius: f32,
    pub radius_sm: f32,
    pub gap: f32,
    pub row_h: f32,
}

impl Tokens {
    /// Approved Palette B (Neutral warm dark) — the default EncodeCraft theme.
    pub const fn palette_b() -> Self {
        Self {
            kind: ThemeKind::Dark,
            app_bg: Color32::from_rgb(0x1B, 0x1B, 0x1D),
            header_bg: Color32::from_rgb(0x1B, 0x1B, 0x1D),
            panel_bg: Color32::from_rgb(0x23, 0x23, 0x26),
            raised: Color32::from_rgb(0x2C, 0x2C, 0x30),
            tab_text: Color32::from_rgb(0xA0, 0xA0, 0xA8),
            tab_text_active: Color32::from_rgb(0xE6, 0xE6, 0xE8),
            accent: Color32::from_rgb(0x24, 0x48, 0xF5),
            accent_hover: Color32::from_rgb(0x3A, 0x5C, 0xFF),
            link: Color32::from_rgb(0x8E, 0xA2, 0xFF),
            text: Color32::from_rgb(0xE6, 0xE6, 0xE8),
            text_dim: Color32::from_rgb(0xA0, 0xA0, 0xA8),
            text_faint: Color32::from_rgb(0x6B, 0x6B, 0x73),
            text_disabled: Color32::from_rgb(0x6B, 0x6B, 0x73),
            hover: Color32::from_rgb(0x36, 0x36, 0x3B),
            pressed: Color32::from_rgb(0x36, 0x36, 0x3B),
            field_bg: Color32::from_rgb(0x2C, 0x2C, 0x30),
            field_border: Color32::from_rgb(0x3A, 0x3A, 0x40),
            separator: Color32::from_rgb(0x3A, 0x3A, 0x40),
            row: Color32::from_rgb(0x23, 0x23, 0x26),
            row_alt: Color32::from_rgb(0x2C, 0x2C, 0x30),
            row_selected: Color32::from_rgb(0x36, 0x36, 0x3B),
            cache_green: Color32::from_rgb(0x3F, 0xB9, 0x50),
            danger: Color32::from_rgb(0xF8, 0x51, 0x49),
            warning: Color32::from_rgb(0xD2, 0x99, 0x22),
            radius: 6.0,
            radius_sm: 3.0,
            gap: 4.0,
            row_h: 28.0,
        }
    }

    pub fn for_kind(kind: ThemeKind) -> Self {
        let dark = Self::palette_b();
        match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Darker => Tokens {
                kind,
                app_bg: Color32::from_rgb(0x12, 0x12, 0x14),
                header_bg: Color32::from_rgb(0x12, 0x12, 0x14),
                panel_bg: Color32::from_rgb(0x1A, 0x1A, 0x1C),
                row: Color32::from_rgb(0x1A, 0x1A, 0x1C),
                ..dark
            },
            ThemeKind::Light => Tokens {
                kind,
                app_bg: Color32::from_rgb(0xE8, 0xE8, 0xEA),
                header_bg: Color32::from_rgb(0xF2, 0xF2, 0xF4),
                panel_bg: Color32::from_rgb(0xF7, 0xF7, 0xF8),
                raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                tab_text: Color32::from_rgb(0x5A, 0x5A, 0x62),
                tab_text_active: Color32::from_rgb(0x1B, 0x1B, 0x1D),
                text: Color32::from_rgb(0x1B, 0x1B, 0x1D),
                text_dim: Color32::from_rgb(0x5A, 0x5A, 0x62),
                text_faint: Color32::from_rgb(0x8A, 0x8A, 0x92),
                text_disabled: Color32::from_rgb(0x8A, 0x8A, 0x92),
                hover: Color32::from_rgb(0xDC, 0xDC, 0xE2),
                pressed: Color32::from_rgb(0xD0, 0xD0, 0xD6),
                field_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                field_border: Color32::from_rgb(0xC4, 0xC4, 0xCA),
                separator: Color32::from_rgb(0xC4, 0xC4, 0xCA),
                row: Color32::from_rgb(0xF7, 0xF7, 0xF8),
                row_alt: Color32::from_rgb(0xEE, 0xEE, 0xF0),
                row_selected: Color32::from_rgb(0xDC, 0xDC, 0xE2),
                ..dark
            },
        }
    }
}

pub fn apply(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.kind == ThemeKind::Light { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.extreme_bg_color = t.raised;
    v.faint_bg_color = t.hover;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.hover;
    v.selection.stroke = Stroke::new(2.0_f32, t.accent);
    v.hyperlink_color = t.link;
    v.window_stroke = Stroke::new(1.0_f32, t.field_border);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.field_border);
    v.widgets.inactive.bg_fill = t.raised;
    v.widgets.inactive.weak_bg_fill = t.raised;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t.field_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t.field_border);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_stroke = Stroke::new(1.5_f32, t.accent);
    v.widgets.open.bg_fill = t.hover;
    v.widgets.open.weak_bg_fill = t.hover;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, t.accent);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.visuals.widgets.inactive.fg_stroke.color = t.text;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(16.0));
        s.animation_time = 0.08;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_b_matches_approved_mockup() {
        let t = Tokens::palette_b();
        assert_eq!(t.app_bg, Color32::from_rgb(0x1B, 0x1B, 0x1D));
        assert_eq!(t.panel_bg, Color32::from_rgb(0x23, 0x23, 0x26));
        assert_eq!(t.raised, Color32::from_rgb(0x2C, 0x2C, 0x30));
        assert_eq!(t.hover, Color32::from_rgb(0x36, 0x36, 0x3B));
        assert_eq!(t.row_selected, Color32::from_rgb(0x36, 0x36, 0x3B));
        assert_eq!(t.field_border, Color32::from_rgb(0x3A, 0x3A, 0x40));
        assert_eq!(t.text, Color32::from_rgb(0xE6, 0xE6, 0xE8));
        assert_eq!(t.text_dim, Color32::from_rgb(0xA0, 0xA0, 0xA8));
        assert_eq!(t.text_disabled, Color32::from_rgb(0x6B, 0x6B, 0x73));
        assert_eq!(t.accent, Color32::from_rgb(0x24, 0x48, 0xF5));
        assert_eq!(t.link, Color32::from_rgb(0x8E, 0xA2, 0xFF));
        assert_eq!(t.cache_green, Color32::from_rgb(0x3F, 0xB9, 0x50));
        assert_eq!(t.warning, Color32::from_rgb(0xD2, 0x99, 0x22));
        assert_eq!(t.danger, Color32::from_rgb(0xF8, 0x51, 0x49));
        assert_ne!(t.accent, t.link, "cobalt fill must stay distinct from link tint");
    }

    #[test]
    fn default_theme_is_palette_b() {
        assert_eq!(Tokens::for_kind(ThemeKind::Dark), Tokens::palette_b());
        let darker = Tokens::for_kind(ThemeKind::Darker);
        let light = Tokens::for_kind(ThemeKind::Light);
        assert_ne!(darker.app_bg, Tokens::palette_b().app_bg);
        assert_ne!(light.app_bg, Tokens::palette_b().app_bg);
    }
}
