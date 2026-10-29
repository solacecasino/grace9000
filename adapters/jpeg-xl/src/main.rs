//! JPEG XL, a grace9000 adapter: `jpeg-xl <file.jxl> <out.pam>`.
//!
//! Decodes with jxl-oxide (pure Rust), converted to sRGB (through moxcms
//! when the photo carries its own color profile), turned the way the file
//! says, and written as 16-bit PAM for grace9000 to read.

use std::io::Write;
use std::path::Path;

use jxl_oxide::{EnumColourEncoding, JxlImage, Moxcms, RenderingIntent};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: jpeg-xl <file.jxl> <out.pam>");
        std::process::exit(2);
    }
    if let Err(e) = convert(Path::new(&args[0]), Path::new(&args[1])) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn convert(input: &Path, output: &Path) -> Result<(), String> {
    let mut image = JxlImage::builder().open(input).map_err(|e| format!("not a JPEG XL file this reads: {e}"))?;
    image.set_cms(Moxcms);
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let render = image.render_frame(0).map_err(|e| e.to_string())?;
    let mut stream = render.stream();
    let (w, h, ch) = (stream.width() as usize, stream.height() as usize, stream.channels() as usize);
    let mut buf = vec![0f32; w * h * ch];
    stream.write_to_buffer(&mut buf);
    write_pam(output, w, h, ch, &buf)
}

/// Gray, gray and alpha, color, or color and alpha, as 16-bit RGBA PAM.
fn write_pam(path: &Path, w: usize, h: usize, ch: usize, px: &[f32]) -> Result<(), String> {
    let mut bytes = format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 65535\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
    bytes.reserve(w * h * 8);
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_be_bytes();
    for p in px.chunks_exact(ch.max(1)) {
        let rgba = match ch {
            1 => [p[0], p[0], p[0], 1.0],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 1.0],
            _ => [p[0], p[1], p[2], p[3]],
        };
        for v in rgba {
            bytes.extend(q(v));
        }
    }
    std::fs::File::create(path).and_then(|mut f| f.write_all(&bytes)).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}
