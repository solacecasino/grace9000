//! What grace9000 does for adapters: answers their requests, asks before
//! installing or first running one, and puts their commands, keys, panels
//! and export formats where people find them.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::adapters::{self, EDIT_CHANGE, EDIT_READ, Manifest, Message, StartError, command_name, parse_command};
use crate::layout::Side;
use crate::project::Edit;
use crate::{App, log, menu};

/// A question on screen, waiting for the user.
pub enum Prompt {
    /// A dropped adapter package: install it?
    Install { path: PathBuf, manifest: Box<Manifest> },
    /// An installed adapter's first run: allow it? Then run `then`.
    Allow { adapter: String, then: Option<String> },
    /// `ui.confirm` from an adapter.
    Confirm { adapter: String, id: Value, title: String, message: String },
    /// `ui.prompt` from an adapter.
    Ask { adapter: String, id: Value, title: String, message: String, text: String },
}

/// An `image.render` being written for an adapter.
pub struct Render {
    adapter: String,
    id: Value,
    job: crate::export::Job,
    size: (usize, usize),
}

impl App {
    /// Adapters' default keys, for the keymap.
    pub fn adapter_keys(&self) -> Vec<(String, String)> {
        let mut keys = Vec::new();
        for a in &self.adapters.all {
            for c in &a.manifest.commands {
                if let Some(k) = &c.keys {
                    keys.push((k.clone(), command_name("adapter", &a.manifest.name, &c.id)));
                }
            }
        }
        keys
    }

    /// What adapters add to the menus.
    pub fn menu_extras(&self) -> menu::Extras {
        let mut extras = menu::Extras::default();
        for a in &self.adapters.all {
            let name = &a.manifest.name;
            for c in &a.manifest.commands {
                extras.commands.push((c.menu.clone().unwrap_or_default(), command_name("adapter", name, &c.id), c.label.clone()));
            }
        }
        extras.exports = self.adapters.exports().map(|(a, e)| (command_name("export", a, &e.id), e.label.clone())).collect();
        extras
    }

    /// Keys and menus again, after adapters or keys.toml changed.
    pub fn refresh_commands(&mut self) {
        let text = std::fs::read_to_string(self.dir.join("keys.toml")).unwrap_or_default();
        let (keymap, problems) = crate::keys::Keymap::load(&text, &self.adapter_keys());
        problems.into_iter().for_each(log::error);
        self.keymap = keymap;
        self.menu = menu::MenuBar::install(&self.keymap, &self.ctx, crate::icon_rgba(), &self.menu_extras());
        // A chosen export format that's gone falls back to settings.toml's.
        if let Some((a, id)) = &self.export_choice
            && self.adapters.export(a, id).is_none()
        {
            self.export_choice = None;
        }
    }

    /// Commands that act on adapters. Returns false if `command` isn't one.
    pub fn run_adapter_command(&mut self, command: &str) -> bool {
        if let Some((name, id)) = parse_command(command, "adapter") {
            let (name, id) = (name.to_owned(), id.to_owned());
            match self.adapters.run_command(&name, &id) {
                Ok(()) => {}
                Err(StartError::NeedsApproval) => self.ask(Prompt::Allow { adapter: name, then: Some(id) }),
                Err(StartError::Failed(e)) => self.say(e),
            }
        } else if let Some((name, id)) = parse_command(command, "export") {
            self.start_export(Some((name.to_owned(), id.to_owned())));
        } else if let Some(name) = quoted(command, "adapter-remove") {
            match self.adapters.remove(&name) {
                Ok(()) => {
                    self.forget_adapter_panels(&name);
                    self.refresh_commands();
                    self.say(format!("Removed {name}"));
                }
                Err(e) => self.say(format!("Couldn't remove {name}: {e}")),
            }
        } else if let Some(name) = quoted(command, "adapter-add-back") {
            match self.adapters.add_back(&name) {
                Ok(()) => {
                    self.refresh_commands();
                    self.say(format!("Added {name} back"));
                }
                Err(e) => self.say(format!("Couldn't add {name} back: {e}")),
            }
        } else if let Some(name) = quoted(command, "adapter-start") {
            self.start_adapter(&name);
        } else {
            match command {
                "install adapter" => {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Adapter", &["zip"]).set_title("Install Adapter").pick_file() {
                        self.offer_install(path);
                    }
                }
                "install adapter folder" => {
                    if let Some(path) = rfd::FileDialog::new().set_title("Install Adapter from a Folder").pick_folder() {
                        self.offer_install(path);
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Starts an adapter (for its panel), asking first if it must.
    pub fn start_adapter(&mut self, name: &str) {
        let Some(i) = self.adapters.index(name) else { return };
        match self.adapters.start(i) {
            Ok(_) => {}
            Err(StartError::NeedsApproval) => self.ask(Prompt::Allow { adapter: name.to_owned(), then: None }),
            Err(StartError::Failed(e)) => self.say(e),
        }
    }

    /// Shows or hides an adapter's panel, beside the photo like the others.
    pub fn toggle_adapter_panel(&mut self, adapter: &str, id: &str) {
        let kind = format!("adapter:{adapter}/{id}");
        let mut at = Vec::new();
        if self.layout.find(&kind, &mut at) {
            if !at.is_empty() {
                self.layout.remove(&at);
            }
        } else {
            let mut beside = Vec::new();
            if !self.layout.find("photo", &mut beside) {
                beside.clear();
            }
            self.layout.insert(&beside, &kind, Side::Right);
        }
        self.save_layout();
    }

    /// What a stopped adapter showed goes with it.
    fn forget_adapter_panels(&mut self, adapter: &str) {
        let prefix = format!("adapter:{adapter}/");
        self.adapter_panels.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Offers to install a dropped or picked package.
    pub fn offer_install(&mut self, path: PathBuf) {
        match adapters::package_manifest(&path) {
            Ok(manifest) => self.ask(Prompt::Install { path, manifest: Box::new(manifest) }),
            Err(e) => self.say(format!("{}: {e}", path.display())),
        }
    }

    fn ask(&mut self, prompt: Prompt) {
        if self.prompt.is_none() {
            self.prompt = Some(prompt);
        } else {
            self.prompts.push(prompt);
        }
    }

    /// A short message at the bottom of the window, and in the Log.
    pub fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::info(text.clone());
        let now = self.ctx.input(|i| i.time);
        self.note = Some((text, now + 4.0));
        self.ctx.request_repaint_after(std::time::Duration::from_secs_f64(4.1));
    }

    /// Handles what adapters sent since the last frame.
    pub fn serve_adapters(&mut self) {
        while let Ok(message) = self.adapter_rx.try_recv() {
            match message {
                Message::Stopped { adapter, generation } => {
                    if self.adapters.stopped(&adapter, generation) {
                        self.forget_adapter_panels(&adapter);
                    }
                }
                Message::Call { adapter, id, method, params } => {
                    if let Some(result) = self.call(&adapter, id.clone(), &method, params)
                        && let Some(id) = id
                    {
                        self.adapters.reply(&adapter, id, result);
                    }
                }
            }
        }
    }

    /// One request or notification. None: the answer comes later (a prompt).
    fn call(&mut self, adapter: &str, id: Option<Value>, method: &str, params: Value) -> Option<Result<Value, String>> {
        let Some(i) = self.adapters.index(adapter) else { return Some(Err("unknown adapter".into())) };
        let may = |p: &str| self.adapters.all[i].has(p);
        let need = |p: &str| if may(p) { Ok(()) } else { Err(format!("{method} needs the \"{p}\" permission in adapter.toml")) };
        let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(match method {
            "log" => {
                log::info(format!("{adapter}: {}", text("message")));
                Ok(Value::Null)
            }
            "notify" => {
                self.say(format!("{adapter}: {}", text("text")));
                Ok(Value::Null)
            }
            "panel.set" => {
                let lines = params.get("lines").and_then(Value::as_array).cloned().unwrap_or_default();
                self.adapter_panels.insert(format!("adapter:{adapter}/{}", text("id")), lines);
                self.ctx.request_repaint();
                Ok(Value::Null)
            }
            "edit.get" => need(EDIT_READ).map(|()| self.edit_json()),
            "edit.set" => need(EDIT_CHANGE).and_then(|()| self.set_edit(&params)),
            "edit.import" => need(EDIT_CHANGE).map(|()| {
                // The first photo opens; grace9000 edits one at a time.
                let path = params.get("paths").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).next();
                if let Some(p) = path {
                    self.open_paths(vec![p]);
                }
                Value::Null
            }),
            "image.render" => {
                if let Err(e) = need(EDIT_READ) {
                    return Some(Err(e));
                }
                let Some(id) = id else { return Some(Err("image.render is a request: give it an id".into())) };
                let path = text("path");
                if path.is_empty() {
                    return Some(Err("needs `path`, where the PNG goes".into()));
                }
                let long = params.get("long_edge").and_then(Value::as_u64).unwrap_or(0) as u32;
                let bits = params.get("bits").and_then(Value::as_u64).unwrap_or(0) as u32;
                return match self.render_for(adapter, id.clone(), PathBuf::from(path), long, bits) {
                    Ok(()) => None,
                    Err(e) => Some(Err(e)),
                };
            }
            "ui.confirm" | "ui.prompt" => {
                let Some(id) = id else { return Some(Err("ui prompts are requests: give them an id".into())) };
                let (adapter, title, message) = (adapter.to_owned(), text("title"), text("message"));
                self.ask(if method == "ui.confirm" {
                    Prompt::Confirm { adapter, id, title, message }
                } else {
                    Prompt::Ask { adapter, id, title, message, text: text("default") }
                });
                return None;
            }
            "ui.save_dialog" => {
                let mut d = rfd::FileDialog::new().set_title(text("title")).set_file_name(text("file_name"));
                let ext = text("extension");
                if !ext.is_empty() {
                    d = d.add_filter(&ext, &[ext.as_str()]);
                }
                Ok(d.save_file().map_or(Value::Null, |p| json!(p)))
            }
            "ui.open_dialog" => {
                let mut d = rfd::FileDialog::new().set_title(text("title"));
                let exts: Vec<String> = params.get("extensions").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                if !exts.is_empty() {
                    d = d.add_filter("Files", &exts);
                }
                Ok(d.pick_file().map_or(Value::Null, |p| json!(p)))
            }
            other => Err(format!("grace9000 doesn't know \"{other}\"")),
        })
    }

    /// The photo and its edit as adapters see them.
    pub fn edit_json(&self) -> Value {
        let size = self.photo.as_ref().map(|p| {
            let (w, h) = self.project.edit.size(p.width, p.height);
            json!({ "width": p.width, "height": p.height, "bits": p.bits(), "result": [w.round(), h.round()] })
        });
        json!({
            "photo": self.project.photo,
            "size": size,
            "file": self.file,
            "selected": self.selected,
            "edit": self.project.edit,
        })
    }

    /// `edit.set`: the edit replaced in one undo step, tidied so an adapter
    /// can't make one grace9000 couldn't have made. Steps of kinds grace9000
    /// doesn't know are kept and skipped.
    fn set_edit(&mut self, params: &Value) -> Result<Value, String> {
        let v = params.get("edit").ok_or("needs `edit`")?;
        let mut edit: Edit = serde_json::from_value(v.clone()).map_err(|e| format!("edit: {e}"))?;
        edit.tidy();
        self.snapshot();
        self.project.edit = edit;
        self.selected = self.selected.filter(|&s| s < self.project.edit.steps.len());
        Ok(Value::Null)
    }

    /// `image.render`: the photo with its edit, as a PNG at `path`, answered
    /// when it's written.
    fn render_for(&mut self, adapter: &str, id: Value, path: PathBuf, long_edge: u32, bits: u32) -> Result<(), String> {
        let photo = self.photo.clone().ok_or("no photo is open")?;
        let bits = match bits {
            8 | 16 => bits,
            _ => photo.bits(),
        };
        let (w, h, _) = crate::export::size(&photo, &self.project.edit, long_edge);
        let job = crate::export::Job::start(photo, self.project.edit.clone(), long_edge, None, crate::export::How::Png(bits), path, {
            let ctx = self.ctx.clone();
            move || ctx.request_repaint()
        });
        self.renders.push(Render { adapter: adapter.to_owned(), id, job, size: (w, h) });
        Ok(())
    }

    /// Answers `image.render` requests that have finished.
    pub fn finish_renders(&mut self) {
        let mut i = 0;
        while i < self.renders.len() {
            let finished = self.renders[i].job.progress.lock().unwrap_or_else(|e| e.into_inner()).finished.take();
            match finished {
                Some(r) => {
                    let done = self.renders.remove(i);
                    let result = r.map(|_| json!({ "path": done.job.out, "width": done.size.0, "height": done.size.1 }));
                    self.adapters.reply(&done.adapter, done.id, result);
                }
                None => i += 1,
            }
        }
    }

    /// Draws the prompt waiting for an answer, if any. Escape declines and
    /// Return does nothing: saying yes takes a click (or Tab, then Space).
    pub fn show_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompt.take() else { return };
        let pal = self.palette;
        let mut answer: Option<bool> = None;
        let mut typed = None;
        let modal = egui::Modal::new(egui::Id::unique("adapter prompt")).show(ctx, |ui| {
            ui.set_width(380.0);
            let (title, lines, yes): (String, Vec<String>, &str) = match &prompt {
                Prompt::Install { manifest, .. } => ("Install this adapter?".into(), adapters::summary(manifest), "Install"),
                Prompt::Allow { adapter, .. } => {
                    let lines = self.adapters.index(adapter).map(|i| adapters::summary(&self.adapters.all[i].manifest)).unwrap_or_default();
                    (format!("Allow {adapter} to run?"), lines, "Allow")
                }
                Prompt::Confirm { adapter, title, message, .. } => (format!("{adapter}: {title}"), vec![message.clone()], "OK"),
                Prompt::Ask { adapter, title, message, .. } => (format!("{adapter}: {title}"), vec![message.clone()], "OK"),
            };
            ui.label(egui::RichText::new(title).size(15.0).strong());
            ui.add_space(6.0);
            for line in lines {
                ui.label(line);
            }
            if matches!(prompt, Prompt::Install { .. } | Prompt::Allow { .. }) {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Adapters are separate programs that run as you. Only allow ones you trust.").small().color(pal.weak));
            }
            if let Prompt::Ask { text, .. } = &prompt {
                let mut t = text.clone();
                ui.text_edit_singleline(&mut t);
                typed = Some(t);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let yes = egui::Button::new(egui::RichText::new(yes).color(pal.on_accent)).fill(pal.accent);
                if ui.add(yes).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        let mut prompt = prompt;
        if let (Prompt::Ask { text, .. }, Some(t)) = (&mut prompt, typed) {
            *text = t;
        }
        if answer.is_none() && (modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            answer = Some(false);
        }
        let Some(yes) = answer else {
            self.prompt = Some(prompt);
            return;
        };
        self.answer(prompt, yes);
        if self.prompt.is_none() && !self.prompts.is_empty() {
            self.prompt = Some(self.prompts.remove(0));
        }
    }

    fn answer(&mut self, prompt: Prompt, yes: bool) {
        match prompt {
            Prompt::Install { path, manifest } if yes => match adapters::install(&path, &self.dir) {
                Ok(m) => {
                    // The prompt showed what it may do: that's its approval.
                    adapters::remember_approval(&self.dir, &m);
                    self.adapters.reload();
                    self.refresh_commands();
                    self.adapters.start_launch_adapters();
                    self.say(format!("Installed {} {}", m.name, m.version));
                    let _ = manifest;
                }
                Err(e) => self.say(e),
            },
            Prompt::Allow { adapter, then } if yes => {
                if let Some(i) = self.adapters.index(&adapter) {
                    self.adapters.allow(i);
                    match then {
                        Some(id) => self.run(&command_name("adapter", &adapter, &id)),
                        None => self.start_adapter(&adapter),
                    }
                }
            }
            Prompt::Confirm { adapter, id, .. } => self.adapters.reply(&adapter, id, Ok(json!(yes))),
            Prompt::Ask { adapter, id, text, .. } => self.adapters.reply(&adapter, id, Ok(if yes { json!(text) } else { Value::Null })),
            _ => {}
        }
    }
}

/// `adapter-remove "Name"` and the like, from buttons.
fn quoted(command: &str, verb: &str) -> Option<String> {
    let rest = command.strip_prefix(verb)?.strip_prefix(" \"")?;
    Some(rest.strip_suffix('"')?.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_button_commands() {
        assert_eq!(super::quoted(r#"adapter-remove "Frame Grab""#, "adapter-remove").as_deref(), Some("Frame Grab"));
        assert_eq!(super::quoted(r#"adapter-remove Frame"#, "adapter-remove"), None);
    }
}
