# grace9000 adapters

grace9000 ships bare. An **adapter** is an add-on you choose to have, like
foobar2000's components: a folder with an `adapter.toml` and, usually, a
program in any language. grace9000 starts the program when it's needed and
talks to it with JSON messages, one per line, over stdin and stdout. An
adapter can crash without taking grace9000 down.

The host is the same one splice9000 and wave9000 use, so an adapter written
for one reads naturally for the others. What an adapter sees and changes is
different: here it's one photo and the edit made to it.

An adapter can add:

- **commands**, in any menu and bindable to keys;
- **panels** you place in the layout like any other;
- **export formats**, which need no program at all: an adapter can be only
  an `adapter.toml` of presets;
- **file types it opens**, with a program that turns each file into a
  picture grace9000 reads (that's how camera raw works);
- **steps of its own kind** in the edit (see `edit.set`): grace9000 keeps
  them and skips them, so an adapter can do the work with `image.render`.

Two come built in: **Camera Raw** and **JPEG XL** (their source is in
`adapters/` in this repo). Built-in adapters can be removed in the Adapters
panel and added back. There's no catalog yet.

## Installing

**Drop an adapter's zip or folder on grace9000's window** (or **Adapters →
Install Adapter…**). grace9000 shows what it adds and what it may do, and
installs it only when you click **Install**: Return does nothing and Escape
declines, so a stray key press can't install anything. grace9000 itself
never goes online.

Installed adapters live in `adapters/` in the settings folder. The first
time an adapter you copied in by hand runs, grace9000 asks before starting
it. Installing through the prompt counts as allowing it. Answers are kept
in `approvals.json` until the adapter asks for more.

**Remove** one in the Adapters panel: it goes to the Trash.

## `adapter.toml`

```toml
name = "Web Sizes"              # shown in menus and prompts; the adapter's identity
version = "0.1.0"
author = "you"
license = "MIT"
description = "One sentence about what it does."
homepage = "https://github.com/you/web-sizes"
requires = "0.0.1"              # optional: the oldest grace9000 it works with

run = ["python3", "main.py"]    # program + arguments, run from this folder
permissions = ["edit.read", "edit.change"]
start = "command"               # "command" (default) or "launch"
events = ["changed"]            # events to receive while running

[[commands]]
id = "sizes"                    # sent to you when chosen
label = "Export Web Sizes"
keys = "cmd+shift+w"            # optional default shortcut; keys.toml can change it
menu = "file"                   # optional: "file", "edit", "photo", "steps" or "view"; else Adapters

[[panels]]
id = "notes"                    # filled with panel.set
title = "Notes"

[[exports]]                     # no program needed for these
id = "avif"
label = "AVIF (2048 px)"
extension = "avif"
args = ["-c:v", "libaom-av1", "-still-picture", "1", "-crf", "28"]  # ffmpeg encoder arguments; left out: the extension's own
filter = ""                     # an ffmpeg filter on the finished picture, such as "hue=s=0"
long_edge = 2048                # shrink so the long side is this; 0: settings.toml's
bits = 0                        # 8 or 16 into ffmpeg; 0: the photo's own

[[opens]]                       # file types this adapter opens
label = "Camera raw"            # shown in the Info panel
extensions = ["cr2", "nef"]     # any case, without the dot
run = ["./raw-decode", "{input}", "{output}"]
```

### Opening files

When a file with one of an `[[opens]]` extension is opened, grace9000 runs
its `run` (from the adapter's folder, like the main program) with `{input}`
replaced by the file and `{output}` by where to put the picture, and waits
for it to finish. The program writes the picture as PAM (`P7`, `DEPTH` 1 to
4, `MAXVAL` 255 or up to 65535, any bit depth in between scaled to 16) or
PNG, turned the way it should be shown, in sRGB, then exits with 0. Anything
else, and what it printed to stderr is shown as the reason. An adapter with
only `[[opens]]` needs no main program; its `run` there is the only thing of
it that ever runs. Like any program-backed adapter, one you copied in by
hand is allowed first (Allow… in the Adapters panel).

`[[opens]]` wins over ffmpeg for its extensions, so an adapter can open a
format better than ffmpeg does (camera raw, where ffmpeg shows the sensor's
mosaic).

- `run` is started in the adapter's folder. A program given as a path
  (`./web-sizes`) is found there, and on Windows `.exe` is added if that's
  what's there, so one `adapter.toml` works everywhere.
- grace9000 sets `GRACE9000_CONFIG_DIR`, `GRACE9000_FFMPEG` and
  `GRACE9000_FFPROBE`, so adapters can keep their own files and use the same
  ffmpeg without bringing one.
- Whatever the program prints to stderr shows in the Log panel.
- Commands can be bound in keys.toml: `"cmd+k" = 'adapter "Web Sizes" sizes'`.
  Export formats too: `"cmd+shift+e" = 'export "Web Sizes" avif'`.
- Panels go in layout.toml as `panel = "adapter:<name>/<panel id>"`, or
  with the panel buttons in the Adapters panel.
- Unknown keys are ignored, so newer adapter.toml files work in older
  grace9000s as far as they can. When that isn't far enough, set `requires`.

### Permissions

| Permission | Allows |
|---|---|
| `edit.read` | `edit.get`, `image.render`, and events |
| `edit.change` | `edit.set`, `edit.import` |
| `network` | Nothing through grace9000; it tells people the adapter goes online |

An adapter is a normal program running as you, so permissions control what
it can do **through grace9000**, and `network` is an honest declaration, not
a sandbox. Only install adapters you trust.

## Messages

Every message is one JSON object on one line, in UTF-8. A **notification**
has `method` and `params` and gets no reply. A **request** also has an `id`;
the reply has the same `id` and either `result` or `error` (a string). Only
adapters send requests.

### grace9000 → adapter

| Method | Params | When |
|---|---|---|
| `initialize` | `grace9000` (version), `protocol` (`1`), `permissions`, `ffmpeg`, `ffprobe` (paths, or null) | Right after starting |
| `command` | `command`: the command's `id` | One of its commands was chosen |
| `event` | `kind` plus fields below | Something happened, if listed in `events` (needs `edit.read`) |
| `shutdown` | | grace9000 is quitting or the adapter is being removed; exit soon |

| Event `kind` | Fields |
|---|---|
| `changed` | `steps` (how many), `photo`: at most once a frame while editing |
| `opened` | `path`: a photo was opened |
| `exported` | `path` |
| `saved` | `path`: an edit was saved as a `.grace` file |

### Adapter → grace9000

| Method | Params | Result | Permission |
|---|---|---|---|
| `edit.get` | | The photo and edit (below) | `edit.read` |
| `edit.set` | `edit` (as in `edit.get`) | `null`. Replaces the edit as one undo step; values out of range are brought into it | `edit.change` |
| `edit.import` | `paths` | `null`. Opens the first one (one photo at a time) | `edit.change` |
| `image.render` | `path`, optional `long_edge`, `bits` (8 or 16) | `{ path, width, height }` once a PNG of the photo with its edit is written at `path` | `edit.read` |
| `panel.set` | `id`, `lines` | `null`. Each line is a string, or `{ "text", "command" }` for a button running one of your commands | none |
| `notify` | `text` | `null`. A short message at the bottom of the window | none |
| `log` | `message` | Written to the Log panel | none |
| `ui.confirm` | `title`, `message` | `true` or `false` | none |
| `ui.prompt` | `title`, `message`, `default` | The text typed, or `null` | none |
| `ui.save_dialog` | `title`, `file_name`, `extension` | The chosen path, or `null` | none |
| `ui.open_dialog` | `title`, `extensions` | The chosen path, or `null` | none |

### The photo and its edit

```json
{
  "photo": "/Users/me/dusk.jpg",
  "size": { "width": 6000, "height": 4000, "bits": 8, "result": [5200, 3466] },
  "file": "/Users/me/dusk.grace",
  "selected": 1,
  "edit": {
    "steps": [
      { "kind": "exposure", "on": true, "values": { "ev": 0.35 } },
      { "kind": "curve", "on": true, "values": {}, "points": [[0, 0], [0.25, 0.2], [0.75, 0.82], [1, 1]] }
    ],
    "turns": 0,
    "flip_h": false,
    "flip_v": false,
    "straighten": 1.5,
    "crop": { "x": 0.04, "y": 0.06, "w": 0.92, "h": 0.86 }
  }
}
```

- Steps run top to bottom. A value left out is the kind's default.
- The kinds and their values, with ranges and defaults, are `KINDS` in
  `src/develop.rs`: `exposure` (`ev`), `white balance` (`temperature`,
  `tint`), `contrast` (`amount`), `tone` (`highlights`, `shadows`), `levels`
  (`black`, `white`, `gamma`), `color` (`saturation`, `vibrance`), `curve`
  (`points`), `black and white` (`red`, `green`, `blue`), `vignette`
  (`amount`, `size`, `feather`).
- After the steps, the photo is turned `turns` quarter turns clockwise,
  mirrored, straightened by `straighten` degrees clockwise (zoomed in just
  enough that no corner comes up empty), then cropped. `crop` is in
  fractions of the turned photo; `null` keeps all of it.
- `result` is the size of what's kept, in the photo's own pixels.

## Versions

`initialize` says which protocol version grace9000 speaks. New versions only
add things, so an adapter written for an older version keeps working.

- **1** (grace9000 0.0.1): commands, panels, export formats, the edit,
  `image.render`, events and prompts.
- **2** (grace9000 0.0.2): `[[opens]]`. An adapter using it should set
  `requires = "0.0.2"`.
