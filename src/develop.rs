//! Turning a photo and its edit into pixels: the steps, the turn, the
//! straightening and the crop.
//!
//! `KINDS` is the one list of step kinds: the Stack's Add menu, the Adjust
//! panel's sliders, adapters and files all read it.
//!
//! The maths is cheap on purpose:
//! - Turning, mirroring, straightening and cropping together are one
//!   affine map from an output pixel to the photo, so each pixel's place is
//!   two additions from its neighbour's.
//! - Pixels are read from the pyramid level closest to the output's size,
//!   blended in linear light, so a preview reads only about the pixels it
//!   shows.
//! - Every step is a per-pixel formula, applied in one pass; tables stand
//!   in for powers, and a curve is a table of 1025 values.
//! - Rows are split across every core.

use std::sync::Arc;

use crate::image::{Channel, Curves, Level, Photo, Pixels};
use crate::project::{Crop, Edit, Step};

/// A step's value, as the Adjust panel shows it.
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
}

pub struct Kind {
    pub id: &'static str,
    pub title: &'static str,
    pub what: &'static str,
    pub params: &'static [Param],
}

const fn p(key: &'static str, label: &'static str, min: f32, max: f32, default: f32, unit: &'static str) -> Param {
    Param { key, label, min, max, default, unit }
}

/// Every kind of step, in the Add menu's order.
pub const KINDS: &[Kind] = &[
    Kind { id: "exposure", title: "Exposure", what: "Brighter or darker, in stops", params: &[p("ev", "Exposure", -5.0, 5.0, 0.0, " EV")] },
    Kind {
        id: "white balance",
        title: "White Balance",
        what: "Warmer or cooler, greener or more magenta",
        params: &[p("temperature", "Temperature", -100.0, 100.0, 0.0, ""), p("tint", "Tint", -100.0, 100.0, 0.0, "")],
    },
    Kind { id: "contrast", title: "Contrast", what: "More or less difference between light and dark", params: &[p("amount", "Contrast", -100.0, 100.0, 0.0, "")] },
    Kind {
        id: "tone",
        title: "Highlights & Shadows",
        what: "Bring back bright parts, open up dark ones",
        params: &[p("highlights", "Highlights", -100.0, 100.0, 0.0, ""), p("shadows", "Shadows", -100.0, 100.0, 0.0, "")],
    },
    Kind {
        id: "levels",
        title: "Levels",
        what: "Set the black point, white point and midtones",
        params: &[p("black", "Black", 0.0, 0.9, 0.0, ""), p("white", "White", 0.1, 1.0, 1.0, ""), p("gamma", "Midtones", 0.2, 5.0, 1.0, "")],
    },
    Kind {
        id: "color",
        title: "Saturation",
        what: "More or less color; vibrance favors muted colors",
        params: &[p("saturation", "Saturation", -100.0, 100.0, 0.0, ""), p("vibrance", "Vibrance", -100.0, 100.0, 0.0, "")],
    },
    Kind { id: "curve", title: "Curve", what: "Any tone to any other: drag the points", params: &[] },
    Kind {
        id: "black and white",
        title: "Black & White",
        what: "Gray, mixed from red, green and blue",
        params: &[p("red", "Red", -200.0, 300.0, 30.0, "%"), p("green", "Green", -200.0, 300.0, 59.0, "%"), p("blue", "Blue", -200.0, 300.0, 11.0, "%")],
    },
    Kind {
        id: "vignette",
        title: "Vignette",
        what: "Darker (or lighter) toward the edges",
        params: &[p("amount", "Amount", -100.0, 100.0, -30.0, ""), p("size", "Size", 0.0, 100.0, 50.0, ""), p("feather", "Feather", 0.0, 100.0, 50.0, "")],
    },
];

pub fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// The part of the result to make: `scale` output pixels per photo pixel,
/// and the window `w`×`h` at (`x`, `y`) in the whole result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    /// The whole turned photo, crop ignored (while cropping).
    pub whole: bool,
    /// The steps left out (to compare with the original).
    pub plain: bool,
}

/// The whole result's size at `scale`.
pub fn full_size(photo_w: usize, photo_h: usize, edit: &Edit, scale: f32, whole: bool) -> (f32, f32) {
    let (tw, th) = edit.turned(photo_w, photo_h);
    let c = if whole { Crop::ALL } else { edit.crop() };
    (tw as f32 * c.w * scale, th as f32 * c.h * scale)
}

/// How much straightening `deg` zooms in so no corner comes up empty.
pub fn straighten_zoom(w: f32, h: f32, deg: f32) -> f32 {
    let a = deg.to_radians().abs();
    let long = (w / h).max(h / w);
    a.cos() + long * a.sin()
}

/// From a place in the whole result (in output pixels) to the photo's own
/// pixels: (x, y) = origin + ox · dx + oy · dy.
#[derive(Clone, Copy, Debug)]
struct Map {
    origin: [f32; 2],
    dx: [f32; 2],
    dy: [f32; 2],
}

fn map(photo_w: usize, photo_h: usize, edit: &Edit, scale: f32, whole: bool) -> Map {
    let (w, h) = (photo_w as f32, photo_h as f32);
    let (tw, th) = edit.turned(photo_w, photo_h);
    let (tw, th) = (tw as f32, th as f32);
    let c = if whole { Crop::ALL } else { edit.crop() };
    let zoom = straighten_zoom(tw, th, edit.straighten);
    let (sin, cos) = edit.straighten.to_radians().sin_cos();
    let f = |ox: f32, oy: f32| -> [f32; 2] {
        // Crop, then straighten about the middle (zoomed to fill).
        let (x, y) = (c.x * tw + ox / scale, c.y * th + oy / scale);
        let (dx, dy) = ((x - tw / 2.0) / zoom, (y - th / 2.0) / zoom);
        let (mut x, mut y) = (tw / 2.0 + cos * dx + sin * dy, th / 2.0 - sin * dx + cos * dy);
        // Mirrors, then turns, undone.
        if edit.flip_h {
            x = tw - x;
        }
        if edit.flip_v {
            y = th - y;
        }
        match edit.turns % 4 {
            1 => [y, h - x],
            2 => [w - x, h - y],
            3 => [w - y, x],
            _ => [x, y],
        }
    };
    let o = f(0.0, 0.0);
    let (a, b) = (f(1.0, 0.0), f(0.0, 1.0));
    Map { origin: o, dx: [a[0] - o[0], a[1] - o[1]], dy: [b[0] - o[0], b[1] - o[1]] }
}

/// A step made ready to run on a pixel.
enum Op {
    Gain([f32; 3]),
    Contrast(f32),
    Tone { highlights: f32, shadows: f32 },
    Levels { black: f32, range: f32, inv_gamma: f32 },
    Color { saturation: f32, vibrance: f32 },
    Curve(Arc<[f32]>),
    Mono([f32; 3]),
    Vignette { amount: f32, from: f32, to: f32 },
}

impl Op {
    /// Whether it works on light (linear) or on what the eye sees (gamma).
    fn linear(&self) -> bool {
        matches!(self, Op::Gain(_) | Op::Mono(_) | Op::Vignette { .. })
    }
}

/// The steps, ready.
pub struct Pipeline {
    ops: Vec<Op>,
    curves: Curves,
}

impl Pipeline {
    pub fn new(steps: &[Step]) -> Self {
        let ops = steps.iter().filter(|s| s.on).filter_map(compile).collect();
        Self { ops, curves: Curves::get() }
    }

    /// One pixel, in linear light, at (nx, ny) from -1 to 1 across the
    /// result; returns it gamma-encoded.
    #[inline]
    fn run(&self, mut c: [f32; 3], nx: f32, ny: f32) -> [f32; 3] {
        let k = &self.curves;
        let mut linear = true;
        for op in &self.ops {
            if op.linear() != linear {
                c = if linear { c.map(|v| k.gamma(v)) } else { c.map(|v| k.linear(v)) };
                linear = !linear;
            }
            c = apply(op, c, nx, ny);
        }
        if linear { c.map(|v| k.gamma(v)) } else { c }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn compile(s: &Step) -> Option<Op> {
    let v = |k: &str| s.get(k);
    Some(match s.kind.as_str() {
        "exposure" => Op::Gain([2f32.powf(v("ev")); 3]),
        "white balance" => {
            let (t, n) = (v("temperature") / 100.0, v("tint") / 100.0);
            let m = [2f32.powf(0.35 * t), 2f32.powf(-0.25 * n), 2f32.powf(-0.35 * t)];
            // Gray stays as bright.
            let y = luma(m);
            Op::Gain(m.map(|x| x / y))
        }
        "contrast" => Op::Contrast(v("amount") / 100.0),
        "tone" => Op::Tone { highlights: v("highlights") / 100.0, shadows: v("shadows") / 100.0 },
        "levels" => {
            let (black, white) = (v("black"), v("white"));
            Op::Levels { black, range: (white - black).max(0.01), inv_gamma: 1.0 / v("gamma").max(0.01) }
        }
        "color" => Op::Color { saturation: v("saturation") / 100.0, vibrance: v("vibrance") / 100.0 },
        "curve" => Op::Curve(curve_table(&s.points)),
        "black and white" => {
            let w = [v("red"), v("green"), v("blue")];
            let sum = w[0] + w[1] + w[2];
            let sum = if sum.abs() < 1.0 { 100.0 } else { sum };
            Op::Mono(w.map(|x| x / sum))
        }
        "vignette" => {
            let from = v("size") / 100.0 * 0.9;
            let to = from + (1.0 - from) * (0.05 + v("feather") / 100.0 * 0.95);
            Op::Vignette { amount: v("amount") / 100.0, from, to }
        }
        _ => return None,
    })
}

#[inline]
fn apply(op: &Op, c: [f32; 3], nx: f32, ny: f32) -> [f32; 3] {
    match op {
        Op::Gain(g) => [c[0] * g[0], c[1] * g[1], c[2] * g[2]],
        Op::Contrast(k) => {
            let k = *k;
            c.map(|x| {
                if k >= 0.0 {
                    if !(0.0..=1.0).contains(&x) {
                        return x;
                    }
                    let s = if x < 0.5 { 2.0 * x * x } else { 1.0 - 2.0 * (1.0 - x) * (1.0 - x) };
                    x + k * (s - x)
                } else {
                    0.5 + (x - 0.5) * (1.0 + 0.7 * k)
                }
            })
        }
        Op::Tone { highlights, shadows } => {
            let l = luma(c);
            let lc = l.clamp(0.0, 1.0);
            let ws = 1.0 - smoothstep(0.0, 0.6, lc);
            let wh = smoothstep(0.4, 1.0, lc);
            let mut n = l;
            n += if *shadows >= 0.0 { shadows * ws * 0.8 * (lc.sqrt() - lc) } else { l * shadows * ws * 0.6 };
            n += if *highlights >= 0.0 {
                let m = 1.0 - lc;
                highlights * wh * 0.8 * (m.sqrt() - m)
            } else {
                highlights * wh * 0.5 * (l - 0.4).max(0.0)
            };
            if l > 1e-4 { c.map(|x| x * n / l) } else { c.map(|x| x + n - l) }
        }
        Op::Levels { black, range, inv_gamma } => c.map(|x| {
            let x = (x - black) / range;
            if x > 0.0 { x.powf(*inv_gamma) } else { x }
        }),
        Op::Color { saturation, vibrance } => {
            let l = luma(c);
            let sat = (c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])).clamp(0.0, 1.0);
            let f = (1.0 + saturation + vibrance * (1.0 - sat)).max(0.0);
            c.map(|x| l + (x - l) * f)
        }
        Op::Curve(t) => c.map(|x| {
            let n = (t.len() - 1) as f32;
            let x = x.clamp(0.0, 1.0) * n;
            let i = (x as usize).min(t.len() - 2);
            t[i] + (t[i + 1] - t[i]) * (x - i as f32)
        }),
        Op::Mono(w) => [w[0] * c[0] + w[1] * c[1] + w[2] * c[2]; 3],
        Op::Vignette { amount, from, to } => {
            let d = ((nx * nx + ny * ny) * 0.5).sqrt();
            let k = amount * smoothstep(*from, *to, d);
            if k < 0.0 { c.map(|x| x * (1.0 + k)) } else { c.map(|x| x + (1.0 - x).max(0.0) * k) }
        }
    }
}

/// A curve through `points` as 1025 values: a smooth line that never
/// overshoots between points (Fritsch–Carlson), flat past the ends.
pub fn curve_table(points: &[[f32; 2]]) -> Arc<[f32]> {
    let mut pts: Vec<[f32; 2]> = points.to_vec();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
    if pts.len() < 2 {
        return (0..=1024).map(|i| i as f32 / 1024.0).collect();
    }
    let n = pts.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (pts[i + 1][1] - pts[i][1]) / (pts[i + 1][0] - pts[i][0])).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    (0..=1024)
        .map(|k| {
            let x = k as f32 / 1024.0;
            if x <= pts[0][0] {
                return pts[0][1];
            }
            if x >= pts[n - 1][0] {
                return pts[n - 1][1];
            }
            let i = pts.windows(2).position(|w| x < w[1][0]).unwrap_or(n - 2);
            let h = pts[i + 1][0] - pts[i][0];
            let t = (x - pts[i][0]) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * pts[i][1] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * pts[i + 1][1] + (t3 - t2) * h * m[i + 1]
        })
        .collect()
}

/// Rows `y0..y0 + rows` of `view`, RGBA, each channel made by `out` from 0
/// to 1 (unpremultiplied).
pub fn render<T: Copy + Default + Send>(photo: &Photo, edit: &Edit, view: &View, y0: usize, rows: usize, out: fn(f32) -> T) -> Vec<T> {
    let mut buf = vec![T::default(); view.w * rows * 4];
    if view.w == 0 || rows == 0 {
        return buf;
    }
    let m = map(photo.width, photo.height, edit, view.scale, view.whole);
    let pipe = Pipeline::new(if view.plain { &[] } else { &edit.steps });
    let (fw, fh) = full_size(photo.width, photo.height, edit, view.scale, view.whole);
    let job = Job { m, pipe: &pipe, view, y0, fw, fh, out };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(rows.div_ceil(16)).max(1);
    let band = rows.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in buf.chunks_mut(band * view.w * 4).enumerate() {
            let job = &job;
            s.spawn(move || match &photo.pixels {
                Pixels::Eight(l) => job.rows(l, k * band, chunk),
                Pixels::Sixteen(l) => job.rows(l, k * band, chunk),
            });
        }
    });
    buf
}

struct Job<'a, T> {
    m: Map,
    pipe: &'a Pipeline,
    view: &'a View,
    y0: usize,
    fw: f32,
    fh: f32,
    out: fn(f32) -> T,
}

impl<T: Copy> Job<'_, T> {
    /// Fills `chunk`, the rows from `first` (counted from `y0`).
    fn rows<C: Channel>(&self, levels: &[Level<C>], first: usize, chunk: &mut [T]) {
        let v = self.view;
        let m = &self.m;
        // The level with about one of its pixels per output pixel (or more).
        let step = (m.dx[0].hypot(m.dx[1])).max(m.dy[0].hypot(m.dy[1]));
        let mut k = 0;
        while k + 1 < levels.len() && (2 << k) as f32 <= step * 1.0001 {
            k += 1;
        }
        let level = &levels[k];
        let table = C::table();
        let inv = 1.0 / (1 << k) as f32;
        // Past 2× the photo's own size, show its pixels as squares.
        let nearest = step < 0.5;
        for (r, row) in chunk.chunks_mut(v.w * 4).enumerate() {
            let oy = (v.y + (self.y0 + first + r) as i32) as f32 + 0.5;
            let ny = oy / self.fh * 2.0 - 1.0;
            let ox0 = v.x as f32 + 0.5;
            let mut sx = (m.origin[0] + ox0 * m.dx[0] + oy * m.dy[0]) * inv;
            let mut sy = (m.origin[1] + ox0 * m.dx[1] + oy * m.dy[1]) * inv;
            let (ddx, ddy) = (m.dx[0] * inv, m.dx[1] * inv);
            for (i, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let s = if nearest { nearest_px(level, table, sx, sy) } else { bilinear(level, table, sx, sy) };
                let o = if s[3] <= 0.0 {
                    [0.0; 4]
                } else {
                    let nx = (ox0 + i as f32) / self.fw * 2.0 - 1.0;
                    let c = self.pipe.run([s[0], s[1], s[2]], nx, ny);
                    [c[0], c[1], c[2], s[3]]
                };
                for ch in 0..4 {
                    px[ch] = (self.out)(o[ch]);
                }
                sx += ddx;
                sy += ddy;
            }
        }
    }
}

/// The level's color at (x, y) in its own pixels, in linear light (alpha
/// straight), blending the four nearest; transparent outside.
#[inline]
fn bilinear<C: Channel>(l: &Level<C>, t: &[f32], x: f32, y: f32) -> [f32; 4] {
    let (w, h) = (l.width as f32, l.height as f32);
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return [0.0; 4];
    }
    let (u, v) = ((x - 0.5).clamp(0.0, w - 1.0), (y - 0.5).clamp(0.0, h - 1.0));
    let (x0, y0) = (u as usize, v as usize);
    let (x1, y1) = ((x0 + 1).min(l.width - 1), (y0 + 1).min(l.height - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: usize, y: usize| -> [f32; 4] {
        let p = &l.px[(y * l.width + x) * 4..][..4];
        [t[p[0].index()], t[p[1].index()], t[p[2].index()], p[3].unit()]
    };
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut o = [0.0; 4];
    for ch in 0..4 {
        let top = a[ch] + (b[ch] - a[ch]) * fx;
        let bottom = c[ch] + (d[ch] - c[ch]) * fx;
        o[ch] = top + (bottom - top) * fy;
    }
    o
}

#[inline]
fn nearest_px<C: Channel>(l: &Level<C>, t: &[f32], x: f32, y: f32) -> [f32; 4] {
    if x < 0.0 || y < 0.0 || x >= l.width as f32 || y >= l.height as f32 {
        return [0.0; 4];
    }
    let p = &l.px[(y as usize * l.width + x as usize) * 4..][..4];
    [t[p[0].index()], t[p[1].index()], t[p[2].index()], p[3].unit()]
}

pub fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×2 photo: red, green, blue, white on top; black, gray below.
    fn photo() -> Photo {
        let mut px = Vec::new();
        for c in [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255], [0, 0, 0], [128, 128, 128], [0, 0, 0], [128, 128, 128]] {
            px.extend([c[0], c[1], c[2], 255]);
        }
        Photo::from_rgba8("t.png".into(), 4, 2, px, String::new())
    }

    fn all(p: &Photo, e: &Edit) -> Vec<[u8; 4]> {
        let (w, h) = full_size(p.width, p.height, e, 1.0, false);
        let v = View { scale: 1.0, x: 0, y: 0, w: w as usize, h: h as usize, whole: false, plain: false };
        render(p, e, &v, 0, v.h, to_u8).as_chunks::<4>().0.to_vec()
    }

    #[test]
    fn untouched_comes_back_the_same() {
        let p = photo();
        let px = all(&p, &Edit::default());
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[3], [255, 255, 255, 255]);
        assert_eq!(px[5], [128, 128, 128, 255]);
    }

    #[test]
    fn turns_and_mirrors() {
        let p = photo();
        let e = Edit { turns: 1, ..Edit::default() };
        let px = all(&p, &e);
        // Turned clockwise: 2 wide, 4 tall; the top row is the left column,
        // bottom to top.
        assert_eq!(px.len(), 8);
        assert_eq!(px[0], [0, 0, 0, 255]);
        assert_eq!(px[1], [255, 0, 0, 255]);
        let e = Edit { flip_h: true, ..Edit::default() };
        assert_eq!(all(&p, &e)[0], [255, 255, 255, 255]);
        let e = Edit { turns: 2, ..Edit::default() };
        assert_eq!(all(&p, &e)[0], [128, 128, 128, 255]);
        let e = Edit { turns: 3, ..Edit::default() };
        assert_eq!(all(&p, &e)[0], [255, 255, 255, 255]);
    }

    #[test]
    fn crops() {
        let p = photo();
        let e = Edit { crop: Some(Crop { x: 0.25, y: 0.5, w: 0.5, h: 0.5 }), ..Edit::default() };
        assert_eq!(all(&p, &e), [[128, 128, 128, 255], [0, 0, 0, 255]]);
    }

    #[test]
    fn steps() {
        let p = photo();
        let mut e = Edit::default();
        let mut s = Step::new("exposure");
        s.values.insert("ev".into(), 1.0);
        e.steps.push(s);
        // Mid gray (0.216 light) doubled is 0.43 light, 176 stored.
        assert_eq!(all(&p, &e)[5][0], 176);
        e.steps[0].on = false;
        assert_eq!(all(&p, &e)[5][0], 128);
        let e = Edit { steps: vec![Step::new("black and white")], ..Edit::default() };
        let px = all(&p, &e);
        assert!(px[0][0] == px[0][1] && px[0][1] == px[0][2]);
        assert_eq!(px[3], [255, 255, 255, 255]);
        // An unknown kind is skipped.
        let e = Edit { steps: vec![Step::new("from the future")], ..Edit::default() };
        assert_eq!(all(&p, &e)[0], [255, 0, 0, 255]);
        // A flat curve at the middle makes everything mid.
        let mut s = Step::new("curve");
        s.points = vec![[0.0, 0.5], [1.0, 0.5]];
        let e = Edit { steps: vec![s], ..Edit::default() };
        assert!(all(&p, &e).iter().all(|c| c[0] == 128 && c[1] == 128));
    }

    #[test]
    fn neutral_steps_change_nothing() {
        let p = photo();
        for k in KINDS.iter().filter(|k| !matches!(k.id, "black and white" | "vignette")) {
            let e = Edit { steps: vec![Step::new(k.id)], ..Edit::default() };
            assert_eq!(all(&p, &e), all(&p, &Edit::default()), "{}", k.id);
        }
    }

    #[test]
    fn curves_pass_through_their_points_without_overshoot() {
        let t = curve_table(&[[0.0, 0.0], [0.25, 0.4], [0.5, 0.45], [1.0, 1.0]]);
        assert!((t[256] - 0.4).abs() < 1e-3 && (t[512] - 0.45).abs() < 1e-3);
        assert!(t.windows(2).all(|w| w[1] >= w[0] - 1e-6));
        let t = curve_table(&[[0.2, 0.1], [0.8, 0.9]]);
        assert_eq!((t[0], t[1024]), (0.1, 0.9));
    }

    #[test]
    fn straightening_fills_the_frame() {
        let p = Photo::from_rgba8("t".into(), 40, 20, vec![255; 40 * 20 * 4], String::new());
        let e = Edit { straighten: 10.0, ..Edit::default() };
        assert!(all(&p, &e).iter().all(|c| c[3] == 255));
        assert!((straighten_zoom(4.0, 2.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn windows_and_scales() {
        let p = photo();
        // The right half at 1:1, and the whole at half size.
        let v = View { scale: 1.0, x: 2, y: 0, w: 2, h: 2, whole: false, plain: false };
        let px = render(&p, &Edit::default(), &v, 0, 2, to_u8);
        assert_eq!(&px[..4], &[0, 0, 255, 255]);
        let v = View { scale: 0.5, x: 0, y: 0, w: 2, h: 1, whole: false, plain: false };
        assert_eq!(render(&p, &Edit::default(), &v, 0, 1, to_u16).len(), 8);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// cargo test --release bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_preview() {
        let p = Photo::from_rgba8("t".into(), 3000, 2000, (0..3000 * 2000 * 4).map(|i| (i * 7 % 251) as u8).collect(), String::new());
        let mut e = Edit { straighten: 1.5, crop: Some(Crop { x: 0.04, y: 0.06, w: 0.92, h: 0.86 }), ..Edit::default() };
        for k in ["exposure", "tone", "white balance", "curve", "color", "vignette"] {
            let mut s = Step::new(k);
            if let Some(kind) = kind(k) {
                for p in kind.params {
                    s.values.insert(p.key.into(), p.default + (p.max - p.default) * 0.2);
                }
            }
            e.steps.push(s);
        }
        let v = View { scale: 0.53, x: 0, y: 0, w: 1470, h: 916, whole: false, plain: false };
        for steps in [0, 6] {
            let mut e = e.clone();
            e.steps.truncate(steps);
            let t = std::time::Instant::now();
            for _ in 0..10 {
                std::hint::black_box(render(&p, &e, &v, 0, v.h, to_u8));
            }
            println!("{steps} steps: {:.1} ms", t.elapsed().as_secs_f64() * 100.0);
        }
    }
}
