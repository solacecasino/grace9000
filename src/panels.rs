//! What each panel shows. Panels only read the app and queue commands or
//! make small edits; any panel can be placed anywhere, or twice.

use egui::{Align2, Color32, CursorIcon, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

use crate::develop::{self, KINDS};
use crate::layout::Part;
use crate::project::{Crop, Step};
use crate::theme::Palette;
use crate::{App, keys, log};

/// What dragging on the Photo panel is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    None,
    /// Moving the view; where it was and where the pointer was.
    Pan { center: [f32; 2], from: Pos2 },
    /// Changing the crop by its edges (`left`... all false: moving it).
    Crop { left: bool, right: bool, top: bool, bottom: bool, start: Crop, from: Pos2 },
}

/// A step dragged in the Steps panel.
struct StepDrag(usize);

pub fn draw(app: &mut App, kind: &str, part: Part, ui: &mut Ui) {
    if part == Part::Header {
        header(app, kind, ui);
        return;
    }
    // The photo runs edge to edge; the rest get a margin.
    let edge = matches!(kind, "photo" | "histogram");
    let rect = if edge { ui.max_rect() } else { ui.max_rect().shrink2(vec2(12.0, 8.0)) };
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
    let ui = &mut ui;
    match kind {
        "photo" => photo(app, ui),
        "steps" => steps(app, ui),
        "adjust" => adjust(app, ui),
        "histogram" => histogram(app, ui),
        "info" => info(app, ui),
        "export" => export(app, ui),
        "log" => log_panel(ui),
        "help" => help(app, ui),
        "adapters" => adapters_panel(app, ui),
        k if k.starts_with("adapter:") => adapter_panel(app, k, ui),
        _ => {}
    }
}

fn small(ui: &mut Ui, text: &str, tip: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(11.0)).frame(false).min_size(vec2(18.0, 18.0))).on_hover_text(tip)
}

/// Small controls at the right end of a panel's header.
fn header(app: &mut App, kind: &str, ui: &mut Ui) {
    match kind {
        "photo" => {
            for (text, cmd, tip) in [("100%", "zoom 100", "The photo's own pixels (cmd+1)"), ("Fit", "zoom fit", "Fit the panel (cmd+0)")] {
                if small(ui, text, tip).clicked() {
                    app.queue.push(cmd.into());
                }
            }
            let on = |ui: &mut Ui, lit: bool, text: &str, tip: &str| {
                ui.add(egui::Button::selectable(lit, RichText::new(text).size(11.0)).frame_when_inactive(false)).on_hover_text(tip)
            };
            if on(ui, app.cropping, "Crop", "Crop and straighten (c)").clicked() {
                app.queue.push("crop".into());
            }
            if on(ui, app.compare, "Before", "The photo without its steps (\\)").clicked() {
                app.queue.push("compare".into());
            }
        }
        "steps" => {
            ui.menu_button(RichText::new("+ Add").size(11.0), |ui| {
                for k in KINDS {
                    if ui.button(k.title).on_hover_text(k.what).clicked() {
                        app.queue.push(format!("add {}", k.id));
                        ui.close();
                    }
                }
            });
        }
        "adapters" if small(ui, "+ Install", "Install an adapter from its zip (or drop it on the window)").clicked() => {
            app.queue.push("install adapter".into());
        }
        _ => {}
    }
}

/// The photo with its edit, at exactly the size it's shown: zoom with
/// cmd+scroll or a pinch, move around by scrolling or dragging. While
/// cropping it shows the whole photo with the crop on top.
fn photo(app: &mut App, ui: &mut Ui) {
    let rect = ui.max_rect();
    let pal = app.palette;
    ui.painter().rect_filled(rect, 0.0, pal.well);
    let Some(photo) = app.photo.clone() else {
        let text = if app.reading.is_some() {
            "Reading…"
        } else if app.tools.is_none() {
            "Drop a PNG anywhere (install ffmpeg for other photos)"
        } else {
            "Drop a photo anywhere to start"
        };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(15.0), pal.weak);
        return;
    };
    let ppp = ui.ctx().pixels_per_point();
    let whole = app.cropping;
    let (fw1, fh1) = develop::full_size(photo.width, photo.height, &app.project.edit, 1.0, whole);
    let margin = if whole { 36.0 } else { 16.0 } * ppp;
    let p = rect.size() * ppp;
    let o = rect.min.to_vec2() * ppp;
    let fit = ((p.x - 2.0 * margin).max(16.0) / fw1).min((p.y - 2.0 * margin).max(16.0) / fh1);
    let mut scale = app.view.zoom.unwrap_or(fit);
    if app.view.zoom.is_none() {
        app.view.center = [0.5, 0.5];
    }
    let clamp = |c: [f32; 2], f: egui::Vec2| -> [f32; 2] {
        let one = |c: f32, f: f32, p: f32| if f <= p { 0.5 } else { c.clamp(p / 2.0 / f, 1.0 - p / 2.0 / f) };
        [one(c[0], f.x, p.x), one(c[1], f.y, p.y)]
    };
    let top_left = |c: [f32; 2], f: egui::Vec2| (o + p / 2.0 - vec2(c[0] * f.x, c[1] * f.y)).round();

    // Looking around (not while cropping: then the whole photo shows).
    let resp = ui.interact(rect, ui.scope_id().with("photo"), Sense::click_and_drag());
    if !whole {
        let f = vec2(fw1, fh1) * scale;
        let tl = top_left(app.view.center, f);
        let zoom = if resp.contains_pointer() { ui.input(|i| i.zoom_delta()) } else { 1.0 };
        let pointer = ui.input(|i| i.pointer.hover_pos()).map(|q| q.to_vec2() * ppp);
        let zoom_at = |app: &mut App, to: f32, at: egui::Vec2| {
            let to = to.clamp(0.01, 32.0);
            let n = vec2((at.x - tl.x) / f.x, (at.y - tl.y) / f.y);
            let f2 = vec2(fw1, fh1) * to;
            let tl2 = at - vec2(n.x * f2.x, n.y * f2.y);
            let c = o + p / 2.0 - tl2;
            app.view.zoom = Some(to);
            app.view.center = [c.x / f2.x, c.y / f2.y];
        };
        if zoom != 1.0
            && let Some(at) = pointer
        {
            zoom_at(app, scale * zoom, at);
        } else if resp.contains_pointer() {
            let d = ui.input(|i| i.smooth_scroll_delta) * ppp;
            if d != egui::Vec2::ZERO && app.view.zoom.is_some() {
                app.view.center = [app.view.center[0] - d.x / f.x, app.view.center[1] - d.y / f.y];
            }
        }
        if resp.double_clicked()
            && let Some(at) = pointer
        {
            if app.view.zoom.is_some() {
                app.view.zoom = None;
            } else {
                zoom_at(app, 1.0, at);
            }
        }
        if resp.drag_started()
            && let Some(from) = ui.input(|i| i.pointer.press_origin())
        {
            app.view.drag = Drag::Pan { center: app.view.center, from };
        }
        if let (Drag::Pan { center, from }, Some(now)) = (app.view.drag, resp.interact_pointer_pos())
            && resp.dragged()
            && app.view.zoom.is_some()
        {
            let d = (now - from) * ppp;
            app.view.center = [center[0] - d.x / f.x, center[1] - d.y / f.y];
        }
        if app.view.zoom.is_some() && (resp.hovered() || resp.dragged()) {
            ui.ctx().set_cursor_icon(if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
        scale = app.view.zoom.unwrap_or(fit);
    }
    if resp.drag_stopped() && matches!(app.view.drag, Drag::Pan { .. }) {
        app.view.drag = Drag::None;
    }
    app.view.scale = scale;
    let f = vec2(fw1, fh1) * scale;
    app.view.center = clamp(app.view.center, f);
    let tl = top_left(app.view.center, f);

    // Ask for exactly the part that shows.
    let (fx, fy) = (f.x.round(), f.y.round());
    let (x0, y0) = ((o.x - tl.x).max(0.0), (o.y - tl.y).max(0.0));
    let (x1, y1) = ((o.x + p.x - tl.x).min(fx), (o.y + p.y - tl.y).min(fy));
    if x1 > x0 && y1 > y0 {
        let view = develop::View { scale, x: x0 as i32, y: y0 as i32, w: (x1 - x0) as usize, h: (y1 - y0) as usize, whole, plain: app.compare };
        app.ask_preview(view);
    }

    // Behind the photo, then the photo (stretched from the last preview
    // until the new one comes).
    let img = Rect::from_min_size((tl / ppp).to_pos2(), f / ppp);
    app.view.area = Some(img);
    let painter = ui.painter().with_clip_rect(rect);
    backdrop(&painter, img.intersect(rect), &app.settings.look.backdrop, &pal);
    if let (Some(tex), Some(sv)) = (&app.texture, app.view.shown)
        && sv.whole == whole
    {
        let k = scale / sv.scale;
        let min = tl + vec2(sv.x as f32, sv.y as f32) * k;
        let size = vec2(sv.w as f32, sv.h as f32) * k;
        painter.image(tex.id(), Rect::from_min_size((min / ppp).to_pos2(), size / ppp), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    }
    if whole {
        crop_overlay(app, ui, rect, img, &resp, fw1 / fh1);
    }

    // What's showing, in the corners.
    let label = |pos: Pos2, align: Align2, text: String, fill: Color32, color: Color32| {
        let galley = painter.layout_no_wrap(text, FontId::proportional(10.5), color);
        let r = align.anchor_size(pos, galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(r, 4.0, fill);
        painter.galley(r.min + vec2(6.0, 3.0), galley, color);
    };
    let fill = Color32::from_black_alpha(150);
    let zoom = format!("{}{:.0}%", if app.view.zoom.is_none() && !whole { "Fit · " } else { "" }, scale * 100.0);
    label(rect.right_bottom() + vec2(-8.0, -8.0), Align2::RIGHT_BOTTOM, zoom, fill, Color32::from_gray(220));
    if app.compare {
        label(rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, "BEFORE".into(), pal.accent, pal.on_accent);
    }
    if whole {
        let hint = "CROP  ·  drag the edges or the middle  ·  [ ] straighten  ·  Return when done";
        label(pos2(rect.center().x, rect.bottom() - 8.0), Align2::CENTER_BOTTOM, hint.into(), fill, Color32::from_gray(220));
    }
}

/// The well, a color, or a checkerboard behind the photo.
fn backdrop(p: &egui::Painter, r: Rect, how: &str, pal: &Palette) {
    if how.trim() == "checker" {
        p.rect_filled(r, 0.0, Color32::from_gray(200));
        let s = 8.0;
        let (nx, ny) = ((r.width() / s).ceil() as i32, (r.height() / s).ceil() as i32);
        if nx * ny > 40_000 {
            return;
        }
        for j in 0..ny {
            for i in (j % 2..nx).step_by(2) {
                let cell = Rect::from_min_size(r.min + vec2(i as f32 * s, j as f32 * s), vec2(s, s)).intersect(r);
                p.rect_filled(cell, 0.0, Color32::from_gray(160));
            }
        }
    } else if let Some(c) = crate::settings::hex(how) {
        p.rect_filled(r, 0.0, c);
    } else {
        p.rect_filled(r, 0.0, pal.well);
    }
}

/// The crop on the whole photo: dimmed outside, thirds inside, and edges
/// and corners to drag.
fn crop_overlay(app: &mut App, ui: &Ui, rect: Rect, img: Rect, resp: &egui::Response, photo_aspect: f32) {
    let pal = app.palette;
    let c = app.project.edit.crop();
    let at = |c: &Crop| Rect::from_min_size(img.min + vec2(c.x * img.width(), c.y * img.height()), vec2(c.w * img.width(), c.h * img.height()));
    let near = 10.0;
    let hit = |c: &Crop, q: Pos2| -> Option<(bool, bool, bool, bool)> {
        let r = at(c);
        if !r.expand(near).contains(q) {
            return None;
        }
        let (l, rt) = ((q.x - r.left()).abs() < near, (q.x - r.right()).abs() < near);
        let (t, b) = ((q.y - r.top()).abs() < near, (q.y - r.bottom()).abs() < near);
        Some((l, rt && !l, t, b && !t))
    };
    // Starting and following a drag.
    // Where the press began, not where the drag was noticed: a quick
    // drag is already past the corner by then.
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin())
        && let Some((left, right, top, bottom)) = hit(&c, q)
    {
        app.snapshot();
        app.view.drag = Drag::Crop { left, right, top, bottom, start: c, from: q };
    }
    if let (Drag::Crop { left, right, top, bottom, start, from }, Some(q)) = (app.view.drag, resp.interact_pointer_pos())
        && resp.dragged()
    {
        let d = vec2((q.x - from.x) / img.width(), (q.y - from.y) / img.height());
        let ratio = app.crop_aspect.map(|a| a / photo_aspect);
        let next = drag_crop(start, d, (left, right, top, bottom), ratio);
        app.project.edit.crop = Some(next).filter(|c| !c.is_all());
        app.edited();
    }
    if resp.drag_stopped() {
        app.view.drag = Drag::None;
    }
    // The cursor says what a drag would do.
    if let Some(q) = ui.input(|i| i.pointer.hover_pos()).filter(|q| rect.contains(*q)) {
        let icon = match hit(&c, q) {
            Some((true, _, true, _) | (_, true, _, true)) => CursorIcon::ResizeNwSe,
            Some((true, _, _, true) | (_, true, true, _)) => CursorIcon::ResizeNeSw,
            Some((true, _, _, _) | (_, true, _, _)) => CursorIcon::ResizeHorizontal,
            Some((_, _, true, _) | (_, _, _, true)) => CursorIcon::ResizeVertical,
            Some(_) => CursorIcon::Move,
            None => CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }

    let c = app.project.edit.crop();
    let r = at(&c);
    let p = ui.painter().with_clip_rect(rect);
    let dim = Color32::from_black_alpha(150);
    for part in [
        Rect::from_min_max(img.min, pos2(img.right(), r.top())),
        Rect::from_min_max(pos2(img.left(), r.bottom()), img.max),
        Rect::from_min_max(pos2(img.left(), r.top()), pos2(r.left(), r.bottom())),
        Rect::from_min_max(pos2(r.right(), r.top()), pos2(img.right(), r.bottom())),
    ] {
        p.rect_filled(part, 0.0, dim);
    }
    let thin = Stroke::new(1.0, Color32::from_white_alpha(70));
    for k in [1.0, 2.0] {
        let x = r.left() + r.width() * k / 3.0;
        let y = r.top() + r.height() * k / 3.0;
        p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], thin);
        p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], thin);
    }
    p.rect_stroke(r, 0.0, Stroke::new(1.5, pal.accent), StrokeKind::Middle);
    for q in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
        p.rect_filled(Rect::from_center_size(q, vec2(9.0, 9.0)), 1.0, pal.accent);
    }
    for q in [r.center_top(), r.center_bottom(), r.left_center(), r.right_center()] {
        p.rect_filled(Rect::from_center_size(q, vec2(7.0, 7.0)), 1.0, pal.accent);
    }
}

/// A crop dragged by `d` (in fractions of the photo) from `start`, by the
/// edges named, or moved when none are; held to `ratio` (width / height in
/// fractions) when given.
pub fn drag_crop(start: Crop, d: egui::Vec2, edges: (bool, bool, bool, bool), ratio: Option<f32>) -> Crop {
    let (left, right, top, bottom) = edges;
    let min = 0.01;
    if !(left || right || top || bottom) {
        return Crop { x: (start.x + d.x).clamp(0.0, 1.0 - start.w), y: (start.y + d.y).clamp(0.0, 1.0 - start.h), ..start };
    }
    let (mut x0, mut y0, mut x1, mut y1) = (start.x, start.y, start.x + start.w, start.y + start.h);
    if left {
        x0 = (x0 + d.x).clamp(0.0, x1 - min);
    }
    if right {
        x1 = (x1 + d.x).clamp(x0 + min, 1.0);
    }
    if top {
        y0 = (y0 + d.y).clamp(0.0, y1 - min);
    }
    if bottom {
        y1 = (y1 + d.y).clamp(y0 + min, 1.0);
    }
    if let Some(r) = ratio.filter(|r| *r > 0.0) {
        let (w, h) = (x1 - x0, y1 - y0);
        let horizontal = left || right;
        let vertical = top || bottom;
        if horizontal {
            // Width leads; height follows, from the edge held still.
            let mut h2 = w / r;
            let (cy, room) = if top {
                (y1, y1)
            } else if bottom {
                (y0, 1.0 - y0)
            } else {
                ((start.y + start.h / 2.0), (start.y + start.h / 2.0).min(1.0 - start.y - start.h / 2.0) * 2.0)
            };
            if h2 > room {
                h2 = room;
                let w2 = h2 * r;
                if left {
                    x0 = x1 - w2;
                } else {
                    x1 = x0 + w2;
                }
            }
            if top {
                y0 = cy - h2;
            } else if bottom {
                y1 = cy + h2;
            } else {
                y0 = cy - h2 / 2.0;
                y1 = cy + h2 / 2.0;
            }
        } else if vertical {
            let mut w2 = h * r;
            let cx = start.x + start.w / 2.0;
            let room = cx.min(1.0 - cx) * 2.0;
            if w2 > room {
                w2 = room;
                let h2 = w2 / r;
                if top {
                    y0 = y1 - h2;
                } else {
                    y1 = y0 + h2;
                }
            }
            x0 = cx - w2 / 2.0;
            x1 = cx + w2 / 2.0;
        }
    }
    Crop { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }.tidy()
}

/// The largest crop of shape `ratio` (in fractions) centered in `c`.
pub fn fit_crop(c: Crop, ratio: f32) -> Crop {
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let (w, h) = if c.w / c.h > ratio { (c.h * ratio, c.h) } else { (c.w, c.w / ratio) };
    Crop { x: cx - w / 2.0, y: cy - h / 2.0, w, h }.tidy()
}

/// Every step, top to bottom: click to select, the dot to turn it off or
/// on, drag to reorder. Then the crop and turn.
fn steps(app: &mut App, ui: &mut Ui) {
    let pal = app.palette;
    if app.photo.is_none() {
        ui.weak("Open a photo to add steps.");
        return;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if app.project.edit.steps.is_empty() {
            ui.add_space(4.0);
            ui.weak("No steps yet. + Add one above, or from the Steps menu.");
            ui.add_space(8.0);
        }
        let mut moved = None;
        for i in 0..app.project.edit.steps.len() {
            let s = app.project.edit.steps[i].clone();
            let title = develop::kind(&s.kind).map_or_else(|| s.kind.clone(), |k| k.title.to_owned());
            let detail = summary(&s);
            let selected = app.selected == Some(i) && !app.cropping;
            let (r, dot) = ui.dnd_drag_source(Id::unique(("step", i)), StepDrag(i), |ui| step_row(ui, &title, &detail, selected, s.on, &pal)).inner;
            if let Some(from) = r.dnd_release_payload::<StepDrag>() {
                moved = Some((from.0, i));
            }
            if r.dnd_hover_payload::<StepDrag>().is_some() {
                ui.painter().hline(r.rect.x_range(), r.rect.top(), Stroke::new(2.0, pal.accent));
            }
            if dot.clicked() {
                app.snapshot();
                app.project.edit.steps[i].on = !s.on;
            } else if r.clicked() {
                app.selected = Some(i);
                app.cropping = false;
            }
            r.context_menu(|ui| {
                for (label, cmd) in [("Turn Off or On", "step toggle"), ("Reset", "step reset"), ("Move Up", "step up"), ("Move Down", "step down"), ("Delete", "step delete")] {
                    if ui.button(label).clicked() {
                        app.selected = Some(i);
                        app.queue.push(cmd.into());
                        ui.close();
                    }
                }
            });
        }
        if let Some((from, to)) = moved {
            app.move_step(from, to);
        }
        ui.add_space(6.0);
        ui.separator();
        let e = &app.project.edit;
        let mut parts = Vec::new();
        if let (Some(c), Some(p)) = (e.crop, &app.photo) {
            let (w, h) = e.size(p.width, p.height);
            let _ = c;
            parts.push(format!("Crop {}×{}", w.round(), h.round()));
        }
        if e.turns > 0 {
            parts.push(format!("{}°", u32::from(e.turns) * 90));
        }
        if e.flip_h || e.flip_v {
            parts.push("mirrored".into());
        }
        if e.straighten != 0.0 {
            parts.push(format!("straightened {:+.1}°", e.straighten));
        }
        let detail = if parts.is_empty() { "The whole photo".to_owned() } else { parts.join(" · ") };
        let (r, _) = step_row(ui, "Crop & Turn", &detail, app.cropping, true, &pal);
        if r.clicked() {
            app.queue.push("crop".into());
        }
    });
}

/// A step's values that aren't their defaults, briefly.
fn summary(s: &Step) -> String {
    let Some(k) = develop::kind(&s.kind) else { return "Needs a newer grace9000; skipped".into() };
    if s.kind == "curve" {
        return format!("{} points", s.points.len());
    }
    let parts: Vec<String> = k.params.iter().filter(|p| (s.get(p.key) - p.default).abs() > 1e-4).map(|p| format!("{} {}", p.label, number(s.get(p.key), p))).collect();
    if parts.is_empty() { "No change yet".into() } else { parts.join(" · ") }
}

fn number(v: f32, p: &develop::Param) -> String {
    if p.max - p.min >= 50.0 { format!("{:+.0}{}", v, p.unit) } else { format!("{:.2}{}", v, p.unit) }
}

/// A row: a dot that shows whether it's on, the title, and what it does.
/// Returns the row and the dot.
fn step_row(ui: &mut Ui, title: &str, detail: &str, selected: bool, on: bool, pal: &Palette) -> (egui::Response, egui::Response) {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let v = ui.visuals();
    if selected {
        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill);
        ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 8.0), vec2(2.0, rect.height() - 16.0)), 1.0, v.selection.stroke.color);
    } else if r.hovered() {
        ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
    }
    let dot_rect = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(20.0, 20.0));
    let dot = ui.interact(dot_rect, Id::unique(("dot", title, rect.top() as i32)), Sense::click()).on_hover_text("Turn off or on (e)");
    if on {
        ui.painter().circle_filled(dot_rect.center(), 4.5, pal.accent);
    } else {
        ui.painter().circle_stroke(dot_rect.center(), 4.0, Stroke::new(1.2, pal.weak));
    }
    let p = ui.painter().with_clip_rect(rect.shrink(2.0).intersect(ui.clip_rect()));
    let text = if on { v.strong_text_color() } else { pal.weak };
    p.text(rect.left_top() + vec2(30.0, 5.0), Align2::LEFT_TOP, title, FontId::proportional(13.0), text);
    p.text(rect.left_top() + vec2(30.0, 22.0), Align2::LEFT_TOP, detail, FontId::proportional(11.0), v.weak_text_color());
    (r, dot)
}

/// The selected step's values, or the crop's while cropping.
fn adjust(app: &mut App, ui: &mut Ui) {
    let pal = app.palette;
    let Some(photo) = app.photo.clone() else {
        ui.weak("Open a photo first.");
        return;
    };
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(80.0);
        if app.cropping {
            crop_controls(app, ui, photo.width, photo.height);
            return;
        }
        let Some(i) = app.selected.filter(|&i| i < app.project.edit.steps.len()) else {
            ui.weak("Select a step, or add one:");
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for k in KINDS {
                    if ui.button(k.title).on_hover_text(k.what).clicked() {
                        app.queue.push(format!("add {}", k.id));
                    }
                }
            });
            ui.add_space(8.0);
            if ui.button("Crop & Straighten…").clicked() {
                app.queue.push("crop".into());
            }
            return;
        };
        let s = app.project.edit.steps[i].clone();
        let Some(kind) = develop::kind(&s.kind) else {
            ui.label(RichText::new(&s.kind).strong());
            ui.weak("This step comes from a newer grace9000 or an adapter. It's kept in the edit, but this version skips it.");
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(kind.title).strong().color(pal.text));
            if !s.on {
                ui.label(RichText::new("OFF").size(9.5).color(pal.weak));
            }
        });
        ui.label(RichText::new(kind.what).small().color(pal.weak));
        ui.add_space(6.0);
        for p in kind.params {
            let mut v = s.get(p.key);
            let wide = p.max - p.min >= 50.0;
            let mut slider = egui::Slider::new(&mut v, p.min..=p.max).text(p.label).suffix(p.unit).clamping(egui::SliderClamping::Always);
            slider = if wide { slider.step_by(1.0).fixed_decimals(0) } else { slider.fixed_decimals(2) };
            let r = ui.add(slider).on_hover_text("Double-click to reset");
            let mut changed = (v - s.get(p.key)).abs() > f32::EPSILON;
            if r.double_clicked() {
                v = p.default;
                changed = true;
            }
            edit(app, &r, changed, |a| {
                a.project.edit.steps[i].values.insert(p.key.to_owned(), v);
            });
        }
        if s.kind == "curve" {
            curve_editor(app, ui, i);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (label, cmd) in [(if s.on { "Turn Off" } else { "Turn On" }, "step toggle"), ("Reset", "step reset"), ("Delete", "step delete")] {
                if ui.button(label).clicked() {
                    app.queue.push(cmd.into());
                }
            }
        });
    });
}

fn crop_controls(app: &mut App, ui: &mut Ui, pw: usize, ph: usize) {
    let pal = app.palette;
    ui.label(RichText::new("Crop & Straighten").strong().color(pal.text));
    let (tw, th) = app.project.edit.turned(pw, ph);
    let (w, h) = app.project.edit.size(pw, ph);
    ui.label(RichText::new(format!("{}×{} of {tw}×{th}", w.round(), h.round())).small().color(pal.weak));
    ui.add_space(6.0);
    let original = tw as f32 / th as f32;
    let shapes: [(&str, Option<f32>); 8] =
        [("Any", None), ("Original", Some(original)), ("1:1", Some(1.0)), ("4:5", Some(0.8)), ("3:2", Some(1.5)), ("2:3", Some(2.0 / 3.0)), ("16:9", Some(16.0 / 9.0)), ("9:16", Some(9.0 / 16.0))];
    let current = shapes.iter().find(|s| s.1.zip(app.crop_aspect).is_some_and(|(a, b)| (a - b).abs() < 1e-3) || (s.1.is_none() && app.crop_aspect.is_none())).map_or("Any", |s| s.0);
    let mut picked = None;
    egui::ComboBox::from_label("Shape").selected_text(current).show_ui(ui, |ui| {
        for (name, a) in shapes {
            if ui.selectable_label(current == name, name).clicked() {
                picked = Some(a);
            }
        }
    });
    if let Some(a) = picked {
        app.crop_aspect = a;
        if let Some(a) = a {
            app.snapshot();
            let c = fit_crop(app.project.edit.crop(), a / original);
            app.project.edit.crop = Some(c).filter(|c| !c.is_all());
        }
    }
    let mut v = app.project.edit.straighten;
    let r = ui.add(egui::Slider::new(&mut v, -45.0..=45.0).text("Straighten").suffix("°").fixed_decimals(1)).on_hover_text("Double-click to reset · [ and ] nudge it");
    let mut changed = (v - app.project.edit.straighten).abs() > f32::EPSILON;
    if r.double_clicked() {
        v = 0.0;
        changed = true;
    }
    edit(app, &r, changed, |a| a.project.edit.straighten = v);
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (label, cmd) in [("Rotate Left", "rotate left"), ("Rotate Right", "rotate right"), ("Flip Horizontal", "flip horizontal"), ("Flip Vertical", "flip vertical")] {
            if ui.button(label).clicked() {
                app.queue.push(cmd.into());
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Reset").on_hover_text("The whole photo, not straightened").clicked() {
            app.queue.push("crop reset".into());
        }
        let done = egui::Button::new(RichText::new("Done").color(pal.on_accent)).fill(pal.accent);
        if ui.add(done).clicked() {
            app.queue.push("crop".into());
        }
    });
}

/// The curve: drag a point, click to add one, double-click one to remove
/// it. The photo's brightness is drawn behind.
fn curve_editor(app: &mut App, ui: &mut Ui, i: usize) {
    let pal = app.palette;
    let side = ui.available_width().min(260.0);
    let (rect, r) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, pal.well);
    if let Some(h) = &app.histogram {
        bars(&p, rect, &h[3], Color32::from_white_alpha(28));
    }
    let grid = Stroke::new(1.0, pal.line);
    for k in 1..4 {
        let t = k as f32 / 4.0;
        p.line_segment([pos2(rect.left() + t * side, rect.top()), pos2(rect.left() + t * side, rect.bottom())], grid);
        p.line_segment([pos2(rect.left(), rect.top() + t * side), pos2(rect.right(), rect.top() + t * side)], grid);
    }
    let to_screen = |q: [f32; 2]| pos2(rect.left() + q[0] * side, rect.bottom() - q[1] * side);
    let to_curve = |q: Pos2| [((q.x - rect.left()) / side).clamp(0.0, 1.0), ((rect.bottom() - q.y) / side).clamp(0.0, 1.0)];
    let points = app.project.edit.steps[i].points.clone();
    let table = develop::curve_table(&points);
    let line: Vec<Pos2> = (0..=64).map(|k| to_screen([k as f32 / 64.0, table[k * 16]])).collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, pal.text)));
    let nearest = |q: Pos2| points.iter().enumerate().map(|(k, pt)| (k, to_screen(*pt).distance(q))).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k);
    let id = ui.scope_id().with(("curve point", i));
    let held: Option<usize> = ui.data(|d| d.get_temp(id));
    if r.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin())
    {
        app.snapshot();
        let k = match nearest(q) {
            Some(k) => k,
            None => {
                let s = &mut app.project.edit.steps[i];
                s.points.push(to_curve(q));
                s.points.sort_by(|a, b| a[0].total_cmp(&b[0]));
                s.points.iter().position(|pt| *pt == to_curve(q)).unwrap_or(0)
            }
        };
        ui.data_mut(|d| d.insert_temp(id, k));
    }
    if r.dragged()
        && let (Some(k), Some(q)) = (held, r.interact_pointer_pos())
    {
        let s = &mut app.project.edit.steps[i];
        if k < s.points.len() {
            // Between its neighbours, so the order never changes.
            let lo = if k == 0 { 0.0 } else { s.points[k - 1][0] + 0.01 };
            let hi = if k + 1 == s.points.len() { 1.0 } else { s.points[k + 1][0] - 0.01 };
            let c = to_curve(q);
            s.points[k] = [c[0].clamp(lo, hi.max(lo)), c[1]];
            app.edited();
        }
    }
    if r.drag_stopped() {
        ui.data_mut(|d| d.remove::<usize>(id));
    }
    if r.clicked()
        && let Some(q) = r.interact_pointer_pos()
        && nearest(q).is_none()
    {
        app.snapshot();
        let s = &mut app.project.edit.steps[i];
        s.points.push(to_curve(q));
        s.points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }
    if r.double_clicked()
        && let Some(q) = r.interact_pointer_pos()
        && let Some(k) = nearest(q)
        && app.project.edit.steps[i].points.len() > 2
    {
        app.snapshot();
        app.project.edit.steps[i].points.remove(k);
    }
    for pt in &app.project.edit.steps[i].points {
        p.circle_filled(to_screen(*pt), 4.0, pal.accent);
    }
    r.on_hover_text("Drag a point · click to add one · double-click one to remove it");
}

/// Applies an edit from a field, with one undo step per drag or typed value.
fn edit(app: &mut App, r: &egui::Response, changed: bool, f: impl FnOnce(&mut App)) {
    if r.drag_started() || (changed && !r.dragged()) {
        app.snapshot();
    }
    if changed {
        f(app);
        app.edited();
    }
}

/// Counts as bars, scaled by their square root so small ones still show.
fn bars(p: &egui::Painter, rect: Rect, counts: &[u32; 256], color: Color32) {
    // The tallest, leaving out the two ends (often huge when clipped).
    let top = counts[1..255].iter().copied().max().unwrap_or(1).max(1) as f32;
    let w = rect.width() / 256.0;
    let mut mesh = egui::Mesh::default();
    for (k, &n) in counts.iter().enumerate() {
        let h = ((n as f32 / top).sqrt()).min(1.0) * rect.height();
        if h > 0.0 {
            mesh.add_colored_rect(Rect::from_min_max(pos2(rect.left() + k as f32 * w, rect.bottom() - h), pos2(rect.left() + (k + 1) as f32 * w, rect.bottom())), color);
        }
    }
    p.add(mesh);
}

fn histogram(app: &mut App, ui: &mut Ui) {
    let rect = ui.max_rect();
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, app.palette.well);
    let Some(h) = &app.histogram else {
        p.text(rect.center(), Align2::CENTER_CENTER, "No photo", FontId::proportional(12.0), app.palette.weak);
        return;
    };
    let area = rect.shrink2(vec2(10.0, 10.0));
    for (k, c) in [(0, Color32::from_rgba_unmultiplied(255, 70, 70, 70)), (1, Color32::from_rgba_unmultiplied(70, 220, 90, 70)), (2, Color32::from_rgba_unmultiplied(80, 130, 255, 80))] {
        bars(&p, area, &h[k], c);
    }
    bars(&p, area, &h[3], Color32::from_white_alpha(40));
    // Clipped ends, marked.
    let total: u32 = h[3].iter().sum::<u32>().max(1);
    let clipped = |n: u32| n as f32 / total as f32 > 0.005;
    if clipped(h[3][0]) {
        p.rect_filled(Rect::from_min_size(area.left_top(), vec2(3.0, area.height())), 0.0, Color32::from_rgb(80, 130, 255));
    }
    if clipped(h[3][255]) {
        p.rect_filled(Rect::from_min_size(area.right_top() - vec2(3.0, 0.0), vec2(3.0, area.height())), 0.0, Color32::from_rgb(255, 90, 90));
    }
}

fn info(app: &mut App, ui: &mut Ui) {
    let pal = app.palette;
    let Some(photo) = app.photo.clone() else {
        ui.weak(if app.reading.is_some() { "Reading…" } else { "No photo open." });
        return;
    };
    ui.label(RichText::new(photo.name()).strong().color(pal.text));
    // The folder by name; all of its path on hover.
    let folder = photo.path.parent();
    let name = folder.and_then(|f| f.file_name()).map_or(String::new(), |n| format!("in {}", n.to_string_lossy()));
    ui.label(RichText::new(name).small().color(pal.weak)).on_hover_text(folder.map(|f| f.display().to_string()).unwrap_or_default());
    ui.add_space(4.0);
    let (w, h) = app.project.edit.size(photo.width, photo.height);
    egui::Grid::new("info grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(RichText::new(k).color(pal.weak));
            ui.label(v);
            ui.end_row();
        };
        row("Size", format!("{}×{} · {:.1} MP", photo.width, photo.height, (photo.width * photo.height) as f64 / 1e6));
        row("Depth", format!("{} bits a channel", photo.bits()));
        row("Stored as", photo.format.clone());
        row("Result", format!("{}×{}", w.round(), h.round()));
        row("In memory", format!("{:.0} MB", photo.bytes() as f64 / 1e6));
        if let Some(v) = app.view.shown {
            row("Preview", format!("{}×{} in {:.0} ms", v.w, v.h, app.preview_ms));
        }
        if let Some(f) = &app.file {
            row("Edit", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    });
}

fn export(app: &mut App, ui: &mut Ui) {
    // The format: settings.toml's, or one an adapter adds.
    let presets: Vec<(String, String, String)> = app.adapters.exports().map(|(a, e)| (a.to_owned(), e.id.clone(), e.label.clone())).collect();
    let s = &app.settings.export;
    let fmt = s.format.to_lowercase();
    let own = match fmt.as_str() {
        "jpg" | "jpeg" => format!("JPEG · quality {}", s.jpeg_quality),
        "webp" => format!("WebP · quality {}", s.webp_quality),
        "png" => format!("PNG{}", if s.png_bits > 0 { format!(" {}-bit", s.png_bits) } else { String::new() }),
        other => other.to_uppercase(),
    };
    let chosen = app.export_choice.as_ref().and_then(|(a, id)| presets.iter().find(|p| &p.0 == a && &p.1 == id)).map(|p| p.2.clone());
    let mut choice = app.export_choice.clone();
    egui::ComboBox::from_id_salt("export format")
        .selected_text(chosen.unwrap_or_else(|| own.clone()))
        .width(ui.available_width().min(220.0))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut choice, None, own.as_str()).on_hover_text("The format in settings.toml");
            for (a, id, label) in &presets {
                ui.selectable_value(&mut choice, Some((a.clone(), id.clone())), label).on_hover_text(format!("From {a}"));
            }
        });
    app.export_choice = choice;
    if let Some(photo) = &app.photo {
        let (w, h, _) = crate::export::size(photo, &app.project.edit, app.settings.export.long_edge);
        ui.label(format!("{w}×{h}"));
    }
    if app.export_choice.is_none() && ui.link("Change in settings.toml").clicked() {
        app.queue.push("config".into());
    }
    ui.add_space(6.0);
    if let Some(job) = &app.export {
        let done = job.progress.lock().unwrap_or_else(|e| e.into_inner()).rows;
        let frac = (done as f32 / job.total.max(1) as f32).clamp(0.0, 1.0);
        let secs = job.started.elapsed().as_secs_f32();
        let eta = if frac > 0.05 { format!(" · {:.0} s left", (secs / frac - secs).max(0.0)) } else { String::new() };
        ui.add(egui::ProgressBar::new(frac).text(format!("{:.0}%{eta}", frac * 100.0)));
        if ui.button("Cancel").clicked() {
            app.cancel_export();
        }
    } else {
        let button = egui::Button::new(RichText::new("Export…").color(app.palette.on_accent)).fill(app.palette.accent).min_size(vec2(140.0, 28.0));
        if ui.add_enabled(app.photo.is_some(), button).clicked() {
            app.queue.push("export".into());
        }
        match &app.last_export {
            Some(Ok(s)) => {
                ui.weak(format!("Done: {s}"));
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            None => {}
        }
    }
}

fn log_panel(ui: &mut Ui) {
    egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
        let err = ui.visuals().error_fg_color;
        log::each(|is_err, text| {
            if is_err {
                ui.colored_label(err, text);
            } else {
                ui.weak(text);
            }
        });
    });
}

fn help(app: &mut App, ui: &mut Ui) {
    ui.weak("Click to run. Keys are set in keys.toml.");
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("help").num_columns(2).striped(true).show(ui, |ui| {
            for (name, _, what) in keys::COMMANDS {
                if ui.add(egui::Button::new(crate::menu::label(name)).frame(false)).on_hover_text(format!("{what}  ·  \"{name}\" in keys.toml")).clicked() {
                    app.queue.push((*name).into());
                }
                let key = app.keymap.key_for(name).map(|k| crate::menu::shortcut_text(&k)).unwrap_or_default();
                ui.label(RichText::new(key).monospace().color(app.palette.weak));
                ui.end_row();
            }
        });
    });
}

/// The grace9000 mark at `left_center`, `w` wide: a photo's frame, cut on
/// the same slant splice9000's strips are joined at, the lower part in the
/// signal color.
pub fn mark(p: &egui::Painter, left_center: Pos2, w: f32, pal: &Palette) {
    let light = if pal.dark { Color32::from_rgb(0xe8, 0xe9, 0xeb) } else { Color32::from_rgb(0x2a, 0x2c, 0x31) };
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - w / 2.0), vec2(w, w));
    let gap = w * 0.05;
    // The cut runs from 38% down the left edge to 62% down the right.
    let (yl, yr) = (r.top() + w * 0.38, r.top() + w * 0.62);
    p.add(egui::Shape::convex_polygon(vec![r.left_top(), r.right_top(), pos2(r.right(), yr - gap), pos2(r.left(), yl - gap)], light, Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![pos2(r.left(), yl + gap), pos2(r.right(), yr + gap), r.right_bottom(), r.left_bottom()], pal.accent, Stroke::NONE));
}

/// Every adapter: what it adds, what it may do, and Remove; then built-in
/// ones that were removed, with Add Back.
fn adapters_panel(app: &mut App, ui: &mut Ui) {
    let pal = app.palette;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.label(RichText::new("Drop an adapter's zip on this window to install it.").small().color(pal.weak));
        ui.add_space(8.0);
        if app.adapters.all.is_empty() {
            ui.weak("No adapters yet.");
        }
        let mut toggles = Vec::new();
        for (i, a) in app.adapters.all.iter().enumerate() {
            let m = &a.manifest;
            if i > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(&m.name).strong().color(pal.text));
                ui.label(RichText::new(&m.version).small().color(pal.weak));
                if a.builtin {
                    ui.label(RichText::new("BUILT IN").size(9.5).color(pal.accent));
                }
                if a.is_running() {
                    ui.label(RichText::new("running").size(9.5).color(pal.weak));
                }
            });
            if !m.description.is_empty() {
                ui.label(RichText::new(&m.description).color(pal.weak));
            }
            let can = if m.run.is_empty() {
                "Presets only: nothing runs".to_owned()
            } else if m.permissions.is_empty() {
                "No permissions".to_owned()
            } else {
                format!("Can {}", m.permissions.iter().map(|p| crate::adapters::describe(p)).collect::<Vec<_>>().join("; "))
            };
            ui.label(RichText::new(can).small().color(pal.weak));
            if !m.exports.is_empty() {
                let list: Vec<&str> = m.exports.iter().map(|e| e.label.as_str()).collect();
                ui.label(RichText::new(format!("Export formats: {}", list.join(", "))).small().color(pal.weak));
            }
            ui.horizontal_wrapped(|ui| {
                for c in &m.commands {
                    if ui.small_button(&c.label).clicked() {
                        app.queue.push(crate::adapters::command_name("adapter", &m.name, &c.id));
                    }
                }
                for p in &m.panels {
                    let kind = format!("adapter:{}/{}", m.name, p.id);
                    if ui.add(egui::Button::selectable(app.layout.contains(&kind), RichText::new(format!("{} panel", p.title)).small())).clicked() {
                        toggles.push((m.name.clone(), p.id.clone()));
                    }
                }
                let tip = if a.builtin { "Take it out of grace9000; Add Back brings it again" } else { "Move it to the Trash" };
                if ui.small_button("Remove").on_hover_text(tip).clicked() {
                    app.queue.push(format!("adapter-remove \"{}\"", m.name));
                }
                if !m.homepage.is_empty() && ui.small_button("Homepage").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&m.homepage));
                }
            });
        }
        for (a, id) in toggles {
            app.toggle_adapter_panel(&a, &id);
        }
        let removed = app.adapters.removed_builtins();
        if !removed.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("REMOVED").size(10.0).color(pal.weak));
            for m in removed {
                ui.horizontal(|ui| {
                    ui.label(&m.name);
                    if ui.small_button("Add Back").clicked() {
                        app.queue.push(format!("adapter-add-back \"{}\"", m.name));
                    }
                });
            }
        }
        ui.add_space(10.0);
        if ui.link("Install from a folder instead…").clicked() {
            app.queue.push("install adapter folder".into());
        }
    });
}

/// A panel an adapter fills with `panel.set`: lines of text, or buttons
/// that run its commands.
fn adapter_panel(app: &mut App, kind: &str, ui: &mut Ui) {
    let Some((name, _)) = crate::layout::adapter_panel(kind) else { return };
    let Some(i) = app.adapters.index(name) else {
        ui.weak(format!("{name} isn't installed."));
        return;
    };
    if !app.adapters.all[i].is_running() {
        if app.adapters.approved(i) {
            // Started once when its panel shows; after a crash, by hand.
            let tried = ui.ctx().data_mut(|d| std::mem::replace(d.get_temp_mut_or_default::<bool>(Id::unique(("started", kind))), true));
            if !tried || ui.button(format!("Start {name}")).clicked() {
                app.queue.push(format!("adapter-start \"{name}\""));
            }
        } else if ui.button(format!("Allow {name}…")).clicked() {
            app.queue.push(format!("adapter-start \"{name}\""));
        }
    }
    let lines = app.adapter_panels.get(kind).cloned().unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for line in &lines {
            match line {
                serde_json::Value::String(t) => {
                    ui.label(t);
                }
                serde_json::Value::Object(o) => {
                    let text = o.get("text").and_then(|t| t.as_str()).unwrap_or_default();
                    match o.get("command").and_then(|c| c.as_str()) {
                        Some(c) => {
                            if ui.button(text).clicked() {
                                app.queue.push(crate::adapters::command_name("adapter", name, c));
                            }
                        }
                        None => {
                            ui.label(text);
                        }
                    }
                }
                _ => {}
            }
        }
    });
}

/// The short message adapters and installs leave, at the bottom.
pub fn note(ui: &Ui, rect: Rect, app: &mut App) {
    let now = ui.ctx().input(|i| i.time);
    let Some((text, until)) = &app.note else { return };
    if now > *until {
        app.note = None;
        return;
    }
    let pal = app.palette;
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("note")));
    let galley = painter.layout(text.clone(), FontId::proportional(12.0), pal.text, (rect.width() - 80.0).max(200.0));
    let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 26.0), galley.size() + vec2(28.0, 14.0));
    painter.rect_filled(r, 6.0, pal.header);
    painter.rect_stroke(r, 6.0, Stroke::new(1.0, pal.line), StrokeKind::Inside);
    painter.galley(r.min + vec2(14.0, 7.0), galley, pal.text);
}

pub fn edit_banner(ui: &Ui, rect: Rect, pal: &Palette) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("layout banner")));
    let text = "EDIT LAYOUT  ·  drag panels onto each other  ·  right-click to add or remove  ·  Esc when done";
    let galley = painter.layout_no_wrap(text.into(), FontId::proportional(11.0), pal.on_accent);
    let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 22.0), galley.size() + vec2(24.0, 12.0));
    painter.rect_filled(r, 6.0, pal.accent);
    painter.galley(r.min + vec2(12.0, 6.0), galley, pal.on_accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_drag_by_edges_and_move() {
        let c = Crop { x: 0.2, y: 0.2, w: 0.5, h: 0.5 };
        let moved = drag_crop(c, vec2(0.1, 0.9), (false, false, false, false), None);
        assert!((moved.x - 0.3).abs() < 1e-6 && (moved.y - 0.5).abs() < 1e-6 && moved.w == 0.5);
        let wider = drag_crop(c, vec2(0.1, 0.0), (false, true, false, false), None);
        assert!((wider.w - 0.6).abs() < 1e-6 && wider.x == 0.2);
        // Past the opposite edge, it stops just short.
        let thin = drag_crop(c, vec2(2.0, 0.0), (true, false, false, false), None);
        assert!(thin.w > 0.0 && thin.x + thin.w <= 0.7 + 1e-6);
        // A square held square from a corner.
        let sq = drag_crop(c, vec2(-0.1, -0.3), (true, false, true, false), Some(1.0));
        assert!((sq.w - sq.h).abs() < 1e-5, "{sq:?}");
        assert!((sq.x + sq.w - 0.7).abs() < 1e-5 && (sq.y + sq.h - 0.7).abs() < 1e-5);
    }

    #[test]
    fn shapes_fit_inside() {
        let f = fit_crop(Crop::ALL, 2.0);
        assert!((f.w - 1.0).abs() < 1e-6 && (f.h - 0.5).abs() < 1e-6 && (f.y - 0.25).abs() < 1e-6);
        let f = fit_crop(Crop::ALL, 0.5);
        assert!((f.w - 0.5).abs() < 1e-6 && (f.h - 1.0).abs() < 1e-6);
    }

    #[test]
    fn summaries() {
        let mut s = Step::new("exposure");
        assert_eq!(summary(&s), "No change yet");
        s.values.insert("ev".into(), 1.5);
        assert_eq!(summary(&s), "Exposure 1.50 EV");
        let mut s = Step::new("tone");
        s.values.insert("shadows".into(), 40.0);
        assert_eq!(summary(&s), "Shadows +40");
        assert!(summary(&Step::new("from the future")).contains("newer"));
    }
}
