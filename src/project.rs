//! The document: one photo and the edit made to it. The photo itself is
//! never changed; the edit is a list of steps and a crop, saved as readable
//! JSON in a `.grace` file beside it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A `.grace` file, and session.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// The format this file was written in; files without it are format 1.
    #[serde(default)]
    pub grace9000: u32,
    /// The photo, as it was when opened.
    #[serde(default)]
    pub photo: Option<PathBuf>,
    #[serde(default)]
    pub edit: Edit,
    /// Only in session.json: the `.grace` file this came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

/// What's done to the photo: the steps, top to bottom, then where it's
/// turned, straightened and cropped.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Quarter turns clockwise, 0 to 3.
    #[serde(default)]
    pub turns: u8,
    /// Mirrored left to right, or top to bottom, after turning.
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    /// Degrees, -45 to 45, clockwise.
    #[serde(default)]
    pub straighten: f32,
    /// The part kept, in fractions of the turned photo; None: all of it.
    #[serde(default)]
    pub crop: Option<Crop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    pub const ALL: Crop = Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    /// Kept inside the photo, and at least a sliver.
    pub fn tidy(self) -> Crop {
        let w = self.w.clamp(0.002, 1.0);
        let h = self.h.clamp(0.002, 1.0);
        Crop { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }

    pub fn is_all(&self) -> bool {
        self.x <= 1e-4 && self.y <= 1e-4 && self.w >= 1.0 - 1e-4 && self.h >= 1.0 - 1e-4
    }
}

/// One adjustment. Steps of a kind this version doesn't know (from a newer
/// grace9000, or an adapter) are kept as they are and skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub kind: String,
    #[serde(default = "yes")]
    pub on: bool,
    /// The step's values by name; a missing one is its default.
    #[serde(default)]
    pub values: BTreeMap<String, f32>,
    /// A curve's points, (in, out) from 0 to 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f32; 2]>,
}

fn yes() -> bool {
    true
}

impl Step {
    pub fn new(kind: &str) -> Self {
        let points = if kind == "curve" { vec![[0.0, 0.0], [1.0, 1.0]] } else { Vec::new() };
        Self { kind: kind.to_owned(), on: true, values: BTreeMap::new(), points }
    }

    /// A value, or its default.
    pub fn get(&self, key: &str) -> f32 {
        if let Some(v) = self.values.get(key) {
            return *v;
        }
        crate::develop::kind(&self.kind).and_then(|k| k.params.iter().find(|p| p.key == key)).map_or(0.0, |p| p.default)
    }

    /// Every value back to its default.
    pub fn reset(&mut self) {
        self.values.clear();
        if self.kind == "curve" {
            self.points = vec![[0.0, 0.0], [1.0, 1.0]];
        }
    }
}

impl Edit {
    /// The photo's size once turned.
    pub fn turned(&self, w: usize, h: usize) -> (usize, usize) {
        if self.turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    pub fn crop(&self) -> Crop {
        self.crop.unwrap_or(Crop::ALL)
    }

    /// The size of what's kept, in the photo's own pixels.
    pub fn size(&self, w: usize, h: usize) -> (f32, f32) {
        let (tw, th) = self.turned(w, h);
        let c = self.crop();
        ((tw as f32 * c.w).max(1.0), (th as f32 * c.h).max(1.0))
    }

    /// Nothing done to it at all.
    pub fn is_untouched(&self) -> bool {
        *self == Edit::default()
    }

    /// Values kept in range, a crop kept inside the photo: for files and
    /// adapters that say otherwise.
    pub fn tidy(&mut self) {
        self.turns %= 4;
        self.straighten = if self.straighten.is_finite() { self.straighten.clamp(-45.0, 45.0) } else { 0.0 };
        self.crop = self.crop.map(Crop::tidy).filter(|c| !c.is_all());
        for s in &mut self.steps {
            if let Some(k) = crate::develop::kind(&s.kind) {
                for p in k.params {
                    if let Some(v) = s.values.get_mut(p.key) {
                        *v = if v.is_finite() { v.clamp(p.min, p.max) } else { p.default };
                    }
                }
            }
            for pt in &mut s.points {
                pt[0] = pt[0].clamp(0.0, 1.0);
                pt[1] = pt[1].clamp(0.0, 1.0);
            }
            s.points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_hand_written_files_read() {
        // The fewest fields, and unknown ones from a newer version.
        let p: Project = serde_json::from_str(r#"{"photo":"/a.jpg","edit":{"steps":[{"kind":"exposure","values":{"ev":1.5}},{"kind":"from the future","glow":3}]},"later":1}"#).unwrap();
        assert_eq!(p.edit.steps.len(), 2);
        assert!(p.edit.steps[0].on);
        assert_eq!(p.edit.steps[0].get("ev"), 1.5);
        assert_eq!(p.edit.steps[1].get("anything"), 0.0);
        let p: Project = serde_json::from_str("{}").unwrap();
        assert!(p.edit.is_untouched());
    }

    #[test]
    fn tidy_keeps_things_in_range() {
        let mut e = Edit { turns: 5, straighten: 90.0, crop: Some(Crop { x: 0.9, y: -1.0, w: 0.5, h: 2.0 }), ..Edit::default() };
        let mut s = Step::new("exposure");
        s.values.insert("ev".into(), 99.0);
        e.steps.push(s);
        e.tidy();
        assert_eq!((e.turns, e.straighten), (1, 45.0));
        assert_eq!(e.steps[0].get("ev"), 5.0);
        assert_eq!(e.crop, Some(Crop { x: 0.5, y: 0.0, w: 0.5, h: 1.0 }));
        let mut e = Edit { crop: Some(Crop::ALL), ..Edit::default() };
        e.tidy();
        assert!(e.crop.is_none());
        assert_eq!(e.turned(4, 3), (4, 3));
        e.turns = 3;
        assert_eq!(e.turned(4, 3), (3, 4));
    }
}
