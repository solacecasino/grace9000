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

/// The files grace9000 offers to open. ffmpeg reads more; any file can be
/// dropped on the window.
pub const OPENS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "bmp", "gif", "jxl", "tga", "exr"];

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

    /// The codec and pixel format, and whether it holds more than 8 bits.
    fn probe(&self, path: &Path) -> Result<(String, bool), String> {
        let out = quiet(Command::new(&self.ffprobe))
            .args(["-v", "error", "-select_streams", "v:0", "-of", "json", "-show_entries", "stream=codec_name,pix_fmt,bits_per_raw_sample"])
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
        let (format, deep) = self.probe(path)?;
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
        read_pam(path, bytes, format)
    }
}

/// Reads a photo: PNG by grace9000 itself (so it works without ffmpeg),
/// anything else through ffmpeg.
pub fn load(tools: Option<&Tools>, path: &Path) -> Result<Photo, String> {
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
    let end = bytes.windows(7).position(|w| w == b"ENDHDR\n").ok_or("ffmpeg gave no picture")? + 7;
    let head = String::from_utf8_lossy(&bytes[..end]);
    let field = |name: &str| head.lines().find_map(|l| l.strip_prefix(name).and_then(|v| v.trim().parse::<usize>().ok()));
    let (w, h, depth, max) = (field("WIDTH").ok_or("no width")?, field("HEIGHT").ok_or("no height")?, field("DEPTH").unwrap_or(4), field("MAXVAL").unwrap_or(255));
    if depth != 4 || w == 0 || h == 0 {
        return Err(format!("ffmpeg gave an unexpected picture ({w}×{h}, depth {depth})"));
    }
    let data = &bytes[end..];
    let path = path.to_path_buf();
    if max > 255 {
        let need = w * h * 8;
        let px: Vec<u16> = data.get(..need).ok_or("the picture was cut short")?.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect();
        Ok(Photo::from_rgba16(path, w, h, px, format))
    } else {
        // The pixels stay where ffmpeg put them: no copy of a big photo.
        let need = w * h * 4;
        if data.len() < need {
            return Err("the picture was cut short".into());
        }
        bytes.drain(..end);
        bytes.truncate(need);
        bytes.shrink_to_fit();
        Ok(Photo::from_rgba8(path, w, h, bytes, format))
    }
}

fn parse_probe(json: &[u8]) -> Result<(String, bool), String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let s = v["streams"].as_array().and_then(|s| s.first()).ok_or("no picture in it")?;
    let codec = s["codec_name"].as_str().unwrap_or("?");
    let pix = s["pix_fmt"].as_str().unwrap_or("?");
    let bits = s["bits_per_raw_sample"].as_str().and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    let deep = bits > 8 || ["10", "12", "14", "16", "48", "64", "f32", "f16"].iter().any(|d| pix.contains(d));
    Ok((format!("{codec} · {pix}"), deep))
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
        let (f, deep) = parse_probe(br#"{"streams":[{"codec_name":"mjpeg","pix_fmt":"yuvj420p","bits_per_raw_sample":"8"}]}"#).unwrap();
        assert_eq!((f.as_str(), deep), ("mjpeg · yuvj420p", false));
        assert!(parse_probe(br#"{"streams":[{"codec_name":"tiff","pix_fmt":"rgb48le"}]}"#).unwrap().1);
        assert!(parse_probe(br#"{"streams":[]}"#).is_err());
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
