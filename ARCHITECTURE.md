# How grace9000 is built

For people changing the code. The ideals behind these choices are in
[docs/philosophy.md](docs/philosophy.md).

## The parts

| File | What it does |
|---|---|
| `main.rs` | `App`: all state, the commands, opening and saving, config, the session |
| `project.rs` | the document: the photo, its steps, the turn and the crop (`.grace`) |
| `image.rs` | finding ffmpeg, reading photos (PNG and PAM by itself, others through adapters), the pyramid, sRGB tables |
| `develop.rs` | `KINDS`, the one list of step kinds, and the renderer |
| `preview.rs` | the preview thread, and the histogram |
| `export.rs` | rendering the photo to a file, strip by strip |
| `panels.rs` | what each panel draws, the crop and the curve editor |
| `layout.rs` | the panel tree, splitters, layout edit mode, `layout.toml` (from splice9000) |
| `menu.rs` | the menus: macOS's own menu bar, or drawn in the window |
| `theme.rs` | the palette every surface draws with |
| `adapters.rs` | adapters: finding, installing, approving, running, removing (from splice9000) |
| `hosting.rs` | what the app does for adapters: their requests, prompts, menus, keys |
| `keys.rs` | `COMMANDS`, the one list of commands, and `keys.toml` |
| `settings.rs` | `settings.toml` |
| core9000 | the window (drawn by the CPU), the adapter host, the family protocol, the sisters, the config watcher, files macOS hands over, the log: shared by the 9000 series ([solacecasino/core9000](https://github.com/solacecasino/core9000)) |

## One owner of state

Everything lives in `App`, on the interface thread. Three kinds of
background work exist, and none of them touches the edit:

- **Reading a photo**: a thread decodes it and sends it down a channel;
  `take_loaded` shows it, with the edit from a `.grace` file or the
  session if there is one. Only the last photo asked for counts.
- **The preview**: one thread, fed through a one-slot mailbox. A new
  request replaces any that waited, so dragging a slider asks for many
  previews but only the newest is made.
- **Export**, and `image.render` for adapters: a thread renders the photo
  into a file.

Each calls `request_repaint` when there's something new. Nothing redraws on
a timer, so an idle editor uses no CPU.

## Photos

A photo is read once, by ffmpeg, as PAM: a header with the size, then
RGBA, 8 bits a channel, or 16 when the file holds more than 8. ffmpeg turns
it the way its EXIF says first. PNG is read by the `png` crate instead, so
it works without ffmpeg.

An adapter's `[[opens]]` wins over both for its extensions (`Opener`): its
program runs on the reading thread and leaves a PAM or PNG in the temporary
folder, which is read the same way and removed. That's how camera raw
(`adapters/camera-raw`, rawler) and JPEG XL (`adapters/jpeg-xl`, jxl-oxide)
come in, so the core still links no codecs. Built-in adapters ship beside
the program (`scripts/stage-adapters.sh`).

A HEIC or AVIF stored as a grid of tiles comes back from an ffmpeg older
than 7.1 as its first tile alone; ffprobe's list of same-sized tiles gives
that away, and grace9000 says so instead of showing a piece of the photo. The pixels are kept as they were stored,
gamma-encoded, beside a pyramid of halvings down to 256 pixels (a third
more memory).

## The renderer

`develop::render` makes any window of the result at any scale:

- Cropping, straightening, mirroring and turning together are one affine
  map from an output pixel to the photo's (`Map`), worked out from three
  points. Each pixel's place is its neighbour's plus a constant.
- Pixels are read from the pyramid level with about one of its pixels per
  output pixel, the four nearest blended in linear light (through a table
  per bit depth). Past 2× the photo's size they show as squares.
- Steps are compiled once (`Op`) and run per pixel in order. Exposure,
  white balance, black & white and the vignette work in linear light; the
  others in sRGB, as the eye sees it. The pixel changes space only where
  two steps in a row disagree, through 4097-value tables.
- A curve is a monotone cubic through its points (Fritsch–Carlson, so it
  never overshoots between them), kept as 1025 values.
- Rows are split across every core with `std::thread::scope`.

The preview asks for exactly the part of the result its panel shows, at the
screen's own pixel size, so the window copies it one to one. While a new
preview is being made, the last one is stretched into place, so zooming and
cropping never wait.

Export renders the same way at full size (or shrunk to `long_edge`), 128
rows at a time, into the `png` crate's stream writer or ffmpeg's input, so
memory stays small however large the photo is.

## Straightening

Straightening zooms in by `cos θ + (long/short) · sin θ`, the least that
keeps every corner of the frame inside the turned photo, so no edge comes up
empty and the crop always means the same part of the photo.

## Commands

Every action is a named command in `keys::COMMANDS`. Keys, menus, buttons
and the Help panel all run them through `App::run`. Adapters' commands are
named `adapter "Name" id` and their export formats `export "Name" id`.

## Testing

`main.rs`'s tests run the real `App` through egui with key and pointer
events, and draw each frame with the shell's own rasterizer. With
`GRACE9000_SHOTS=<folder>` they save a PNG of each step, so a change to the
interface can be looked at without a window.

## Compatibility

Settings, keys, layouts and `.grace` files are read with every field
optional and unknown fields ignored, and `.grace` files carry a `grace9000`
format number. Steps of kinds this version doesn't know are kept and
skipped, not dropped. Keep it that way: old files must keep opening, and
command and step names, once shipped, never change meaning.
