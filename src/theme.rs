//! grace9000's look: a few named colors every surface draws with.
//!
//! The same family as splice9000 and wave9000: quiet graphite with one
//! signal color, here grace violet, used only for what's live: the selected
//! step, the crop, the Export button, a divider being dragged. The photo is
//! the only other color on screen. Panels sit edge to edge, divided by a
//! single hairline.

use egui::Color32;

use crate::settings::Look;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// The signal color.
    pub accent: Color32,
    /// Text drawn on the accent.
    pub on_accent: Color32,
    /// Panel bodies.
    pub panel: Color32,
    /// Panel headers and the top strip.
    pub header: Color32,
    /// The hairlines between panels.
    pub line: Color32,
    /// Behind the photo.
    pub well: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub dark: bool,
}

pub const GRACE_VIOLET: Color32 = Color32::from_rgb(0xa9, 0x8c, 0xff);

impl Palette {
    pub fn new(look: &Look) -> Self {
        let accent = look.accent();
        if look.dark {
            Self {
                accent,
                on_accent: Color32::from_rgb(0x14, 0x0b, 0x08),
                panel: Color32::from_rgb(0x17, 0x18, 0x1b),
                header: Color32::from_rgb(0x12, 0x13, 0x15),
                line: Color32::from_rgb(0x2a, 0x2c, 0x31),
                well: Color32::from_rgb(0x0c, 0x0d, 0x0f),
                text: Color32::from_rgb(0xe8, 0xe9, 0xeb),
                weak: Color32::from_rgb(0x84, 0x88, 0x90),
                dark: true,
            }
        } else {
            Self {
                accent,
                on_accent: Color32::WHITE,
                panel: Color32::from_rgb(0xfa, 0xfa, 0xf9),
                header: Color32::from_rgb(0xf0, 0xf0, 0xee),
                line: Color32::from_rgb(0xd9, 0xd9, 0xd6),
                well: Color32::from_rgb(0x1a, 0x1b, 0x1e),
                text: Color32::from_rgb(0x1c, 0x1d, 0x20),
                weak: Color32::from_rgb(0x6f, 0x72, 0x78),
                dark: false,
            }
        }
    }

    /// egui's own widgets in the same colors.
    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = self.panel;
        v.window_fill = self.panel;
        v.extreme_bg_color = self.well;
        v.faint_bg_color = self.header;
        v.window_stroke.color = self.line;
        v.override_text_color = None;
        v.selection.bg_fill = self.accent.gamma_multiply(0.35);
        v.selection.stroke.color = self.accent;
        v.hyperlink_color = self.accent;
        let w = &mut v.widgets;
        let quiet = if self.dark { Color32::from_rgb(0x22, 0x24, 0x28) } else { Color32::from_rgb(0xe8, 0xe8, 0xe5) };
        let hover = if self.dark { Color32::from_rgb(0x2c, 0x2f, 0x34) } else { Color32::from_rgb(0xdf, 0xdf, 0xdb) };
        for (state, fill) in [(&mut w.inactive, quiet), (&mut w.hovered, hover), (&mut w.active, hover), (&mut w.open, hover)] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.bg_stroke.color = Color32::TRANSPARENT;
            state.corner_radius = egui::CornerRadius::same(4);
        }
        w.noninteractive.bg_stroke.color = self.line;
        w.noninteractive.fg_stroke.color = self.text;
        w.inactive.fg_stroke.color = self.text;
        v
    }
}
