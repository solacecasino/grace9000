//! Exporting: the same renderer as the preview, at full size, in strips of
//! rows so memory stays small, into a PNG written by grace9000 itself or
//! into ffmpeg's input for any other format.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::adapters::ExportSpec;
use crate::develop::{self, View};
use crate::image::{Photo, Tools, quiet};
use crate::project::Edit;
use crate::settings::Export;

/// How the picture becomes a file.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    /// 8 or 16 bits a channel.
    Png(u32),
    /// ffmpeg reads the picture from its input at this depth; these go
    /// between that and the output file.
    Ffmpeg { bits: u32, args: Vec<String> },
}

/// The formats grace9000 knows by their extension.
pub const EXTENSIONS: &[(&str, &str)] = &[("jpg", "JPEG"), ("png", "PNG"), ("webp", "WebP"), ("tif", "TIFF")];

/// How to write a file ending in `ext` from a photo of `bits`, as
/// settings.toml says, or as an adapter's format says. None: grace9000
/// doesn't know the extension.
pub fn how(ext: &str, s: &Export, bits: u32, spec: Option<&ExportSpec>) -> Option<How> {
    if let Some(spec) = spec {
        let deep = if spec.bits > 8 { 16 } else { 8 };
        let mut args = Vec::new();
        if !spec.filter.is_empty() {
            args.extend(["-vf".into(), spec.filter.clone()]);
        }
        match &spec.args {
            Some(a) => args.extend(a.iter().cloned()),
            None if spec.extension == "png" && spec.filter.is_empty() => return Some(How::Png(if spec.bits == 0 { png_bits(s, bits) } else { deep })),
            None => match how(&spec.extension, s, bits, None)? {
                How::Ffmpeg { args: a, .. } => args.extend(a),
                How::Png(_) => args.extend(["-c:v".into(), "png".into()]),
            },
        }
        return Some(How::Ffmpeg { bits: if spec.bits == 0 { bits } else { deep }, args });
    }
    let list = |l: &[&str]| -> Vec<String> { l.iter().map(|s| (*s).to_owned()).collect() };
    let mut args = match ext.to_lowercase().as_str() {
        "png" if s.extra_args.is_empty() => return Some(How::Png(png_bits(s, bits))),
        "png" => list(&["-c:v", "png"]),
        "jpg" | "jpeg" => {
            // ffmpeg's scale: 2 is best, 31 worst.
            let q = (31.0 - (s.jpeg_quality.clamp(1, 100) as f32 - 1.0) / 99.0 * 29.0).round() as u32;
            let mut a = list(&["-c:v", "mjpeg", "-pix_fmt", "yuvj444p", "-q:v"]);
            a.push(q.to_string());
            return Some(How::Ffmpeg { bits: 8, args: [a, s.extra_args.clone()].concat() });
        }
        "webp" => {
            let mut a = list(&["-c:v", "libwebp", "-quality"]);
            a.push(s.webp_quality.clamp(1, 100).to_string());
            return Some(How::Ffmpeg { bits: 8, args: [a, s.extra_args.clone()].concat() });
        }
        "tif" | "tiff" => list(&["-c:v", "tiff", "-pix_fmt", if bits > 8 { "rgb48le" } else { "rgb24" }]),
        _ => return None,
    };
    args.extend(s.extra_args.iter().cloned());
    Some(How::Ffmpeg { bits, args })
}

fn png_bits(s: &Export, bits: u32) -> u32 {
    match s.png_bits {
        8 => 8,
        16 => 16,
        _ => bits,
    }
}

/// The ffmpeg encoder a format needs, if any.
pub fn encoder(how: &How) -> Option<&str> {
    let How::Ffmpeg { args, .. } = how else { return None };
    args.windows(2).find(|w| w[0] == "-c:v").map(|w| w[1].as_str())
}

/// The result's size: the crop at the photo's own size, or shrunk so its
/// long side is `long_edge` (never enlarged).
pub fn size(photo: &Photo, edit: &Edit, long_edge: u32) -> (usize, usize, f32) {
    let (w, h) = edit.size(photo.width, photo.height);
    let scale = if long_edge > 0 { (long_edge as f32 / w.max(h)).min(1.0) } else { 1.0 };
    (((w * scale).round() as usize).max(1), ((h * scale).round() as usize).max(1), scale)
}

#[derive(Default)]
pub struct Progress {
    pub rows: usize,
    pub finished: Option<Result<String, String>>,
}

pub struct Job {
    pub out: PathBuf,
    pub total: usize,
    pub started: Instant,
    pub progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
}

/// Rows made at a time.
const STRIP: usize = 128;

impl Job {
    pub fn start(photo: Arc<Photo>, edit: Edit, long_edge: u32, tools: Option<Tools>, how: How, out: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (w, h, scale) = size(&photo, &edit, long_edge);
        let progress: Arc<Mutex<Progress>> = Arc::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c, path) = (progress.clone(), cancel.clone(), out.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let view = View { scale, x: 0, y: 0, w, h, whole: false, plain: false };
            let strips = |each: &mut dyn FnMut(&View, usize, usize) -> Result<(), String>| -> Result<(), String> {
                let mut y = 0;
                while y < h {
                    if c.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    let rows = STRIP.min(h - y);
                    each(&view, y, rows)?;
                    y += rows;
                    p.lock().unwrap_or_else(|e| e.into_inner()).rows = y;
                    wake();
                }
                Ok(())
            };
            let r = match &how {
                How::Png(bits) => write_png(&photo, &edit, &path, w, h, *bits, &strips),
                How::Ffmpeg { bits, args } => write_ffmpeg(&photo, &edit, tools.as_ref(), &path, w, h, *bits, args, &strips),
            };
            let r = r.map(|()| {
                let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                format!("{w}×{h}, {:.1} MB in {:.1} s", size as f64 / 1e6, t.elapsed().as_secs_f64())
            });
            if r.is_err() {
                let _ = std::fs::remove_file(&path);
            }
            p.lock().unwrap_or_else(|e| e.into_inner()).finished = Some(r);
            wake();
        });
        Self { out, total: h, started: Instant::now(), progress, cancel }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

type Strips<'a> = &'a dyn Fn(&mut dyn FnMut(&View, usize, usize) -> Result<(), String>) -> Result<(), String>;

fn write_png(photo: &Photo, edit: &Edit, path: &PathBuf, w: usize, h: usize, bits: u32, strips: Strips<'_>) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(if bits > 8 { png::BitDepth::Sixteen } else { png::BitDepth::Eight });
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
    strips(&mut |view, y, rows| {
        let bytes: Vec<u8> = if bits > 8 {
            develop::render(photo, edit, view, y, rows, develop::to_u16).iter().flat_map(|v| v.to_be_bytes()).collect()
        } else {
            develop::render(photo, edit, view, y, rows, develop::to_u8)
        };
        stream.write_all(&bytes).map_err(|e| e.to_string())
    })?;
    stream.finish().map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn write_ffmpeg(photo: &Photo, edit: &Edit, tools: Option<&Tools>, path: &PathBuf, w: usize, h: usize, bits: u32, args: &[String], strips: Strips<'_>) -> Result<(), String> {
    let tools = tools.ok_or("ffmpeg wasn't found; without it only PNG can be written")?;
    let deep = bits > 8;
    let mut child = quiet(Command::new(&tools.ffmpeg))
        .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", if deep { "rgba64le" } else { "rgba" }, "-s"])
        .arg(format!("{w}x{h}"))
        .args(["-color_trc", "iec61966-2-1", "-color_primaries", "bt709", "-i", "-", "-frames:v", "1", "-update", "1"])
        .args(args)
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    // Read what ffmpeg says as it goes, so it never blocks on a full pipe.
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let fed = strips(&mut |view, y, rows| {
        let bytes: Vec<u8> = if deep {
            develop::render(photo, edit, view, y, rows, develop::to_u16).iter().flat_map(|v| v.to_le_bytes()).collect()
        } else {
            develop::render(photo, edit, view, y, rows, develop::to_u8)
        };
        stdin.write_all(&bytes).map_err(|e| format!("ffmpeg stopped reading: {e}"))
    });
    drop(stdin);
    if fed.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let said = err.join().unwrap_or_default();
    fed?;
    if !status.success() {
        return Err(said.lines().last().unwrap_or("ffmpeg failed").to_owned());
    }
    for line in said.lines().filter(|l| !l.trim().is_empty()) {
        crate::log::info(format!("ffmpeg: {line}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_extension() {
        let s = Export::default();
        assert_eq!(how("png", &s, 16, None), Some(How::Png(16)));
        assert_eq!(how("png", &Export { png_bits: 8, ..Export::default() }, 16, None), Some(How::Png(8)));
        let jpg = how("JPG", &s, 16, None).unwrap();
        assert_eq!(encoder(&jpg), Some("mjpeg"));
        assert!(matches!(jpg, How::Ffmpeg { bits: 8, .. }));
        assert_eq!(encoder(&how("webp", &s, 8, None).unwrap()), Some("libwebp"));
        assert!(how("psd", &s, 8, None).is_none());
        // An adapter's format.
        let spec = ExportSpec { id: "a".into(), label: "A".into(), extension: "avif".into(), args: Some(vec!["-c:v".into(), "libaom-av1".into()]), filter: String::new(), long_edge: 0, bits: 0 };
        assert_eq!(encoder(&how("avif", &s, 8, Some(&spec)).unwrap()), Some("libaom-av1"));
        let spec = ExportSpec { extension: "jpg".into(), args: None, filter: "hue=s=0".into(), ..spec };
        let h = how("jpg", &s, 8, Some(&spec)).unwrap();
        let How::Ffmpeg { args, .. } = &h else { panic!() };
        assert_eq!(&args[..2], ["-vf", "hue=s=0"]);
        assert_eq!(encoder(&h), Some("mjpeg"));
    }

    #[test]
    fn writes_png_and_jpeg() {
        let dir = std::env::temp_dir().join(format!("grace9000-export-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let photo = Arc::new(Photo::from_rgba16("t.png".into(), 300, 200, vec![30000; 300 * 200 * 4], String::new()));
        let run = |name: &str, how: How, long: u32| {
            let out = dir.join(name);
            let (tx, rx) = std::sync::mpsc::channel();
            let job = Job::start(photo.clone(), Edit::default(), long, Tools::find(""), how, out.clone(), move || {
                let _ = tx.send(());
            });
            loop {
                let _ = rx.recv_timeout(std::time::Duration::from_secs(120));
                if let Some(r) = job.progress.lock().unwrap().finished.take() {
                    return (r, out);
                }
            }
        };
        let (r, out) = run("a.png", How::Png(16), 0);
        r.unwrap();
        let back = crate::image::load(None, &out).unwrap();
        assert_eq!((back.width, back.height, back.bits()), (300, 200, 16));
        let (r, out) = run("b.png", How::Png(8), 150);
        r.unwrap();
        assert_eq!(crate::image::load(None, &out).unwrap().width, 150);
        if Tools::find("").is_some() || std::env::var_os("GRACE9000_REQUIRE_FFMPEG").is_some() {
            let s = Export::default();
            let (r, out) = run("c.jpg", how("jpg", &s, 16, None).unwrap(), 0);
            r.unwrap();
            let back = crate::image::load(Tools::find("").as_ref(), &out).unwrap();
            assert_eq!((back.width, back.height), (300, 200));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
