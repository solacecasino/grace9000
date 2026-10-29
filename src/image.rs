//! Photos and the ffmpeg programs that read and write them.
//!
//! grace9000 links no codecs, the way its siblings don't: it runs the
//! system's `ffmpeg` and `ffprobe` as separate programs, so a broken file or
//! a crashing decoder can't take the editor down with it. PNG is read by
//! grace9000 itself, so it works without ffmpeg.
//!
//! A photo is read once, at its own bit depth (8 bits a channel, or 16 for
//! anything deeper), and kept as it was stored, gamma and all. Beside it
//! sits a pyramid of halvings, so a preview of any size reads only a few
//! pixels for each one it shows.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::adapters::{Env, Opener};

/// The files grace9000 offers to open. ffmpeg reads more; any file can be
/// dropped on the window.
pub const OPENS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "tif", "tiff", "webp", "heic", "heif", "hif", "avif", "jxl", "jp2", "j2k", "jpf", "bmp", "dib", "gif", "tga",
    "psd", "exr", "hdr", "pfm", "qoi", "dds", "ppm", "pgm", "pbm", "pam", "pnm", "sgi", "rgb", "pcx", "xbm", "xpm", "dpx", "fits", "ico", "cur", "wbmp",
];

/// One level of the pyramid: RGBA, gamma-encoded as in the file.
pub struct Level<T> {
    pub width: usize,
    pub height: usize,
    pub px: Vec<T>,
}

/// The pyramid, from the photo itself (level 0) down to a small one.
pub enum Pixels {
    Eight(Vec<Level<u8>>),
    Sixteen(Vec<Level<u16>>),
}

/// A photo, read.
pub struct Photo {
    pub path: PathBuf,
    pub width: usize,
    pub height: usize,
    pub pixels: Pixels,
    /// How it was stored, for the Info panel: "jpeg · yuvj420p".
    pub format: String,
}

impl Photo {
    pub fn from_rgba8(path: PathBuf, width: usize, height: usize, px: Vec<u8>, format: String) -> Self {
        Self { path, width, height, pixels: Pixels::Eight(pyramid(Level { width, height, px })), format }
    }

    pub fn from_rgba16(path: PathBuf, width: usize, height: usize, px: Vec<u16>, format: String) -> Self {
        Self { path, width, height, pixels: Pixels::Sixteen(pyramid(Level { width, height, px })), format }
    }

    pub fn bits(&self) -> u32 {
        match self.pixels {
            Pixels::Eight(_) => 8,
            Pixels::Sixteen(_) => 16,
        }
    }

    /// The memory the pixels take, pyramid included.
    pub fn bytes(&self) -> usize {
        match &self.pixels {
            Pixels::Eight(l) => l.iter().map(|l| l.px.len()).sum(),
            Pixels::Sixteen(l) => l.iter().map(|l| l.px.len() * 2).sum(),
        }
    }

    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// A channel value as stored: 8 or 16 bits.
pub trait Channel: Copy + Send + Sync + 'static {
    const MAX: f32;
    /// Every stored value's linear light. Fetch it once, then index it.
    fn table() -> &'static [f32];
    fn index(self) -> usize;
    fn unit(self) -> f32 {
        self.index() as f32 / Self::MAX
    }
    /// Each of 4 channels averaged, for the next level down.
    fn average(a: Self, b: Self, c: Self, d: Self) -> Self;
}

impl Channel for u8 {
    const MAX: f32 = 255.0;
    fn table() -> &'static [f32] {
        static T: OnceLock<Vec<f32>> = OnceLock::new();
        T.get_or_init(|| (0..256).map(|v| to_linear_exact(v as f32 / 255.0)).collect())
    }
    fn index(self) -> usize {
        self as usize
    }
    fn average(a: Self, b: Self, c: Self, d: Self) -> Self {
        ((u16::from(a) + u16::from(b) + u16::from(c) + u16::from(d) + 2) / 4) as u8
    }
}

impl Channel for u16 {
    const MAX: f32 = 65535.0;
    fn table() -> &'static [f32] {
        static T: OnceLock<Vec<f32>> = OnceLock::new();
        T.get_or_init(|| (0..65536).map(|v| to_linear_exact(v as f32 / 65535.0)).collect())
    }
    fn index(self) -> usize {
        self as usize
    }
    fn average(a: Self, b: Self, c: Self, d: Self) -> Self {
        ((u32::from(a) + u32::from(b) + u32::from(c) + u32::from(d) + 2) / 4) as u16
    }
}

/// sRGB's curve, from what's stored to light, exactly.
fn to_linear_exact(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Light to sRGB's curve, exactly.
fn to_gamma_exact(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

const N: usize = 4096;

/// sRGB's curve both ways as tables for 0 to 1 (exact outside), fetched
/// once and passed around, so each pixel only indexes them.
#[derive(Clone, Copy)]
pub struct Curves {
    lin: &'static [f32],
    gam: &'static [f32],
}

impl Curves {
    pub fn get() -> Self {
        static L: OnceLock<Vec<f32>> = OnceLock::new();
        static G: OnceLock<Vec<f32>> = OnceLock::new();
        let lin = L.get_or_init(|| (0..=N).map(|i| to_linear_exact(i as f32 / N as f32)).collect());
        let gam = G.get_or_init(|| (0..=N).map(|i| to_gamma_exact(i as f32 / N as f32)).collect());
        Self { lin, gam }
    }

    #[inline]
    pub fn linear(&self, v: f32) -> f32 {
        if !(0.0..=1.0).contains(&v) { to_linear_exact(v) } else { lerp(self.lin, v) }
    }

    #[inline]
    pub fn gamma(&self, v: f32) -> f32 {
        if !(0.0..=1.0).contains(&v) { to_gamma_exact(v) } else { lerp(self.gam, v) }
    }
}

#[inline]
fn lerp(t: &[f32], v: f32) -> f32 {
    let x = v * N as f32;
    let i = (x as usize).min(N - 1);
    t[i] + (t[i + 1] - t[i]) * (x - i as f32)
}

/// Halves `top` until it's 256 pixels or less on its long side.
fn pyramid<T: Channel>(top: Level<T>) -> Vec<Level<T>> {
    let mut levels = vec![top];
    loop {
        let l = levels.last().expect("one at least");
        if l.width.max(l.height) <= 256 {
            break;
        }
        let (w, h) = ((l.width / 2).max(1), (l.height / 2).max(1));
        let mut px = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let (r0, r1) = ((2 * y).min(l.height - 1), (2 * y + 1).min(l.height - 1));
            for x in 0..w {
                let (c0, c1) = ((2 * x).min(l.width - 1), (2 * x + 1).min(l.width - 1));
                for ch in 0..4 {
                    let at = |r: usize, c: usize| l.px[(r * l.width + c) * 4 + ch];
                    px.push(T::average(at(r0, c0), at(r0, c1), at(r1, c0), at(r1, c1)));
                }
            }
        }
        levels.push(Level { width: w, height: h, px });
    }
    levels
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Finds ffmpeg and ffprobe: `configured` (from settings.toml) if set,
    /// otherwise on the PATH or where package managers put them. Apps
    /// started from Finder don't get the shell's PATH, hence the list.
    pub fn find(configured: &str) -> Option<Self> {
        let ffmpeg = if configured.trim().is_empty() {
            search(exe("ffmpeg"))?
        } else {
            let p = PathBuf::from(configured.trim());
            if !p.is_file() {
                return None;
            }
            p
        };
        // ffprobe sits beside ffmpeg in every package of it.
        let ffprobe = ffmpeg.with_file_name(exe("ffprobe"));
        let ffprobe = if ffprobe.is_file() { ffprobe } else { search(exe("ffprobe"))? };
        Some(Self { ffmpeg, ffprobe })
    }

    /// The encoders this ffmpeg was built with, such as "libwebp".
    pub fn encoders(&self) -> Vec<String> {
        let out = quiet(Command::new(&self.ffmpeg)).args(["-hide_banner", "-encoders"]).output();
        out.map(|o| parse_encoders(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
    }

    /// How the picture is stored (`Probe`).
    fn probe(&self, path: &Path) -> Result<Probe, String> {
        let out = quiet(Command::new(&self.ffprobe))
            .args(["-v", "error", "-of", "json", "-show_entries", "stream=codec_type,codec_name,pix_fmt,bits_per_raw_sample,width,height"])
            .arg(path)
            .output()
            .map_err(|e| format!("couldn't run ffprobe: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(err.lines().last().unwrap_or("ffprobe couldn't read it").to_owned());
        }
        parse_probe(&out.stdout)
    }

    /// The first picture in the file, turned the way its EXIF says (ffmpeg
    /// does that itself), as PAM: a header with the size, then pixels.
    pub fn decode(&self, path: &Path) -> Result<Photo, String> {
        let Probe { format, deep, tile } = self.probe(path)?;
        let mut child = quiet(Command::new(&self.ffmpeg))
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "pam", "-pix_fmt", if deep { "rgba64be" } else { "rgba" }, "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't run ffmpeg: {e}"))?;
        let mut bytes = Vec::new();
        child.stdout.take().expect("piped").read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let mut err = String::new();
        let _ = child.stderr.take().expect("piped").read_to_string(&mut err);
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() && bytes.is_empty() {
            return Err(err.lines().last().unwrap_or("ffmpeg couldn't read it").to_owned());
        }
        let photo = read_pam(path, bytes, format)?;
        // A HEIC or AVIF stored as a grid of tiles: an ffmpeg older than
        // 7.1 gives back the first tile alone.
        if let Some((tw, th)) = tile
            && ((photo.width, photo.height) == (tw, th) || (photo.width, photo.height) == (th, tw))
        {
            return Err(format!("it's stored in tiles, and this ffmpeg ({}) shows only one of them. ffmpeg 7.1 or newer opens the whole photo.", self.ffmpeg.display()));
        }
        Ok(photo)
    }
}

/// What ffprobe says about a photo.
#[derive(Debug, PartialEq)]
struct Probe {
    /// "mjpeg · yuvj420p", for the Info panel.
    format: String,
    /// More than 8 bits a channel.
    deep: bool,
    /// The size of each tile, when it's a grid of same-sized pictures.
    tile: Option<(usize, usize)>,
}

/// Reads a photo: through an adapter that opens files of its kind (camera
/// raw, say), PNG and PAM by grace9000 itself (so they work without
/// ffmpeg), anything else through ffmpeg.
pub fn load(tools: Option<&Tools>, path: &Path, openers: &[Opener], env: &Env) -> Result<Photo, String> {
    if let Some(o) = openers.iter().find(|o| o.opens(path)) {
        if !o.approved {
            return Err(format!("{} opens these files: allow it in the Adapters panel (Allow…) first", o.adapter));
        }
        return open_with(o, path, env);
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pam")) {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return read_pam(path, bytes, "pam".into());
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        match read_png(path) {
            Ok(p) => return Ok(p),
            // Odd PNGs (animated, palette tricks) can still go to ffmpeg.
            Err(e) if tools.is_none() => return Err(e),
            Err(_) => {}
        }
    }
    let tools = tools.ok_or("ffmpeg wasn't found; without it only PNG files can be opened")?;
    tools.decode(path)
}

/// Runs an adapter's program on `path` and reads the picture it leaves.
fn open_with(o: &Opener, path: &Path, env: &Env) -> Result<Photo, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!("grace9000-open-{}-{}.pam", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let r = o.run(path, &out, env).and_then(|()| {
        let bytes = std::fs::read(&out).map_err(|e| format!("{} left no picture: {e}", o.adapter))?;
        let format = format!("{} · via {}", o.label, o.adapter);
        // 8 or 16-bit PAM, or PNG.
        if bytes.starts_with(b"P7") {
            read_pam(path, bytes, format)
        } else {
            read_png(&out).map(|p| Photo { path: path.to_path_buf(), format, ..p })
        }
    });
    let _ = std::fs::remove_file(&out);
    r
}

fn read_png(path: &Path) -> Result<Photo, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("too big")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as usize, info.height as usize);
    // After EXPAND and ALPHA: gray+alpha or RGBA, 8 or 16 bits.
    let gray = matches!(info.color_type, png::ColorType::GrayscaleAlpha);
    let deep = info.bit_depth == png::BitDepth::Sixteen;
    let format = format!("png · {}{}-bit", if gray { "gray · " } else { "" }, if deep { 16 } else { 8 });
    let path = path.to_path_buf();
    if deep {
        let v: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect();
        let px = if gray { v.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect() } else { v };
        Ok(Photo::from_rgba16(path, w, h, px, format))
    } else {
        let px = if gray { buf.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect() } else { buf };
        Ok(Photo::from_rgba8(path, w, h, px, format))
    }
}

/// ffmpeg's PAM output: "P7", then WIDTH, HEIGHT, DEPTH and MAXVAL lines up
/// to ENDHDR, then the pixels (16-bit ones big-endian).
fn read_pam(path: &Path, mut bytes: Vec<u8>, format: String) -> Result<Photo, String> {
    let end = bytes.windows(7).position(|w| w == b"ENDHDR\n").ok_or("no picture in it")? + 7;
    let head = String::from_utf8_lossy(&bytes[..end]);
    let field = |name: &str| head.lines().find_map(|l| l.strip_prefix(name).and_then(|v| v.trim().parse::<usize>().ok()));
    let (w, h, depth, max) = (field("WIDTH").ok_or("no width")?, field("HEIGHT").ok_or("no height")?, field("DEPTH").unwrap_or(4), field("MAXVAL").unwrap_or(255));
    if !(1..=4).contains(&depth) || w == 0 || h == 0 || max == 0 || max > 65535 {
        return Err(format!("an unexpected picture ({w}×{h}, depth {depth}, max {max})"));
    }
    let data = &bytes[end..];
    let path = path.to_path_buf();
    let deep = max > 255;
    let need = w * h * depth * if deep { 2 } else { 1 };
    if data.len() < need {
        return Err("the picture was cut short".into());
    }
    // Gray, gray and alpha, color, or color and alpha, to RGBA.
    let rgba = |v: &[u32], full: u32| -> [u32; 4] {
        match depth {
            1 => [v[0], v[0], v[0], full],
            2 => [v[0], v[0], v[0], v[1]],
            3 => [v[0], v[1], v[2], full],
            _ => [v[0], v[1], v[2], v[3]],
        }
    };
    if deep {
        // Scaled to 16 bits whatever MAXVAL says.
        let scale = |v: u32| (v * 65535 / max as u32) as u16;
        let vals: Vec<u32> = data[..need].as_chunks::<2>().0.iter().map(|b| u32::from(u16::from_be_bytes(*b))).collect();
        let px = vals.chunks_exact(depth).flat_map(|c| rgba(c, max as u32).map(scale)).collect();
        Ok(Photo::from_rgba16(path, w, h, px, format))
    } else if depth == 4 && max == 255 {
        // The pixels stay where ffmpeg put them: no copy of a big photo.
        bytes.drain(..end);
        bytes.truncate(need);
        bytes.shrink_to_fit();
        Ok(Photo::from_rgba8(path, w, h, bytes, format))
    } else {
        let scale = |v: u32| (v * 255 / max as u32) as u8;
        let vals: Vec<u32> = data[..need].iter().map(|&b| u32::from(b)).collect();
        let px = vals.chunks_exact(depth).flat_map(|c| rgba(c, max as u32).map(scale)).collect();
        Ok(Photo::from_rgba8(path, w, h, px, format))
    }
}

fn parse_probe(json: &[u8]) -> Result<Probe, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    // Old ffprobes leave out codec_type for some image formats.
    let video: Vec<&serde_json::Value> = streams.iter().filter(|s| s["codec_type"].as_str().is_none_or(|t| t == "video")).collect();
    let s = video.first().ok_or("no picture in it")?;
    let codec = s["codec_name"].as_str().unwrap_or("?");
    let pix = s["pix_fmt"].as_str().unwrap_or("?");
    let bits = s["bits_per_raw_sample"].as_str().and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    let deep = bits > 8 || ["10", "12", "14", "16", "48", "64", "f32", "f16"].iter().any(|d| pix.contains(d));
    let size = |s: &serde_json::Value| (s["width"].as_u64().unwrap_or(0) as usize, s["height"].as_u64().unwrap_or(0) as usize);
    let first = size(s);
    let same = video.iter().filter(|s| size(s) == first).count();
    let tile = (matches!(codec, "hevc" | "av1") && same > 1).then_some(first);
    Ok(Probe { format: format!("{codec} · {pix}"), deep, tile })
}

/// The names in `ffmpeg -encoders`: the second word of each line after
/// the "------" that ends its legend.
fn parse_encoders(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(String::from)
        .collect()
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

fn search(name: String) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin", "/snap/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

/// A command whose window (on Windows) never shows.
pub fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(std::process::Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ffprobe_output() {
        let p = parse_probe(br#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","pix_fmt":"yuvj420p","bits_per_raw_sample":"8","width":4,"height":3}]}"#).unwrap();
        assert_eq!(p, Probe { format: "mjpeg · yuvj420p".into(), deep: false, tile: None });
        assert!(parse_probe(br#"{"streams":[{"codec_name":"tiff","pix_fmt":"rgb48le"}]}"#).unwrap().deep);
        assert!(parse_probe(br#"{"streams":[]}"#).is_err());
        // A HEIC grid: same-sized HEVC tiles, and a smaller thumbnail.
        let tiles = br#"{"streams":[{"codec_type":"video","codec_name":"hevc","pix_fmt":"yuvj420p","width":512,"height":512},{"codec_type":"video","codec_name":"hevc","width":512,"height":512},{"codec_type":"video","codec_name":"hevc","width":320,"height":240}]}"#;
        assert_eq!(parse_probe(tiles).unwrap().tile, Some((512, 512)));
    }

    #[test]
    fn reads_the_encoder_list() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D mjpeg                Motion JPEG\n V....D libwebp              WebP\n";
        assert_eq!(parse_encoders(text), ["mjpeg", "libwebp"]);
    }

    #[test]
    fn reads_pam() {
        let mut b = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        b.extend([1, 2, 3, 255, 4, 5, 6, 255]);
        let p = read_pam(Path::new("x.jpg"), b, String::new()).unwrap();
        assert_eq!((p.width, p.height, p.bits()), (2, 1, 8));
        let mut b = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 65535\nENDHDR\n".to_vec();
        b.extend([0x12, 0x34, 0, 0, 0, 0, 0xff, 0xff]);
        let p = read_pam(Path::new("x.tif"), b, String::new()).unwrap();
        let Pixels::Sixteen(l) = &p.pixels else { panic!() };
        assert_eq!(l[0].px, [0x1234, 0, 0, 0xffff]);
        assert!(read_pam(Path::new("x"), b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nENDHDR\n".to_vec(), String::new()).is_err());
        // Gray, and color without alpha, and a MAXVAL that isn't 255.
        let mut b = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 1\nMAXVAL 255\nENDHDR\n".to_vec();
        b.extend([10, 200]);
        let Pixels::Eight(l) = read_pam(Path::new("g.pam"), b, String::new()).unwrap().pixels else { panic!() };
        assert_eq!(l[0].px, [10, 10, 10, 255, 200, 200, 200, 255]);
        let mut b = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nMAXVAL 1023\nENDHDR\n".to_vec();
        b.extend([0x03, 0xff, 0, 0, 0x02, 0x00]);
        let Pixels::Sixteen(l) = read_pam(Path::new("c.pam"), b, String::new()).unwrap().pixels else { panic!() };
        assert_eq!(l[0].px, [65535, 0, 32799, 65535]);
    }

    #[test]
    fn pyramid_halves_down_to_256() {
        let p = Photo::from_rgba8("x".into(), 1000, 600, vec![200; 1000 * 600 * 4], String::new());
        let Pixels::Eight(l) = &p.pixels else { panic!() };
        let sizes: Vec<_> = l.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, [(1000, 600), (500, 300), (250, 150)]);
        assert!(l[2].px.iter().all(|&v| v == 200));
    }

    #[test]
    fn gamma_round_trips() {
        let k = Curves::get();
        for v in [0.0, 0.001, 0.02, 0.18, 0.5, 0.9, 1.0] {
            assert!((k.linear(k.gamma(v)) - v).abs() < 1e-3, "{v}");
        }
        assert!((k.gamma(2.0) - 1.353).abs() < 1e-2);
        assert_eq!(u8::table()[128], to_linear_exact(128.0 / 255.0));
        assert!((k.linear(0.5) - to_linear_exact(0.5)).abs() < 1e-5);
    }
}

#[cfg(test)]
mod adapter_tests {
    use super::*;

    /// A built-in adapter (its folder name), from its staged folder or its
    /// build; None when it isn't built (GRACE9000_REQUIRE_ADAPTERS=1 makes
    /// that fail).
    pub fn adapter(name: &str) -> Option<Opener> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let staged = std::env::var_os("GRACE9000_BUILTIN_ADAPTERS").map(|d| PathBuf::from(d).join(name));
        let built = root.join(format!("adapters/{name}/target/release"));
        let dir = [staged, Some(built)].into_iter().flatten().find(|d| d.join(name).exists() || d.join(format!("{name}.exe")).exists());
        let Some(dir) = dir else {
            assert!(std::env::var_os("GRACE9000_REQUIRE_ADAPTERS").is_none(), "{name} isn't built");
            return None;
        };
        let text = std::fs::read_to_string(root.join(format!("adapters/{name}/adapter.toml"))).unwrap();
        let m: crate::adapters::Manifest = toml::from_str(&text).unwrap();
        let o = &m.opens[0];
        Some(Opener { adapter: m.name.clone(), label: o.label.clone(), dir, run: o.run.clone(), extensions: o.extensions.clone(), approved: true })
    }

    fn at(p: &Photo, x: usize, y: usize) -> [u16; 3] {
        let Pixels::Sixteen(l) = &p.pixels else { panic!("raw comes in at 16 bits") };
        let i = (y * p.width + x) * 4;
        [l[0].px[i], l[0].px[i + 1], l[0].px[i + 2]]
    }

    #[test]
    fn opens_camera_raw_through_its_adapter() {
        let Some(o) = adapter("camera-raw") else { return };
        let photos = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/photos");
        let p = load(None, &photos.join("synthetic.dng"), std::slice::from_ref(&o), &Env::default()).unwrap();
        assert!(p.format.contains("Camera Raw"), "{}", p.format);
        assert_eq!(p.bits(), 16);
        // Red on the left, blue on the right, green across the top, and a
        // gray middle, as the mosaic was made.
        let (w, h) = (p.width, p.height);
        let (red, blue, green, gray) = (at(&p, w / 8, h * 3 / 4), at(&p, w * 7 / 8, h * 3 / 4), at(&p, w / 2, 2), at(&p, w / 2, h / 2));
        assert!(red[0] > red[1] * 3 && red[0] > red[2] * 3, "{red:?}");
        assert!(blue[2] > blue[0] * 3 && blue[2] > blue[1] * 3, "{blue:?}");
        assert!(green[1] > green[0] * 3 && green[1] > green[2] * 3, "{green:?}");
        assert!(gray.iter().all(|&v| (v as i32 - gray[0] as i32).abs() < 2000), "{gray:?}");
        // 0.18 light is about 0.46 in sRGB.
        assert!((gray[1] as f32 / 65535.0 - 0.46).abs() < 0.05, "{gray:?}");

        // Held turned: the left side comes out on top, green on the right.
        let t = load(None, &photos.join("synthetic-turned.dng"), &[o], &Env::default()).unwrap();
        assert_eq!((t.width, t.height), (h, w));
        let top = at(&t, t.width / 4, t.height / 8);
        let right = at(&t, t.width - 2, t.height / 2);
        assert!(top[0] > top[2] * 3, "{top:?}");
        assert!(right[1] > right[0] * 3, "{right:?}");
    }

    #[test]
    fn opens_jpeg_xl_through_its_adapter() {
        let Some(o) = adapter("jpeg-xl") else { return };
        let photos = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/photos");
        let p = load(None, &photos.join("tiny.jxl"), std::slice::from_ref(&o), &Env::default()).unwrap();
        assert_eq!((p.width, p.height, p.bits()), (240, 135, 16));
        // A real photo's colors, where this ffmpeg can write JPEG XL.
        let Some(tools) = Tools::find("") else { return };
        let path = std::env::temp_dir().join(format!("grace9000-{}.jxl", std::process::id()));
        let wrote = quiet(Command::new(&tools.ffmpeg))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x3366cc:s=30x20:d=1", "-frames:v", "1", "-update", "1", "-distance", "0.5"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !wrote {
            return;
        }
        let p = load(None, &path, &[o], &Env::default()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((p.width, p.height), (30, 20));
        let c = at(&p, 15, 10).map(|v| v as f32 / 65535.0);
        assert!((c[0] - 0.2).abs() < 0.05 && (c[1] - 0.4).abs() < 0.05 && (c[2] - 0.8).abs() < 0.05, "{c:?}");
    }

    #[test]
    fn an_opener_that_isnt_allowed_says_so() {
        let o = Opener { adapter: "X".into(), label: "x".into(), dir: ".".into(), run: vec![], extensions: vec!["xyz".into()], approved: false };
        let e = load(None, Path::new("a.XYZ"), &[o], &Env::default()).err().unwrap();
        assert!(e.contains("allow it"), "{e}");
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    /// Every format ffmpeg can write, written and read back: the size
    /// right, and the color close (exactly where nothing is lossy).
    #[test]
    fn opens_every_format() {
        let Some(tools) = Tools::find("") else {
            assert!(std::env::var_os("GRACE9000_REQUIRE_FFMPEG").is_none(), "ffmpeg wasn't found");
            return;
        };
        let dir = std::env::temp_dir().join(format!("grace9000-formats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // ffmpeg's own encoders, always there; then ones that need a library.
        let always = ["jpg", "png", "tif", "bmp", "gif", "tga", "ppm", "pgm", "pam", "sgi", "pcx", "dpx", "exr", "hdr", "pfm", "qoi", "jp2", "fits", "xwd"];
        let maybe = ["webp", "avif", "jxl"];
        let mut opened = Vec::new();
        for ext in always.iter().chain(&maybe) {
            let path = dir.join(format!("x.{ext}"));
            let wrote = quiet(Command::new(&tools.ffmpeg))
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x3366cc:s=30x20:d=1", "-frames:v", "1", "-update", "1"])
                .arg(&path)
                .status()
                .is_ok_and(|s| s.success());
            if !wrote {
                assert!(!always.contains(ext), "ffmpeg couldn't write .{ext}");
                continue;
            }
            let p = load(Some(&tools), &path, &[], &Env::default()).unwrap_or_else(|e| panic!(".{ext}: {e}"));
            assert_eq!((p.width, p.height), (30, 20), ".{ext}");
            let c = match &p.pixels {
                Pixels::Eight(l) => [l[0].px[0], l[0].px[1], l[0].px[2]].map(|v| v as f32 / 255.0),
                Pixels::Sixteen(l) => [l[0].px[0], l[0].px[1], l[0].px[2]].map(|v| v as f32 / 65535.0),
            };
            let want = if *ext == "pgm" { [0.37; 3] } else { [0.2, 0.4, 0.8] };
            let off = c.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            // GIF has 256 colors to pick from.
            let near = if *ext == "gif" { 0.15 } else { 0.06 };
            assert!(off < near, ".{ext}: {c:?}");
            opened.push(*ext);
        }
        // HEIC, where macOS can make one.
        let heic = dir.join("x.heic");
        let png = dir.join("x.png");
        if Command::new("sips").args(["-s", "format", "heic"]).arg(&png).arg("--out").arg(&heic).output().is_ok_and(|o| o.status.success()) {
            let p = load(Some(&tools), &heic, &[], &Env::default()).unwrap();
            assert_eq!((p.width, p.height), (30, 20));
            opened.push("heic");
        }
        // Photoshop too.
        let psd = dir.join("x.psd");
        if Command::new("sips").args(["-s", "format", "psd"]).arg(&png).arg("--out").arg(&psd).output().is_ok_and(|o| o.status.success()) {
            let p = load(Some(&tools), &psd, &[], &Env::default()).unwrap();
            assert_eq!((p.width, p.height), (30, 20));
            opened.push("psd");
        }
        eprintln!("opened: {}", opened.join(" "));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
