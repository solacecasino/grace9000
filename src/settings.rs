//! settings.toml. Every field has a default, so a missing or half-written
//! file still works, and unknown keys are ignored so old files keep working.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Path to ffmpeg; empty: look for it.
    pub ffmpeg: String,
    pub export: Export,
    pub edit: Edit,
    pub look: Look,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Export {
    /// The format File → Export suggests: jpg, png, webp, tif.
    pub format: String,
    /// JPEG: 1 to 100.
    pub jpeg_quality: u32,
    /// WebP: 1 to 100.
    pub webp_quality: u32,
    /// PNG: 8 or 16; 0: the photo's own.
    pub png_bits: u32,
    /// Shrink so the long side is this many pixels; 0: full size.
    pub long_edge: u32,
    /// Passed to ffmpeg just before the output file.
    pub extra_args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// How much each zoom in or out changes the size.
    pub zoom_step: f32,
    /// How far each straighten key turns, in degrees.
    pub straighten_step: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Look {
    pub dark: bool,
    pub accent: String,
    pub font_size: f32,
    /// Each panel has a slim header to drag it by.
    pub panel_headers: bool,
    /// Behind the photo: "#rrggbb", or "checker" to show transparency.
    pub backdrop: String,
}

impl Default for Export {
    fn default() -> Self {
        Self { format: "jpg".into(), jpeg_quality: 92, webp_quality: 90, png_bits: 0, long_edge: 0, extra_args: Vec::new() }
    }
}

impl Default for Edit {
    fn default() -> Self {
        Self { zoom_step: 1.5, straighten_step: 0.5 }
    }
}

impl Default for Look {
    fn default() -> Self {
        Self { dark: true, accent: "#a98cff".into(), font_size: 13.0, panel_headers: true, backdrop: String::new() }
    }
}

/// "#rrggbb" as a color.
pub fn hex(text: &str) -> Option<egui::Color32> {
    let hex = text.trim().trim_start_matches('#');
    match (hex.len(), u32::from_str_radix(hex, 16)) {
        (6, Ok(v)) => Some(egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)),
        _ => None,
    }
}

impl Look {
    pub fn accent(&self) -> egui::Color32 {
        hex(&self.accent).unwrap_or(crate::theme::GRACE_VIOLET)
    }
}

pub const EXAMPLE: &str = r##"# grace9000 settings. Saved changes apply at once; delete a line to get
# its default back. Everything here is optional.

# Where ffmpeg is, if grace9000 can't find it on its own. It reads every
# photo but PNG, and writes every format but PNG.
# ffmpeg = "/opt/homebrew/bin/ffmpeg"

[export]
# What Export suggests: jpg, png, webp or tif. PNG is written by grace9000
# itself; the others by ffmpeg.
format = "jpg"
jpeg_quality = 92
webp_quality = 90
# PNG: 8 or 16 bits a channel; 0 keeps the photo's own.
png_bits = 0
# Shrink so the long side is this many pixels; 0 keeps the full size.
long_edge = 0
# Anything else for ffmpeg, put just before the output file.
# extra_args = ["-map_metadata", "-1"]

[edit]
# How much each zoom in or out changes the size.
zoom_step = 1.5
# How far [ and ] turn while straightening, in degrees.
straighten_step = 0.5

[look]
dark = true
# The one signal color: the selected step, the crop, the Export button.
accent = "#a98cff"
font_size = 13
# A slim header on each panel, to drag it somewhere else. Off: use
# Edit Layout (cmd+L) to move panels.
panel_headers = true
# Behind the photo: a color like "#000000", or "checker" to see
# transparency. Empty: the panel's own well.
backdrop = ""
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_parses_and_half_files_work() {
        let s: Settings = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(s.export.jpeg_quality, 92);
        let s: Settings = toml::from_str("[export]\nlong_edge = 2048\nsomething_new = 1").unwrap();
        assert_eq!((s.export.long_edge, s.export.format.as_str(), s.edit.zoom_step), (2048, "jpg", 1.5));
        assert_eq!(Look { accent: "#ff0000".into(), ..Look::default() }.accent(), egui::Color32::RED);
        assert_eq!(Look { accent: "nope".into(), ..Look::default() }.accent(), crate::theme::GRACE_VIOLET);
    }
}
