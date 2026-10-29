# grace9000's ideals and how it's built

The same ideals as Folio, Leaf, splice9000 and wave9000, its siblings. For
the full how and why, see [ARCHITECTURE.md](../ARCHITECTURE.md).

## Core ideals

1. **Start bare.** A fresh install opens to an empty photo panel and
   nothing else: no presets for sale, no start screen, no account, no
   library to import into. Every extra feature arrives only when the user
   installs it.
2. **Endlessly customizable, through files.** There's no settings window.
   Behaviour, shortcuts and looks live in plain text files
   (`settings.toml`, `keys.toml`, `layout.toml`) in one folder.
   - The app reloads them as soon as they're saved, with no restart.
   - Copying the folder moves your whole setup.
   - Every file ships with commented examples.
3. **Absolute backward compatibility.** Nothing that worked before may stop
   working: edits, configs, key bindings, the adapter protocol, step names.
   Old names keep working alongside new ones. Steps of a kind a version
   doesn't know are kept, not dropped.
4. **The user owns their data.** The photo is never written to. An edit is
   a small readable JSON file beside it. There's no cloud, telemetry or
   account, and the app itself never goes online.
5. **Lean and honest about cost.** Memory, download size, idle CPU and
   preview time are measured, not guessed, and every release reports real
   numbers, including what didn't work. Idle means idle: nothing redraws on
   a timer.
6. **Never crash on the user's account.** Bad config falls back to
   defaults and shows the problem on screen. ffmpeg errors go to the Log.
   Crashes leave a report for next time, and the edit is recovered.
7. **The interface never waits for pixels.** Reading, previewing and
   exporting happen on their own threads; a slider asks for a preview and
   moves on.
8. **A sister, not a copy.** grace9000, splice9000 and wave9000 share a
   look, a layout system, key conventions, the adapter host and their file
   habits. Each stays its own app.

## Architecture

- **One small core.** Steps, crop, layout and export in one binary, plus
  Folio's CPU-drawn window. No codecs are linked in: photos are read and
  written by the system's ffmpeg, run as a separate program. PNG is read
  and written without it.
- **Read once, then only look.** A photo is decoded once and kept as it was
  stored, with a pyramid of halvings, so any preview reads about one pixel
  for each it shows.
- **The maths is cheap.** Crop, straighten, mirror and turn are one affine
  map; steps are per-pixel formulas run in one pass, with tables in place
  of powers; rows are split across every core. The preview is made at the
  exact size it's shown, so the screen copies it one to one.
- **Everything is a panel, and panels go anywhere** (from splice9000).
- **Adapters for everything else** (from splice9000).
- **Everything is a named command.** One table drives keys, menus, buttons
  and the Help panel; one more (`KINDS`) drives every step.
- **The CPU draws only what changed** (from Folio).
