//! Named commands and the keys that run them.
//!
//! `COMMANDS` is the one list of everything grace9000 can do. Key
//! bindings, the Help panel and the buttons all run commands by name, and
//! keys.toml binds any key to any of them.

use std::collections::BTreeMap;

use egui::{Key, KeyboardShortcut, Modifiers};

/// Name, default key, what it does.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("open", "cmd+o", "Open a photo or a grace9000 edit"),
    ("save", "cmd+s", "Save the edit"),
    ("save as", "cmd+shift+s", "Save the edit under a new name"),
    ("export", "cmd+e", "Export the photo with the edit"),
    ("close", "cmd+w", "Close the photo"),
    ("undo", "cmd+z", "Undo"),
    ("redo", "cmd+shift+z", "Redo"),
    ("compare", "backslash", "Show the photo without the steps, or with them again"),
    ("crop", "c", "Crop and straighten, or finish"),
    ("crop reset", "", "Keep the whole photo"),
    ("rotate left", "cmd+left", "Turn a quarter to the left"),
    ("rotate right", "cmd+right", "Turn a quarter to the right"),
    ("flip horizontal", "", "Mirror left to right"),
    ("flip vertical", "", "Mirror top to bottom"),
    ("straighten left", "openbracket", "Straighten a little to the left"),
    ("straighten right", "closebracket", "Straighten a little to the right"),
    ("step delete", "backspace", "Remove the selected step"),
    ("step toggle", "e", "Turn the selected step off or on"),
    ("step reset", "", "Put the selected step back to its defaults"),
    ("step up", "alt+up", "Move the selected step earlier"),
    ("step down", "alt+down", "Move the selected step later"),
    ("step next", "down", "Select the next step"),
    ("step previous", "up", "Select the previous step"),
    ("add exposure", "", "Add an Exposure step"),
    ("add white balance", "", "Add a White Balance step"),
    ("add contrast", "", "Add a Contrast step"),
    ("add tone", "", "Add a Highlights & Shadows step"),
    ("add levels", "", "Add a Levels step"),
    ("add color", "", "Add a Saturation step"),
    ("add curve", "", "Add a Curve step"),
    ("add black and white", "", "Add a Black & White step"),
    ("add vignette", "", "Add a Vignette step"),
    ("zoom in", "cmd+plus", "Zoom in"),
    ("zoom out", "cmd+minus", "Zoom out"),
    ("zoom fit", "cmd+0", "Fit the photo in its panel"),
    ("zoom 100", "cmd+1", "Show the photo's own pixels"),
    ("layout edit", "cmd+l", "Move, add and remove panels"),
    ("layout reset", "", "Put the panels back as they came"),
    ("config", "cmd+comma", "Open the folder with settings, keys and layout"),
    ("quit", "cmd+q", "Quit"),
    ("panel photo", "", "Show or hide the Photo panel"),
    ("panel steps", "", "Show or hide the Steps panel"),
    ("panel adjust", "", "Show or hide the Adjust panel"),
    ("panel histogram", "", "Show or hide the Histogram panel"),
    ("panel info", "", "Show or hide the Info panel"),
    ("panel export", "", "Show or hide the Export panel"),
    ("panel log", "", "Show or hide the Log panel"),
    ("panel help", "", "Show or hide the Help panel"),
    ("panel adapters", "", "Show or hide the Adapters panel"),
    ("install adapter", "", "Install an adapter from a zip or folder"),
];

pub const EXAMPLE: &str = r#"# grace9000 keys: "key" = "command". Saved changes apply at once.
# cmd is Cmd on macOS and Ctrl elsewhere; also ctrl, alt, shift.
# The commands are listed in the Help panel (and in COMMANDS in keys.rs).
# Adapters' commands work too: "cmd+g" = 'adapter "Name" id'
# and export formats: "cmd+shift+e" = 'export "Name" id'
# To turn a default key off, bind it to "none".
#
# "x" = "compare"
# "r" = "crop"
# "delete" = "step delete"
# "cmd+shift+x" = "add exposure"
"#;

pub struct Keymap {
    /// Most modifiers first, so cmd+shift+z wins over cmd+z.
    bindings: Vec<(KeyboardShortcut, String)>,
}

impl Keymap {
    /// The defaults, then adapters' own defaults where the key is free, with
    /// keys.toml's entries on top. Bad entries are skipped and returned as
    /// problems.
    pub fn load(text: &str, adapter_keys: &[(String, String)]) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut map: BTreeMap<String, String> =
            COMMANDS.iter().filter(|c| !c.1.is_empty()).map(|c| (c.1.to_owned(), c.0.to_owned())).collect();
        for (key, command) in adapter_keys {
            map.entry(key.clone()).or_insert_with(|| command.clone());
        }
        match toml::from_str::<BTreeMap<String, String>>(text) {
            Ok(user) => {
                for (key, cmd) in user {
                    if cmd != "none" && !is_command(&cmd) {
                        problems.push(format!("keys.toml: no command called \"{cmd}\""));
                        continue;
                    }
                    map.insert(key, cmd);
                }
            }
            Err(e) => problems.push(format!("keys.toml: {}", e.message())),
        }
        let mut bindings = Vec::new();
        for (key, cmd) in map {
            if cmd == "none" {
                continue;
            }
            match parse(&key) {
                Some(s) => bindings.push((s, cmd)),
                None => problems.push(format!("keys.toml: can't read the key \"{key}\"")),
            }
        }
        bindings.sort_by_key(|(s, _)| std::cmp::Reverse(count(s.modifiers)));
        (Self { bindings }, problems)
    }

    /// The commands whose keys were pressed this frame. While typing into a
    /// field only keys with cmd/ctrl count.
    pub fn take(&self, ctx: &egui::Context) -> Vec<String> {
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input_mut(|i| {
            self.bindings
                .iter()
                .filter(|(s, _)| !typing || s.modifiers.command || s.modifiers.ctrl)
                .filter(|(s, _)| i.consume_shortcut(s))
                .map(|(_, c)| c.clone())
                .collect()
        })
    }

    /// The key for `command`, as written in keys.toml.
    pub fn key_for(&self, command: &str) -> Option<String> {
        self.bindings.iter().find(|(_, c)| c == command).map(|(s, _)| describe(s))
    }
}

/// A built-in command, or an adapter's (`adapter "Name" id`, `export "Name" id`).
fn is_command(cmd: &str) -> bool {
    COMMANDS.iter().any(|c| c.0 == cmd)
        || crate::adapters::parse_command(cmd, "adapter").is_some()
        || crate::adapters::parse_command(cmd, "export").is_some()
}

fn count(m: Modifiers) -> u8 {
    m.alt as u8 + m.shift as u8 + m.ctrl as u8 + m.command as u8
}

fn parse(text: &str) -> Option<KeyboardShortcut> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.to_lowercase().split('+').map(str::trim) {
        match part {
            "cmd" | "command" => modifiers |= Modifiers::COMMAND,
            "ctrl" | "control" => modifiers |= Modifiers::CTRL,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            "" => key = Some(Key::Plus), // "cmd++"
            name => key = Some(key_named(name)?),
        }
    }
    Some(KeyboardShortcut::new(modifiers, key?))
}

fn key_named(name: &str) -> Option<Key> {
    Some(match name {
        "space" => Key::Space,
        "enter" | "return" => Key::Enter,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "plus" | "=" | "equals" => Key::Equals,
        "minus" | "-" => Key::Minus,
        "comma" | "," => Key::Comma,
        "backslash" | "\\" => Key::Backslash,
        "openbracket" | "[" => Key::OpenBracket,
        "closebracket" | "]" => Key::CloseBracket,
        "period" | "." => Key::Period,
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => Key::from_name(&c.to_ascii_uppercase().to_string())?,
                _ => Key::from_name(other).or_else(|| Key::from_name(&other.to_uppercase()))?,
            }
        }
    })
}

fn describe(s: &KeyboardShortcut) -> String {
    let m = s.modifiers;
    let mut parts = Vec::new();
    if m.command {
        parts.push("cmd");
    }
    if m.ctrl {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    let key = match s.logical_key {
        Key::Equals => "plus".to_owned(),
        Key::ArrowLeft => "left".to_owned(),
        Key::ArrowRight => "right".to_owned(),
        Key::ArrowUp => "up".to_owned(),
        Key::ArrowDown => "down".to_owned(),
        Key::Backslash => "backslash".to_owned(),
        Key::OpenBracket => "openbracket".to_owned(),
        Key::CloseBracket => "closebracket".to_owned(),
        k => k.name().to_lowercase(),
    };
    parts.push(&key);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let (k, problems) = Keymap::load("", &[]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(k.key_for("redo").as_deref(), Some("cmd+shift+z"));
        assert_eq!(k.key_for("zoom in").as_deref(), Some("cmd+plus"));
        assert_eq!(k.bindings.len(), COMMANDS.iter().filter(|c| !c.1.is_empty()).count());
        let (k, problems) = Keymap::load("\"k\" = \"compare\"\n\"c\" = \"none\"\n\"q\" = \"fly\"\n\"cmd+bogus\" = \"play\"\n\"cmd+g\" = 'adapter \"Frame Grab\" grab'", &[]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(k.key_for("crop").is_none());
        assert!(k.bindings.iter().any(|(s, c)| c == "compare" && s.logical_key == Key::K));
        assert_eq!(k.key_for("straighten left").as_deref(), Some("openbracket"));
        assert_eq!(k.key_for(r#"adapter "Frame Grab" grab"#).as_deref(), Some("cmd+g"));
        let (_, problems) = Keymap::load("not toml [", &[]);
        assert_eq!(problems.len(), 1);
        // An adapter's default key, unless something has it already.
        let extra = [("cmd+alt+g".to_owned(), "adapter \"A\" x".to_owned()), ("c".to_owned(), "adapter \"A\" y".to_owned())];
        let (k, _) = Keymap::load("", &extra);
        assert_eq!(k.key_for("adapter \"A\" x").as_deref(), Some("cmd+alt+g"));
        assert_eq!(k.key_for("crop").as_deref(), Some("c"));
    }
}
