//! grace9000's adapters: the 9000 series' shared host (core9000), with what
//! grace9000 adds to `adapter.toml` (export formats and file types it opens)
//! and the permissions of its own requests. The protocol is in
//! docs/adapters.md.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use core9000::adapters::{Env, Message, StartError, builtin_dir, command_name, describe, is_package, parse_command, remember_approval, summary};

/// The version of grace9000's own requests (`edit.*`, `image.render`),
/// sent in `initialize` as `protocol`. New versions only add things.
pub const PROTOCOL: u32 = 2;

pub const EDIT_READ: &str = "edit.read";
pub const EDIT_CHANGE: &str = "edit.change";

/// Who grace9000 is, for core9000.
pub static APP: core9000::App = core9000::App {
    name: "grace9000",
    version: env!("CARGO_PKG_VERSION"),
    env_prefix: "GRACE9000",
    legacy_env_prefix: "",
    protocol: PROTOCOL,
    permissions: &[(EDIT_READ, "see your photo: the file, its size and your edit"), (EDIT_CHANGE, "change your edit, render it to files and open photos")],
    events_permission: EDIT_READ,
};

/// What grace9000 reads from `adapter.toml` beyond what every sister does.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Extra {
    #[serde(default)]
    pub exports: Vec<ExportSpec>,
    /// File types it opens (protocol 2).
    #[serde(default)]
    pub opens: Vec<OpenSpec>,
}

impl core9000::adapters::Extra for Extra {
    fn adds(&self) -> Vec<String> {
        let mut adds = Vec::new();
        if !self.exports.is_empty() {
            adds.push(format!("{} export format{}", self.exports.len(), if self.exports.len() == 1 { "" } else { "s" }));
        }
        if !self.opens.is_empty() {
            let exts: Vec<String> = self.opens.iter().flat_map(|o| o.extensions.iter().map(|e| format!(".{e}"))).collect();
            adds.push(format!("opening {} files", exts.join(" ")));
        }
        adds
    }

    fn without_program(&self) -> bool {
        !self.exports.is_empty() || !self.opens.is_empty()
    }

    fn own_programs(&self) -> Option<String> {
        (!self.opens.is_empty()).then(|| "Its program runs on the files it opens, and nothing else.".into())
    }

    const ALONE: &'static str = "`[[exports]]` or `[[opens]]`";
}

pub type Manifest = core9000::adapters::Manifest<Extra>;
pub type Host = core9000::adapters::Host<Extra>;

pub fn package_manifest(path: &Path) -> Result<Manifest, String> {
    core9000::adapters::package_manifest(path)
}

pub fn install(source: &Path, config: &Path) -> Result<Manifest, String> {
    core9000::adapters::install(source, config)
}

/// grace9000's sections, across every adapter.
pub trait Sections {
    /// Every export format, with the adapter it comes from.
    fn exports(&self) -> impl Iterator<Item = (&str, &ExportSpec)>;
    fn export(&self, adapter: &str, id: &str) -> Option<&ExportSpec>;
    /// Every way an adapter opens files, ready to run.
    fn openers(&self) -> Vec<Opener>;
}

impl Sections for Host {
    fn exports(&self) -> impl Iterator<Item = (&str, &ExportSpec)> {
        self.all.iter().flat_map(|a| a.manifest.extra.exports.iter().map(move |e| (a.manifest.name.as_str(), e)))
    }

    fn export(&self, adapter: &str, id: &str) -> Option<&ExportSpec> {
        self.exports().find(|(a, e)| *a == adapter && e.id == id).map(|(_, e)| e)
    }

    fn openers(&self) -> Vec<Opener> {
        let mut out = Vec::new();
        for (i, a) in self.all.iter().enumerate() {
            for o in &a.manifest.extra.opens {
                out.push(Opener {
                    adapter: a.manifest.name.clone(),
                    label: o.label.clone(),
                    dir: a.dir.clone(),
                    run: o.run.clone(),
                    extensions: o.extensions.iter().map(|e| e.trim_start_matches('.').to_lowercase()).collect(),
                    approved: self.approved(i),
                });
            }
        }
        out
    }
}

/// An export format: how the photo is written. Anything left out comes
/// from settings.toml.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct ExportSpec {
    pub id: String,
    pub label: String,
    /// The file extension, such as "jpg" or "avif".
    pub extension: String,
    /// ffmpeg's encoder arguments, in place of the extension's own, such as
    /// ["-c:v", "libaom-av1", "-still-picture", "1"].
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// An ffmpeg filter run on the finished picture, such as `hue=s=0`.
    #[serde(default)]
    pub filter: String,
    /// Shrink so the long side is this many pixels; 0: settings.toml's.
    #[serde(default)]
    pub long_edge: u32,
    /// 8 or 16 bits a channel into ffmpeg; 0: the photo's own.
    #[serde(default)]
    pub bits: u32,
}

/// File types an adapter opens: its program turns one into a picture
/// grace9000 reads, then quits. `run` has `{input}` (the file) and
/// `{output}` (where the picture goes, as 8 or 16-bit PAM or PNG).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
pub struct OpenSpec {
    pub label: String,
    /// Without the dot; any case.
    pub extensions: Vec<String>,
    pub run: Vec<String>,
}

/// An adapter's way of opening some files, ready to run on a reading thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Opener {
    pub adapter: String,
    pub label: String,
    pub dir: PathBuf,
    pub run: Vec<String>,
    pub extensions: Vec<String>,
    /// The user allowed it (built-in ones always are).
    pub approved: bool,
}

impl Opener {
    pub fn opens(&self, path: &Path) -> bool {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))
    }

    /// Runs the program on `input`, writing `output`. Blocks; call it off
    /// the interface thread.
    pub fn run(&self, input: &Path, output: &Path, env: &Env) -> Result<(), String> {
        // Absolute, since the program starts in its own folder.
        let (input, output) = (std::path::absolute(input).unwrap_or(input.into()), std::path::absolute(output).unwrap_or(output.into()));
        let fill = |a: &String| a.replace("{input}", &input.to_string_lossy()).replace("{output}", &output.to_string_lossy());
        let run: Vec<String> = self.run.iter().map(fill).collect();
        let child = core9000::adapters::spawn(&self.dir, &run, env)?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{}: {}", self.adapter, err.trim().lines().last().unwrap_or("it couldn't open the file")));
        }
        Ok(())
    }
}
