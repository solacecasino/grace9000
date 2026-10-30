// No console window behind the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! grace9000: a bare, modular photo editor.
//!
//! One owner of state: everything lives in `App`, on the interface thread.
//! Reading photos, the preview and exporting run on their own threads, so
//! nothing slow can make the interface wait. The photo itself is never
//! changed: the edit is a list of steps and a crop, kept beside it.

mod adapters;
mod develop;
mod export;
mod hosting;
mod image;
mod incoming;
mod keys;
mod layout;
mod log;
mod menu;
mod panels;
mod preview;
mod project;
mod settings;
mod shell;
mod theme;
mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{TextureHandle, TextureOptions, ViewportBuilder, ViewportCommand};

use crate::export::Job;
use crate::image::{Photo, Tools};
use crate::keys::Keymap;
use crate::layout::{LayoutUi, Node, Side};
use crate::preview::Preview;
use crate::project::{Crop, Edit, Project, Step};
use crate::settings::Settings;

/// 256 px is all the dock, the window and About ever show.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

/// The strip across the top of the window: on macOS under the transparent
/// titlebar (the menus are the system's), elsewhere the menus themselves.
const TOP: f32 = 32.0;

/// The project file format this version writes.
const FORMAT: u32 = 1;

/// A photo that's been read, or why it couldn't be, and the edit to give
/// it (from a `.grace` file or the session).
type Loaded = (PathBuf, Result<Photo, String>, Option<Project>);

/// Where the Photo panel is looking.
pub struct View {
    /// Output pixels per photo pixel; None: fit the panel.
    pub zoom: Option<f32>,
    /// The middle of the panel, in fractions of the result.
    pub center: [f32; 2],
    /// The scale last drawn at, for zooming from it.
    pub scale: f32,
    /// The texture shows this part of the result.
    pub shown: Option<develop::View>,
    /// Where the photo was last drawn on screen, whole or cropped.
    pub area: Option<egui::Rect>,
    pub drag: panels::Drag,
}

pub struct App {
    dir: PathBuf,
    palette: theme::Palette,
    menu: menu::MenuBar,
    settings: Settings,
    keymap: Keymap,
    layout: Node,
    layout_ui: LayoutUi,
    config_changed: Arc<AtomicBool>,
    tools: Option<Tools>,

    pub project: Project,
    pub photo: Option<Arc<Photo>>,
    /// The photo being read.
    reading: Option<PathBuf>,
    loaded_tx: Sender<Loaded>,
    loaded_rx: Receiver<Loaded>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The selected step.
    pub selected: Option<usize>,
    preview: Preview,
    texture: Option<TextureHandle>,
    histogram: Option<Box<[[u32; 256]; 4]>>,
    /// How long the last preview took, in milliseconds.
    preview_ms: f32,
    view: View,
    /// Cropping and straightening: the whole photo shows, with the crop on it.
    cropping: bool,
    /// The crop's shape while cropping (width / height); None: any.
    crop_aspect: Option<f32>,
    /// Showing the photo without its steps.
    compare: bool,
    file: Option<PathBuf>,
    /// Unsaved changes to the edit.
    dirty: bool,
    /// The session needs writing.
    session_dirty: bool,
    /// The system theme last asked for (the traffic lights follow it).
    system_dark: Option<bool>,
    export: Option<Job>,
    last_export: Option<Result<String, String>>,
    /// Commands from buttons, run at the start of the next frame.
    queue: Vec<String>,
    adapters: adapters::Host,
    adapter_rx: Receiver<adapters::Message>,
    /// What adapters put in their panels, by panel kind.
    adapter_panels: HashMap<String, Vec<serde_json::Value>>,
    /// `image.render`s being written for adapters.
    renders: Vec<hosting::Render>,
    /// The question on screen, and the ones waiting behind it.
    prompt: Option<hosting::Prompt>,
    prompts: Vec<hosting::Prompt>,
    /// The export format picked in the Export panel (adapter, id); None:
    /// settings.toml's.
    export_choice: Option<(String, String)>,
    /// A short message at the bottom, until the time given.
    note: Option<(String, f64)>,
    /// The edit changed since adapters were last told.
    emit_changed: bool,
    /// The encoders this ffmpeg has, read the first time they matter.
    encoders: Option<Vec<String>>,
    title: String,
    ctx: egui::Context,
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("grace9000 {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // Files macOS opens with the app, from Finder or another app.
    incoming::listen_early();
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    crash_report(&dir);
    let mut builder = ViewportBuilder::default()
        .with_title("grace9000")
        .with_app_id("grace9000")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([480.0, 320.0]);
    if let Some((rgba, width, height)) = icon_rgba() {
        builder = builder.with_icon(Arc::new(egui::IconData { rgba, width, height }));
    }
    if cfg!(target_os = "macos") {
        // Drawn under a transparent titlebar, so the top strip is ours.
        builder = builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    if let Err(e) = shell::run(builder, move |ctx| App::new(ctx, dir)) {
        eprintln!("grace9000: {e}");
        std::process::exit(1);
    }
}

/// The app icon as RGBA pixels.
fn icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba).then(|| {
        buf.truncate(info.buffer_size());
        (buf, info.width, info.height)
    })
}

/// macOS shows a bare program's icon as a blank one in the dock; give it ours.
#[cfg(all(target_os = "macos", not(test)))]
fn set_dock_icon() {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{ClassType, msg_send};
    let data = objc2_foundation::NSData::with_bytes(ICON_PNG);
    // SAFETY: plain AppKit calls on the main thread with live objects.
    unsafe {
        let (Some(image), Some(app)) = (AnyClass::get(c"NSImage"), AnyClass::get(c"NSApplication")) else { return };
        let image: *mut AnyObject = msg_send![image, alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: &*data];
        let app: *mut AnyObject = msg_send![app, sharedApplication];
        if !image.is_null() && !app.is_null() {
            let _: () = msg_send![app, setApplicationIconImage: image];
        }
    }
    let _ = objc2_foundation::NSData::class();
}

fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GRACE9000_CONFIG_DIR") {
        return d.into();
    }
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/grace9000")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("grace9000")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from).join("grace9000")
    }
}

/// A crash leaves crash.txt; the next start reports it in the Log.
fn crash_report(dir: &Path) {
    let file = dir.join("crash.txt");
    if let Ok(text) = std::fs::read_to_string(&file) {
        log::error(format!("grace9000 stopped unexpectedly last time. Your edit was kept. {}", text.trim()));
        let _ = std::fs::rename(&file, dir.join("crash-last.txt"));
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::write(&file, format!("{info}"));
        default(info);
    }));
}

/// Writes `text` to `path` through a temporary file, so a crash midway
/// can't leave half a file.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

impl App {
    fn new(ctx: &egui::Context, dir: PathBuf) -> Self {
        incoming::started(ctx);
        for (name, text) in [("settings.toml", settings::EXAMPLE), ("keys.toml", keys::EXAMPLE)] {
            let path = dir.join(name);
            if !path.exists() {
                let _ = std::fs::write(path, text);
            }
        }
        let layout_path = dir.join("layout.toml");
        if !layout_path.exists() {
            let _ = std::fs::write(&layout_path, layout::to_toml(&layout::default_layout()));
        }
        let config_changed = Arc::new(AtomicBool::new(false));
        {
            let (flag, ctx) = (config_changed.clone(), ctx.clone());
            watch::config_folder(&dir, move || {
                flag.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        #[cfg(all(target_os = "macos", not(test)))]
        set_dock_icon();
        let (tx, adapter_rx) = channel();
        let adapters = {
            let (tx, ctx) = (std::sync::Mutex::new(tx), ctx.clone());
            let env = adapters::Env { config: dir.clone(), builtin: adapters::builtin_dir(), ffmpeg: None, ffprobe: None };
            adapters::Host::new(env, move |m| {
                let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(m);
                ctx.request_repaint();
            })
        };
        let (loaded_tx, loaded_rx) = channel();
        let preview = {
            let ctx = ctx.clone();
            Preview::start(move || ctx.request_repaint())
        };
        let mut app = Self {
            menu: menu::MenuBar::install(&Keymap::load("", &[]).0, ctx, None, &menu::Extras::default()),
            palette: theme::Palette::new(&settings::Look::default()),
            dir,
            settings: Settings::default(),
            keymap: Keymap::load("", &[]).0,
            layout: layout::default_layout(),
            layout_ui: LayoutUi::default(),
            config_changed,
            tools: None,
            project: Project::default(),
            photo: None,
            reading: None,
            loaded_tx,
            loaded_rx,
            undo: Vec::new(),
            redo: Vec::new(),
            selected: None,
            preview,
            texture: None,
            histogram: None,
            preview_ms: 0.0,
            view: View { zoom: None, center: [0.5, 0.5], scale: 1.0, shown: None, area: None, drag: panels::Drag::None },
            cropping: false,
            crop_aspect: None,
            compare: false,
            file: None,
            dirty: false,
            session_dirty: false,
            system_dark: None,
            export: None,
            last_export: None,
            queue: Vec::new(),
            adapters,
            adapter_rx,
            adapter_panels: HashMap::new(),
            renders: Vec::new(),
            prompt: None,
            prompts: Vec::new(),
            export_choice: None,
            note: None,
            emit_changed: false,
            encoders: None,
            title: String::new(),
            ctx: ctx.clone(),
        };
        app.load_config(true);
        app.adapters.start_launch_adapters();
        let session = std::fs::read_to_string(app.dir.join("session.json")).ok().and_then(|t| serde_json::from_str::<Project>(&t).ok());
        if let Some(p) = session
            && let Some(photo) = p.photo.clone()
        {
            app.read(photo, Some(p));
        }
        // Files named on the command line (Open With on Windows and Linux).
        let named: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).filter(|p| p.exists()).map(|p| std::path::absolute(&p).unwrap_or(p)).collect();
        app.open_paths(named);
        app
    }

    /// (Re)reads settings.toml, keys.toml and layout.toml. A file that
    /// can't be read leaves the rest working and says why in the Log.
    fn load_config(&mut self, first: bool) {
        let read = |name: &str| std::fs::read_to_string(self.dir.join(name)).unwrap_or_default();
        let settings = match toml::from_str::<Settings>(&read("settings.toml")) {
            Ok(s) => s,
            Err(e) => {
                log::error(format!("settings.toml: {} (using the defaults until it's fixed)", e.message()));
                Settings::default()
            }
        };
        match layout::from_toml(&read("layout.toml")) {
            Ok(root) => self.layout = root,
            Err(e) => log::error(format!("layout.toml: {e} (keeping the current layout)")),
        }
        let new_tools = first || settings.ffmpeg != self.settings.ffmpeg;
        self.settings = settings;
        self.apply_look();
        if new_tools {
            self.tools = Tools::find(&self.settings.ffmpeg);
            match &self.tools {
                Some(t) => log::info(format!("Using {}", t.ffmpeg.display())),
                None => log::error("ffmpeg wasn't found, so only PNG files can be opened and written. Install it (ffmpeg.org, or `brew install ffmpeg`) or set `ffmpeg = \"...\"` in settings.toml."),
            }
            self.encoders = None;
            // Adapters get the same ffmpeg.
            self.adapters.env.ffmpeg = self.tools.as_ref().map(|t| t.ffmpeg.clone());
            self.adapters.env.ffprobe = self.tools.as_ref().map(|t| t.ffprobe.clone());
        }
        // Keys (the user's and adapters') and the menus that show them.
        self.refresh_commands();
        self.ctx.request_repaint();
    }

    fn apply_look(&mut self) {
        let look = &self.settings.look;
        self.palette = theme::Palette::new(look);
        let theme = if look.dark { egui::Theme::Dark } else { egui::Theme::Light };
        self.ctx.set_theme(theme);
        self.ctx.set_visuals_of(theme, self.palette.visuals());
        let size = look.font_size.clamp(8.0, 32.0);
        self.ctx.global_style_mut(|s| {
            use egui::TextStyle::*;
            for (style, scale) in [(Body, 1.0), (Button, 1.0), (Monospace, 1.0), (Small, 0.8), (Heading, 1.4)] {
                if let Some(f) = s.text_styles.get_mut(&style) {
                    f.size = size * scale;
                }
            }
        });
    }

    /// Call before changing the edit, so it can be undone.
    pub fn snapshot(&mut self) {
        self.undo.push(self.project.edit.clone());
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.edited();
    }

    pub fn edited(&mut self) {
        self.dirty = true;
        self.session_dirty = true;
        self.emit_changed = true;
        self.ctx.request_repaint();
    }

    pub fn run(&mut self, command: &str) {
        if self.run_adapter_command(command) {
            return;
        }
        if let Some(kind) = command.strip_prefix("add ") {
            self.add_step(kind);
            return;
        }
        let has_photo = self.photo.is_some();
        match command {
            "open" => {
                let theirs: Vec<String> = self.adapters.openers().into_iter().flat_map(|o| o.extensions).collect();
                let mut exts: Vec<&str> = image::OPENS.to_vec();
                exts.extend(theirs.iter().map(String::as_str));
                exts.push("grace");
                if let Some(path) = rfd::FileDialog::new().add_filter("Photos and edits", &exts).add_filter("All files", &["*"]).pick_file() {
                    self.open_paths(vec![path]);
                }
            }
            "close" => {
                self.keep_previous();
                self.photo = None;
                self.texture = None;
                self.histogram = None;
                self.project = Project::default();
                self.file = None;
                self.dirty = false;
                self.cropping = false;
                self.undo.clear();
                self.redo.clear();
                self.selected = None;
                self.session_dirty = true;
            }
            "save" | "save as" if has_photo => {
                let suggested = self.project.photo.as_ref().and_then(|p| p.file_stem()).map_or("photo".into(), |s| s.to_string_lossy().into_owned());
                let path = match (&self.file, command) {
                    (Some(f), "save") => Some(f.clone()),
                    _ => {
                        let mut d = rfd::FileDialog::new().add_filter("grace9000 edit", &["grace"]).set_file_name(format!("{suggested}.grace"));
                        if let Some(folder) = self.project.photo.as_ref().and_then(|p| p.parent()) {
                            d = d.set_directory(folder);
                        }
                        d.save_file()
                    }
                };
                if let Some(path) = path {
                    let mut p = self.project.clone();
                    p.grace9000 = FORMAT;
                    p.file = None;
                    let text = serde_json::to_string_pretty(&p).unwrap_or_default();
                    match write_atomic(&path, &text) {
                        Ok(()) => {
                            self.adapters.emit("saved", serde_json::json!({ "path": path }));
                            self.file = Some(path);
                            self.dirty = false;
                            self.session_dirty = true;
                        }
                        Err(e) => log::error(format!("Couldn't save {}: {e}", path.display())),
                    }
                }
            }
            "export" => self.start_export(self.export_choice.clone()),
            "undo" | "redo" => {
                let (from, to) = if command == "undo" { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
                if let Some(e) = from.pop() {
                    to.push(std::mem::replace(&mut self.project.edit, e));
                    self.selected = self.selected.filter(|&s| s < self.project.edit.steps.len());
                    self.edited();
                }
            }
            "compare" => self.compare = !self.compare,
            "crop" if has_photo => {
                self.cropping = !self.cropping;
                self.view.zoom = None;
            }
            "crop reset" if self.project.edit.crop.is_some() || self.project.edit.straighten != 0.0 => {
                self.snapshot();
                self.project.edit.crop = None;
                self.project.edit.straighten = 0.0;
            }
            "rotate left" | "rotate right" if has_photo => {
                self.snapshot();
                let e = &mut self.project.edit;
                let right = command == "rotate right";
                e.turns = (e.turns + if right { 1 } else { 3 }) % 4;
                // The crop turns with the photo.
                if let Some(c) = e.crop {
                    e.crop = Some(if right { Crop { x: 1.0 - c.y - c.h, y: c.x, w: c.h, h: c.w } } else { Crop { x: c.y, y: 1.0 - c.x - c.w, w: c.h, h: c.w } });
                }
                // Mirrors swap axes too, so the picture turns as a whole.
                std::mem::swap(&mut e.flip_h, &mut e.flip_v);
                self.view.zoom = None;
            }
            "flip horizontal" | "flip vertical" if has_photo => {
                self.snapshot();
                let e = &mut self.project.edit;
                if command == "flip horizontal" {
                    e.flip_h = !e.flip_h;
                    e.crop = e.crop.map(|c| Crop { x: 1.0 - c.x - c.w, ..c });
                } else {
                    e.flip_v = !e.flip_v;
                    e.crop = e.crop.map(|c| Crop { y: 1.0 - c.y - c.h, ..c });
                }
                e.straighten = -e.straighten;
            }
            "straighten left" | "straighten right" if has_photo => {
                let step = self.settings.edit.straighten_step.clamp(0.01, 45.0);
                let d = if command == "straighten right" { step } else { -step };
                self.snapshot();
                self.project.edit.straighten = (self.project.edit.straighten + d).clamp(-45.0, 45.0);
            }
            "step delete" => {
                if let Some(i) = self.selected.filter(|&i| i < self.project.edit.steps.len()) {
                    self.snapshot();
                    self.project.edit.steps.remove(i);
                    self.selected = if self.project.edit.steps.is_empty() { None } else { Some(i.min(self.project.edit.steps.len() - 1)) };
                }
            }
            "step toggle" | "step reset" => {
                if let Some(i) = self.selected.filter(|&i| i < self.project.edit.steps.len()) {
                    self.snapshot();
                    let s = &mut self.project.edit.steps[i];
                    if command == "step toggle" {
                        s.on = !s.on;
                    } else {
                        s.reset();
                    }
                }
            }
            "step up" | "step down" => {
                if let Some(i) = self.selected.filter(|&i| i < self.project.edit.steps.len()) {
                    let to = if command == "step up" { i.checked_sub(1) } else { Some(i + 1).filter(|&j| j < self.project.edit.steps.len()) };
                    if let Some(j) = to {
                        self.move_step(i, j);
                    }
                }
            }
            "step next" | "step previous" => {
                let n = self.project.edit.steps.len();
                if n > 0 {
                    self.selected = Some(match (self.selected, command) {
                        (None, "step next") => 0,
                        (None, _) => n - 1,
                        (Some(i), "step next") => (i + 1).min(n - 1),
                        (Some(i), _) => i.saturating_sub(1),
                    });
                }
            }
            "zoom in" | "zoom out" => {
                let step = self.settings.edit.zoom_step.clamp(1.05, 4.0);
                let s = self.view.scale * if command == "zoom in" { step } else { 1.0 / step };
                self.view.zoom = Some(s.clamp(0.01, 32.0));
            }
            "zoom fit" => self.view.zoom = None,
            "zoom 100" => self.view.zoom = Some(1.0),
            "layout edit" => self.layout_ui.editing = !self.layout_ui.editing,
            c if c.starts_with("panel ") => self.toggle_panel(&c["panel ".len()..]),
            "layout reset" => {
                self.layout = layout::default_layout();
                self.save_layout();
            }
            "config" => open_folder(&self.dir),
            "quit" => self.ctx.send_viewport_cmd(ViewportCommand::Close),
            c if keys::COMMANDS.iter().any(|k| k.0 == c) => {
                // A real command with nothing to act on yet.
                if !has_photo {
                    self.say("Open a photo first: drop one on the window.");
                }
            }
            other => log::error(format!("No command called \"{other}\"")),
        }
    }

    /// Adds a step of `kind` below the selected one (or at the end).
    pub fn add_step(&mut self, kind: &str) {
        if develop::kind(kind).is_none() {
            log::error(format!("No step called \"{kind}\""));
            return;
        }
        self.snapshot();
        let at = self.selected.map_or(self.project.edit.steps.len(), |i| (i + 1).min(self.project.edit.steps.len()));
        self.project.edit.steps.insert(at, Step::new(kind));
        self.selected = Some(at);
    }

    pub fn move_step(&mut self, from: usize, to: usize) {
        let n = self.project.edit.steps.len();
        if from >= n || to >= n || from == to {
            return;
        }
        self.snapshot();
        let s = self.project.edit.steps.remove(from);
        self.project.edit.steps.insert(to, s);
        self.selected = Some(to);
    }

    /// Hides a panel that's showing, or shows one that isn't (beside the
    /// photo). The last panel stays.
    fn toggle_panel(&mut self, kind: &str) {
        if !layout::PANELS.iter().any(|p| p.0 == kind) {
            return;
        }
        let mut at = Vec::new();
        if self.layout.find(kind, &mut at) {
            if at.is_empty() {
                return;
            }
            self.layout.remove(&at);
        } else {
            let mut beside = Vec::new();
            if !self.layout.find("photo", &mut beside) {
                beside.clear();
            }
            self.layout.insert(&beside, kind, Side::Right);
        }
        self.save_layout();
    }

    /// The strip across the top: the mark and name, the photo, and the
    /// tools used most. Dragging it moves the window.
    fn top_strip(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        use egui::{Align2, FontId, RichText, Sense};
        let pal = self.palette;
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0.0, pal.header);
        painter.rect_filled(egui::Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - 1.0..=rect.bottom()), 0.0, pal.line);
        let drag = ui.interact(rect, egui::Id::unique("top strip"), Sense::click_and_drag());
        if drag.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        } else if drag.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }

        // Left: the mark and the name (after the traffic lights on macOS),
        // or the menus elsewhere.
        let mut x = rect.left() + if cfg!(target_os = "macos") { 84.0 } else { 8.0 };
        if !cfg!(target_os = "macos") {
            let menus = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(rect.left() + 400.0, rect.bottom()));
            let mut mui = ui.new_child(egui::UiBuilder::new().max_rect(menus));
            let layout = &self.layout;
            let extras = self.menu_extras();
            menu::draw(&mut mui, &self.keymap, &extras, &|k| layout.contains(k), &mut self.queue);
            x = menus.right() + 8.0;
        }
        let cy = rect.center().y;
        panels::mark(&painter, egui::pos2(x, cy), 18.0, &pal);
        let name = painter.text(egui::pos2(x + 26.0, cy), Align2::LEFT_CENTER, "grace", FontId::proportional(14.0), pal.text);
        painter.text(egui::pos2(name.right() + 1.0, cy + 0.5), Align2::LEFT_CENTER, "9000", FontId::monospace(12.5), pal.accent);

        // Middle: the photo.
        let photo = self.project.photo.as_ref().and_then(|f| f.file_name()).map_or("No photo".to_owned(), |s| s.to_string_lossy().into_owned());
        let photo = if self.dirty { format!("{photo}  •") } else { photo };
        painter.text(rect.center(), Align2::CENTER_CENTER, photo, FontId::proportional(12.0), pal.weak);

        // Right: tools.
        let tools = egui::Rect::from_min_max(egui::pos2(rect.center().x + 120.0, rect.top() + 4.0), egui::pos2(rect.right() - 10.0, rect.bottom() - 4.0));
        let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tools).layout(egui::Layout::right_to_left(egui::Align::Center)));
        tui.spacing_mut().item_spacing.x = 6.0;
        let can_export = self.photo.is_some() && self.export.is_none();
        let export = egui::Button::new(RichText::new("Export").size(12.0).color(pal.on_accent)).fill(pal.accent);
        if tui.add_enabled(can_export, export).on_hover_text("Export the photo with its edit").clicked() {
            self.queue.push("export".into());
        }
        if tui.add(egui::Button::new(RichText::new("Open").size(12.0))).on_hover_text("Open a photo").clicked() {
            self.queue.push("open".into());
        }
        let shown = self.layout.contains("adapters");
        if tui.add(egui::Button::selectable(shown, RichText::new("Adapters").size(12.0))).on_hover_text("Your adapters").clicked() {
            self.queue.push("panel adapters".into());
        }
        let layout = egui::Button::selectable(self.layout_ui.editing, RichText::new("Layout").size(12.0));
        if tui.add(layout).on_hover_text("Move, add and remove panels (or drag any panel's header)").clicked() {
            self.queue.push("layout edit".into());
        }
    }

    /// Reads a photo on another thread; `take_loaded` shows it. `with` is
    /// the edit to give it, from a `.grace` file or the session.
    fn read(&mut self, path: PathBuf, with: Option<Project>) {
        self.reading = Some(path.clone());
        let (tx, tools, ctx) = (self.loaded_tx.clone(), self.tools.clone(), self.ctx.clone());
        let (openers, env) = (self.adapters.openers(), self.adapters.env.clone());
        std::thread::spawn(move || {
            let r = image::load(tools.as_ref(), &path, &openers, &env).map_err(|e| format!("{}: {e}", path.display()));
            let _ = tx.send((path, r, with));
            ctx.request_repaint();
        });
    }

    fn take_loaded(&mut self) {
        while let Ok((path, r, with)) = self.loaded_rx.try_recv() {
            // Only the last one asked for counts.
            if self.reading.as_ref() != Some(&path) {
                continue;
            }
            self.reading = None;
            match r {
                Ok(photo) => {
                    log::info(format!("Opened {} ({}×{}, {} bits, {})", photo.name(), photo.width, photo.height, photo.bits(), photo.format));
                    let photo = Arc::new(photo);
                    let mut project = with.unwrap_or_default();
                    project.edit.tidy();
                    project.photo = Some(path);
                    self.file = project.file.take();
                    self.project = project;
                    self.photo = Some(photo);
                    self.texture = None;
                    self.histogram = None;
                    self.view.zoom = None;
                    self.view.center = [0.5, 0.5];
                    self.view.shown = None;
                    self.preview.again();
                    self.cropping = false;
                    self.compare = false;
                    self.undo.clear();
                    self.redo.clear();
                    self.selected = None;
                    self.dirty = false;
                    self.session_dirty = true;
                    self.emit_changed = true;
                    self.adapters.emit("opened", serde_json::json!({ "path": self.project.photo }));
                }
                Err(e) => self.say(e),
            }
        }
    }

    /// An edit about to be replaced with unsaved changes goes to
    /// previous.grace in the settings folder, so it isn't lost.
    fn keep_previous(&self) {
        if self.dirty && self.photo.is_some() && !self.project.edit.is_untouched() {
            let mut p = self.project.clone();
            p.grace9000 = FORMAT;
            p.file = None;
            if let Ok(text) = serde_json::to_string_pretty(&p) {
                let _ = write_atomic(&self.dir.join("previous.grace"), &text);
                log::info("The last photo's unsaved edit is in previous.grace in the settings folder.");
            }
        }
    }

    fn open_project(&mut self, path: &Path) {
        match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Project>(&t).map_err(|e| e.to_string())) {
            Ok(mut p) => match p.photo.clone() {
                Some(photo) => {
                    // A photo named relative to the edit, or moved beside it.
                    let beside = path.parent().map(|d| d.join(&photo)).filter(|b| b.is_file());
                    let moved = photo.file_name().and_then(|n| path.parent().map(|d| d.join(n))).filter(|b| b.is_file());
                    let photo = if photo.is_file() { photo } else { beside.or(moved).unwrap_or(photo) };
                    p.file = Some(path.to_path_buf());
                    self.keep_previous();
                    self.read(photo, Some(p));
                }
                None => self.say(format!("{} names no photo", path.display())),
            },
            Err(e) => log::error(format!("Couldn't open {}: {e}", path.display())),
        }
    }

    /// Exports the photo, in settings.toml's format or an adapter's `preset`.
    fn start_export(&mut self, preset: Option<(String, String)>) {
        if self.export.is_some() {
            return;
        }
        let Some(photo) = self.photo.clone() else {
            log::error("Nothing to export yet: open a photo first.");
            return;
        };
        let spec = match &preset {
            Some((a, id)) => match self.adapters.export(a, id) {
                Some(s) => Some(s.clone()),
                None => {
                    log::error(format!("No export format \"{id}\" in {a}"));
                    return;
                }
            },
            None => None,
        };
        let name = self.project.photo.as_ref().and_then(|f| f.file_stem()).map_or("photo".into(), |s| s.to_string_lossy().into_owned());
        let ext = spec.as_ref().map_or(self.settings.export.format.to_lowercase(), |s| s.extension.clone());
        let label = export::EXTENSIONS.iter().find(|e| e.0 == ext).map_or(ext.to_uppercase(), |e| e.1.to_owned());
        let mut dialog = rfd::FileDialog::new().add_filter(spec.as_ref().map_or(label.as_str(), |s| s.label.as_str()), &[ext.as_str()]).set_file_name(format!("{name} edit.{ext}"));
        if let Some(folder) = self.project.photo.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(folder);
        }
        let Some(out) = dialog.save_file() else { return };
        if self.project.photo.as_ref() == Some(&out) {
            self.say("That's the photo itself: grace9000 never writes over it. Pick another name.");
            return;
        }
        let ext = out.extension().map_or(ext, |e| e.to_string_lossy().to_lowercase());
        let Some(how) = export::how(&ext, &self.settings.export, photo.bits(), spec.as_ref()) else {
            self.say(format!("grace9000 doesn't know how to write .{ext} files: try jpg, png, webp or tif."));
            return;
        };
        if let Some(missing) = self.missing_encoder(&how) {
            self.say(format!("Your ffmpeg has no {missing} encoder, so it can't write that format."));
            return;
        }
        let long = spec.as_ref().map(|s| s.long_edge).filter(|&l| l > 0).unwrap_or(self.settings.export.long_edge);
        let ctx = self.ctx.clone();
        self.last_export = None;
        self.export = Some(Job::start(photo, self.project.edit.clone(), long, self.tools.clone(), how, out, move || ctx.request_repaint()));
    }

    /// The encoder a format needs that this ffmpeg doesn't have.
    pub fn missing_encoder(&mut self, how: &export::How) -> Option<String> {
        let enc = export::encoder(how)?;
        let Some(tools) = &self.tools else { return Some(enc.to_owned()) };
        let have = self.encoders.get_or_insert_with(|| tools.encoders());
        // An empty list: couldn't tell; let ffmpeg say.
        (!have.is_empty() && !have.iter().any(|e| e == enc)).then(|| enc.to_owned())
    }

    pub fn cancel_export(&self) {
        if let Some(job) = &self.export {
            job.cancel();
        }
    }

    fn check_export(&mut self) {
        let Some(job) = &self.export else { return };
        let finished = job.progress.lock().unwrap_or_else(|e| e.into_inner()).finished.take();
        if let Some(r) = finished {
            match &r {
                Ok(s) => {
                    log::info(format!("Exported {}: {s}", job.out.display()));
                    self.adapters.emit("exported", serde_json::json!({ "path": job.out }));
                }
                Err(e) => log::error(format!("Export failed: {e}")),
            }
            self.last_export = Some(r);
            self.export = None;
        }
    }

    /// A finished preview becomes the texture the Photo panel shows.
    fn take_preview(&mut self) {
        if let Some(done) = self.preview.take() {
            self.view.shown = Some(done.view);
            self.histogram = Some(done.histogram);
            self.preview_ms = done.ms;
            match &mut self.texture {
                Some(t) => t.set(done.image, TextureOptions::LINEAR),
                None => self.texture = Some(self.ctx.load_texture("photo", done.image, TextureOptions::LINEAR)),
            }
        }
    }

    pub fn ask_preview(&mut self, view: develop::View) {
        if let Some(photo) = &self.photo {
            self.preview.ask(preview::Request { photo: photo.clone(), edit: self.project.edit.clone(), view });
        }
    }

    fn save_layout(&self) {
        if let Err(e) = write_atomic(&self.dir.join("layout.toml"), &layout::to_toml(&self.layout)) {
            log::error(format!("Couldn't save layout.toml: {e}"));
        }
    }

    fn save_session(&mut self) {
        let mut p = self.project.clone();
        p.grace9000 = FORMAT;
        p.file = self.file.clone();
        if let Ok(text) = serde_json::to_string(&p) {
            let _ = write_atomic(&self.dir.join("session.json"), &text);
        }
        self.session_dirty = false;
    }

    fn dropped_files(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        self.open_paths(paths);
    }

    /// Opens dropped files, or files named when grace9000 was started:
    /// adapters are offered for install, an edit opens with its photo, and
    /// a photo opens. One photo at a time.
    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let (packages, paths): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| adapters::is_package(p));
        for p in packages {
            self.offer_install(p);
        }
        let Some(first) = paths.first() else { return };
        if paths.len() > 1 {
            self.say(format!("grace9000 edits one photo at a time: opened {}", first.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        }
        if first.extension().is_some_and(|x| x.eq_ignore_ascii_case("grace")) {
            self.open_project(first);
        } else {
            self.keep_previous();
            self.read(first.clone(), None);
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self.project.photo.as_ref().and_then(|f| f.file_name()).map(|s| s.to_string_lossy().into_owned());
        let title = match name {
            Some(n) => format!("{n}{} — grace9000", if self.dirty { " •" } else { "" }),
            None => "grace9000".to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl shell::App for App {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.config_changed.swap(false, Ordering::Relaxed) {
            self.load_config(false);
        }
        self.dropped_files(&ctx);
        self.open_paths(incoming::take());
        self.serve_adapters();
        self.finish_renders();
        let mut commands = std::mem::take(&mut self.queue);
        commands.extend(self.menu.take());
        // While a question is on screen, keys go to it, not to the photo.
        if self.prompt.is_none() {
            let done = |i: &mut egui::InputState| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) || i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            if self.cropping && !ctx.egui_wants_keyboard_input() && ctx.input_mut(done) {
                self.cropping = false;
                self.view.zoom = None;
            }
            commands.extend(self.keymap.take(&ctx));
        }
        if self.layout_ui.editing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.layout_ui.editing = false;
        }
        for c in commands {
            self.run(&c);
        }
        self.take_loaded();
        self.take_preview();
        self.check_export();

        // The traffic lights follow the system theme: match it to ours.
        if self.system_dark != Some(self.palette.dark) {
            self.system_dark = Some(self.palette.dark);
            let theme = if self.palette.dark { egui::SystemTheme::Dark } else { egui::SystemTheme::Light };
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(theme));
        }
        let full = ui.max_rect();
        let strip = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), TOP));
        self.top_strip(ui, strip);
        let rect = egui::Rect::from_min_max(egui::pos2(full.left(), strip.bottom()), full.max);
        let mut root = std::mem::replace(&mut self.layout, Node::panel("photo"));
        let mut lui = std::mem::take(&mut self.layout_ui);
        let palette = self.palette;
        let headers = self.settings.look.panel_headers;
        let titles: HashMap<String, String> = self
            .adapters
            .all
            .iter()
            .flat_map(|a| a.manifest.panels.iter().map(move |p| (format!("adapter:{}/{}", a.manifest.name, p.id), p.title.clone())))
            .collect();
        let title = |k: &str| titles.get(k).cloned().unwrap_or_else(|| layout::title(k).to_owned());
        let out = lui.show(
            ui,
            &mut root,
            rect,
            &mut layout::Frame { palette: &palette, headers, panel: &mut |kind, part, ui| panels::draw(self, kind, part, ui), title: &title },
        );
        self.layout = root;
        self.layout_ui = lui;
        if out.changed {
            self.save_layout();
        }
        if self.layout_ui.editing {
            panels::edit_banner(ui, rect, &palette);
        }
        let layout = &self.layout;
        self.menu.sync(&|k| layout.contains(k));
        self.show_prompt(&ctx);
        panels::note(ui, rect, self);
        if std::mem::take(&mut self.emit_changed) {
            let summary = serde_json::json!({ "steps": self.project.edit.steps.len(), "photo": self.project.photo });
            self.adapters.emit("changed", summary);
        }
        if self.session_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.save_session();
        }
        self.update_title(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_session();
            self.adapters.stop_all();
        }
    }
}

fn open_folder(dir: &Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        log::error(format!("Couldn't open {}: {e}", dir.display()));
    }
}

#[cfg(test)]
mod tests {
    //! The real app, driven through egui with pointer and key events and
    //! drawn by the shell's own rasterizer, so whole features are tested
    //! the way a person uses them. With GRACE9000_SHOTS=<folder> it saves a
    //! PNG of each step (and GRACE9000_SHOT_PHOTO=<file> picks the photo).

    use super::*;
    use crate::shell::raster::{Canvas, Frame, Target, Textures};
    use egui::{Event, Key, Modifiers, PointerButton, Pos2};

    const W: usize = 1400;
    const H: usize = 880;

    struct Harness {
        ctx: egui::Context,
        app: App,
        textures: Textures,
        canvas: Canvas,
        px: Vec<u32>,
        _dir: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("grace9000-ui-{}-{:?}", std::process::id(), std::thread::current().id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let ctx = egui::Context::default();
            let app = App::new(&ctx, dir.clone());
            Self { ctx, app, textures: Textures::default(), canvas: Canvas::default(), px: vec![0; W * H], _dir: dir }
        }

        fn frame(&mut self, events: Vec<Event>) {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(W as f32, H as f32))), events, ..Default::default() };
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| shell::App::ui(app, ui));
            self.textures.set(&out.textures_delta);
            let frame = Frame {
                shapes: out.shapes,
                pixels_per_point: 1.0,
                options: self.ctx.tessellation_options(|o| *o),
                atlas_size: self.ctx.fonts(|f| f.font_image_size()),
                clear: self.ctx.global_style().visuals.panel_fill,
            };
            let mut target = Target { px: &mut self.px, stride: W, width: W, height: H };
            self.canvas.paint(frame, &self.textures, &mut target, 0);
            self.textures.free(&mut out.textures_delta);
        }

        /// Frames until `done`, up to a few seconds (reading and previews
        /// happen on other threads).
        fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            for _ in 0..300 {
                self.frame(vec![]);
                if done(&self.app) {
                    // One more, to draw what just arrived.
                    self.frame(vec![]);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("waited too long for {what}");
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) {
            for pressed in [true, false] {
                self.frame(vec![Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }]);
            }
        }

        fn drag(&mut self, from: Pos2, to: Pos2) {
            let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame(vec![Event::PointerMoved(from)]);
            self.frame(vec![button(from, true)]);
            for k in 1..=8 {
                self.frame(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 8.0))]);
            }
            self.frame(vec![button(to, false)]);
            self.frame(vec![]);
        }

        fn shot(&self, name: &str) {
            let Some(dir) = std::env::var_os("GRACE9000_SHOTS") else { return };
            let bytes: Vec<u8> = self.px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            let file = std::fs::File::create(PathBuf::from(dir).join(format!("{name}.png"))).unwrap();
            let mut enc = png::Encoder::new(file, W as u32, H as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header().unwrap().write_image_data(&bytes).unwrap();
        }

        /// Waits for the preview of the edit as it is now.
        fn settle(&mut self) {
            let want = self.app.project.edit.clone();
            self.until("the preview", |a| a.texture.is_some() && a.preview.last.as_ref().is_some_and(|r| r.edit == want));
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(30));
                self.frame(vec![]);
            }
        }
    }

    fn sample_photo() -> PathBuf {
        if let Some(p) = std::env::var_os("GRACE9000_SHOT_PHOTO") {
            return p.into();
        }
        let path = std::env::temp_dir().join(format!("grace9000-ui-photo-{}.png", std::process::id()));
        let (w, h) = (600u32, 400u32);
        let px: Vec<u8> = (0..w * h).flat_map(|i| [((i % w) * 255 / w) as u8, ((i / w) * 255 / h) as u8, 120, 255]).collect();
        let mut enc = png::Encoder::new(std::fs::File::create(&path).unwrap(), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().unwrap().write_image_data(&px).unwrap();
        path
    }

    #[test]
    fn editing_a_photo_end_to_end() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.shot("0-empty");
        h.app.open_paths(vec![sample_photo()]);
        h.until("the photo", |a| a.photo.is_some());
        h.settle();
        h.shot("1-opened");

        // Steps from the menu commands, selected with the arrow keys.
        for k in ["exposure", "tone", "curve", "vignette"] {
            h.app.queue.push(format!("add {k}"));
        }
        h.frame(vec![]);
        assert_eq!(h.app.project.edit.steps.len(), 4);
        h.key(Key::ArrowUp, Modifiers::NONE);
        h.key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(h.app.selected, Some(1));
        h.app.project.edit.steps[1].values.insert("shadows".into(), 50.0);
        h.app.project.edit.steps[0].values.insert("ev".into(), 0.4);
        h.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(h.app.selected, Some(2));
        h.settle();
        h.shot("2-curve");

        // Off and on with e; alt+up moves it; backspace removes it; undo.
        h.key(Key::E, Modifiers::NONE);
        assert!(!h.app.project.edit.steps[2].on);
        h.key(Key::E, Modifiers::NONE);
        h.key(Key::ArrowUp, Modifiers::ALT);
        assert_eq!(h.app.project.edit.steps[1].kind, "curve");
        h.key(Key::Backspace, Modifiers::NONE);
        assert_eq!(h.app.project.edit.steps.len(), 3);
        h.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(h.app.project.edit.steps.len(), 4);

        // Crop: c, then drag the top-left corner in, then Return.
        h.key(Key::C, Modifiers::NONE);
        assert!(h.app.cropping);
        h.settle();
        let img = h.app.view.area.expect("drawn");
        h.drag(img.min + egui::vec2(1.0, 1.0), img.min + egui::vec2(img.width() * 0.2, img.height() * 0.15));
        let c = h.app.project.edit.crop.expect("cropped");
        assert!((c.x - 0.2).abs() < 0.02 && (c.y - 0.15).abs() < 0.02, "{c:?}");
        h.key(Key::CloseBracket, Modifiers::NONE);
        assert!(h.app.project.edit.straighten > 0.0);
        h.settle();
        h.shot("3-cropping");
        h.key(Key::Enter, Modifiers::NONE);
        assert!(!h.app.cropping);
        h.settle();
        h.shot("4-cropped");

        // Before and after.
        h.key(Key::Backslash, Modifiers::NONE);
        assert!(h.app.compare);
        h.settle();
        h.shot("5-before");
        h.key(Key::Backslash, Modifiers::NONE);

        // Zoom to the photo's own pixels and back.
        h.key(Key::Num1, Modifiers::COMMAND);
        assert_eq!(h.app.view.zoom, Some(1.0));
        h.settle();
        h.shot("6-100");
        h.key(Key::Num0, Modifiers::COMMAND);
        assert_eq!(h.app.view.zoom, None);

        // The session keeps it all.
        h.app.save_session();
        let saved: Project = serde_json::from_str(&std::fs::read_to_string(h.app.dir.join("session.json")).unwrap()).unwrap();
        assert_eq!(saved.edit, h.app.project.edit);
    }
}
