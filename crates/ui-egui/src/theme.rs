//! Design tokens. Colours follow the same dark-UI family as EffectCraft (estimated, not copied
//! from Adobe artwork) so the two apps feel like siblings.

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
    pub tab_text: Color32,
    pub tab_text_active: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
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
    pub fn for_kind(kind: ThemeKind) -> Self {
        let dark = Tokens {
            kind,
            app_bg: Color32::from_rgb(0x12, 0x12, 0x12),
            header_bg: Color32::from_rgb(0x1f, 0x1f, 0x1f),
            panel_bg: Color32::from_rgb(0x23, 0x23, 0x23),
            tab_text: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            tab_text_active: Color32::from_rgb(0xe3, 0xe3, 0xe3),
            accent: Color32::from_rgb(0x2d, 0x8c, 0xeb),
            accent_hover: Color32::from_rgb(0x4a, 0xa0, 0xf5),
            text: Color32::from_rgb(0xc8, 0xc8, 0xc8),
            text_dim: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            text_faint: Color32::from_rgb(0x8c, 0x8c, 0x8c),
            hover: Color32::from_rgb(0x30, 0x30, 0x30),
            pressed: Color32::from_rgb(0x45, 0x45, 0x45),
            field_bg: Color32::from_rgb(0x17, 0x17, 0x17),
            field_border: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            separator: Color32::from_rgb(0x34, 0x34, 0x34),
            row: Color32::from_rgb(0x23, 0x23, 0x23),
            row_alt: Color32::from_rgb(0x27, 0x27, 0x27),
            row_selected: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            cache_green: Color32::from_rgb(0x3c, 0xa6, 0x4c),
            danger: Color32::from_rgb(0xe0, 0x4a, 0x3c),
            warning: Color32::from_rgb(0xe8, 0x9a, 0x2c),
            radius: 6.0,
            radius_sm: 3.0,
            gap: 4.0,
            row_h: 28.0,
        };
        match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Darker => Tokens {
                app_bg: Color32::from_rgb(0x08, 0x08, 0x08),
                header_bg: Color32::from_rgb(0x16, 0x16, 0x16),
                panel_bg: Color32::from_rgb(0x19, 0x19, 0x19),
                row: Color32::from_rgb(0x19, 0x19, 0x19),
                row_alt: Color32::from_rgb(0x1d, 0x1d, 0x1d),
                field_bg: Color32::from_rgb(0x0e, 0x0e, 0x0e),
                ..dark
            },
            ThemeKind::Light => Tokens {
                app_bg: Color32::from_rgb(0xb4, 0xb4, 0xb4),
                header_bg: Color32::from_rgb(0xd6, 0xd6, 0xd6),
                panel_bg: Color32::from_rgb(0xe6, 0xe6, 0xe6),
                tab_text: Color32::from_rgb(0x5a, 0x5a, 0x5a),
                tab_text_active: Color32::from_rgb(0x14, 0x14, 0x14),
                text: Color32::from_rgb(0x22, 0x22, 0x22),
                text_dim: Color32::from_rgb(0x5a, 0x5a, 0x5a),
                hover: Color32::from_rgb(0xd0, 0xd0, 0xd0),
                pressed: Color32::from_rgb(0xc0, 0xc0, 0xc0),
                field_bg: Color32::from_rgb(0xfa, 0xfa, 0xfa),
                field_border: Color32::from_rgb(0xaa, 0xaa, 0xaa),
                separator: Color32::from_rgb(0xc8, 0xc8, 0xc8),
                row: Color32::from_rgb(0xe6, 0xe6, 0xe6),
                row_alt: Color32::from_rgb(0xde, 0xde, 0xde),
                row_selected: Color32::from_rgb(0xaa, 0xc8, 0xf0),
                ..dark
            },
        }
    }
}

pub fn apply(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.kind == ThemeKind::Light { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.extreme_bg_color = t.field_bg;
    v.faint_bg_color = t.row_alt;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = t.accent;
    v.window_stroke = Stroke::new(1.0_f32, t.field_border);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.widgets.inactive.bg_fill = t.field_bg;
    v.widgets.inactive.weak_bg_fill = t.header_bg;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t.field_border);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.active.bg_fill = t.pressed;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(16.0));
        s.animation_time = 0.08;
    });
}
