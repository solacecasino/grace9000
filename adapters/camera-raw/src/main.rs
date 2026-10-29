//! Camera Raw, a grace9000 adapter: `camera-raw <raw file> <out.pam>`.
//!
//! Decodes a camera's raw file with rawler (from dnglab), develops it the
//! plain way (demosaic, the camera's own white balance and color matrix,
//! sRGB), turns it the way the camera says it was held, and writes it as
//! 16-bit PAM for grace9000 to read. Everything after that is grace9000's
//! steps.

use std::io::Write;
use std::path::Path;

use rawler::decoders::RawDecodeParams;
use rawler::imgop::develop::{Intermediate, RawDevelop};
use rawler::rawsource::RawSource;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: camera-raw <raw file> <out.pam>");
        std::process::exit(2);
    }
    if let Err(e) = convert(Path::new(&args[0]), Path::new(&args[1])) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn convert(input: &Path, output: &Path) -> Result<(), String> {
    let source = RawSource::new(input).map_err(|e| format!("couldn't read it: {e}"))?;
    let decoder = rawler::get_decoder(&source).map_err(|e| format!("not a raw file this knows: {e}"))?;
    let params = RawDecodeParams { image_index: 0 };
    let raw = decoder.raw_image(&source, &params, false).map_err(|e| e.to_string())?;
    let orientation = decoder.raw_metadata(&source, &params).ok().and_then(|m| m.exif.orientation).unwrap_or(1);
    let developed = RawDevelop::default().develop_intermediate(&raw).map_err(|e| e.to_string())?;
    let (w, h, rgb) = match developed {
        Intermediate::ThreeColor(p) => (p.width, p.height, p.data),
        Intermediate::Monochrome(p) => (p.width, p.height, p.data.iter().map(|&v| [v; 3]).collect()),
        Intermediate::FourColor(p) => (p.width, p.height, p.data.iter().map(|v| [v[0], v[1], v[2]]).collect()),
    };
    let (w, h, rgb) = orient(w, h, rgb, orientation);
    write_pam(output, w, h, &rgb)
}

/// The pixels turned and mirrored the way EXIF's orientation (1 to 8) says.
fn orient(w: usize, h: usize, px: Vec<[f32; 3]>, o: u16) -> (usize, usize, Vec<[f32; 3]>) {
    if !(2..=8).contains(&o) {
        return (w, h, px);
    }
    let swap = o >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(px.len());
    for y in 0..oh {
        for x in 0..ow {
            // Where this output pixel comes from in the stored picture.
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x),
            };
            out.push(px[sy * w + sx]);
        }
    }
    (ow, oh, out)
}

fn write_pam(path: &Path, w: usize, h: usize, rgb: &[[f32; 3]]) -> Result<(), String> {
    let mut bytes = format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 65535\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
    bytes.reserve(w * h * 8);
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_be_bytes();
    for p in rgb {
        for v in p {
            bytes.extend(q(*v));
        }
        bytes.extend(u16::MAX.to_be_bytes());
    }
    std::fs::File::create(path).and_then(|mut f| f.write_all(&bytes)).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientations() {
        // 2×1: a, b. Orientation 6 (held turned right) shows it 1×2: a above b.
        let px = vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let (w, h, o) = orient(2, 1, px.clone(), 6);
        assert_eq!((w, h), (1, 2));
        assert_eq!(o, px);
        let (_, _, o) = orient(2, 1, px.clone(), 8);
        assert_eq!(o, vec![px[1], px[0]]);
        let (_, _, o) = orient(2, 1, px.clone(), 3);
        assert_eq!(o, vec![px[1], px[0]]);
        assert_eq!(orient(2, 1, px.clone(), 1).2, px);
    }
}
