//! Graphical parameter editors shown in the Properties panel: gradient / palette bar, levels
//! curve, double-handled range slider and a direction sphere. Each returns a `Response` that
//! is marked changed on edits (dragging keeps one undo step per drag, like sliders).

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, pos2, vec2};

use crate::i18n::t;
use crate::theme;
use crate::ui_panels::{from_color32, to_color32};

fn srgb(c: [f32; 3]) -> [f32; 3] {
    c.map(s3dp_core::document::linear_to_srgb)
}

fn linear(c: [f32; 3]) -> [f32; 3] {
    c.map(s3dp_core::document::srgb_to_linear)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

// ------------------------------------------------------------------------------- gradient

/// One stop: color (linear) and position (None = palette swatch).
pub struct Stop {
    pub color: [f32; 3],
    pub pos: Option<f32>,
}

/// Gradient color at `x` like the shader (sRGB interpolation): 0 linear, 1 smooth, 2 constant.
pub fn gradient_eval(stops: &[Stop], x: f32, interp: u32) -> [f32; 3] {
    let n = stops.len();
    if n == 0 {
        return [0.0; 3];
    }
    let pos = |i: usize| stops[i].pos.unwrap_or(i as f32 / (n.max(2) - 1) as f32);
    if x <= pos(0) {
        return srgb(stops[0].color);
    }
    for i in 0..n - 1 {
        let (a, b) = (pos(i), pos(i + 1).max(pos(i)));
        if x <= b {
            let mut k = if b - a < 1e-6 { 1.0 } else { (x - a) / (b - a) };
            match interp {
                1 => k = k * k * (3.0 - 2.0 * k),
                2 => k = 0.0,
                _ => {}
            }
            return mix3(srgb(stops[i].color), srgb(stops[i + 1].color), k);
        }
    }
    srgb(stops[n - 1].color)
}

/// Gradient bar with draggable stops (positions kept ascending) or palette swatches, plus a
/// row with the selected stop's color / position and the stop count.
pub fn gradient(ui: &mut egui::Ui, id: egui::Id, stops: &mut [Stop], count: Option<(&mut i32, i32, i32)>, interp: u32) -> Response {
    let sel_id = id.with("selected");
    let mut selected: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    let used = count.as_ref().map_or(stops.len(), |(c, _, _)| (**c).clamp(1, stops.len() as i32) as usize);
    let stops_used = &mut stops[..used];
    selected = selected.min(used.saturating_sub(1));
    let palette = stops_used.iter().all(|s| s.pos.is_none());

    let width = ui.available_width().max(60.0);
    let (bar, mut resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let painter = ui.painter_at(bar.expand(1.0));
    if palette {
        let w = bar.width() / used as f32;
        for (i, s) in stops_used.iter().enumerate() {
            let r = Rect::from_min_size(pos2(bar.left() + w * i as f32, bar.top()), vec2(w, bar.height()));
            painter.rect_filled(r, 0.0, to_color32(srgb(s.color)));
            if i == selected {
                painter.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            selected = (((p.x - bar.left()) / w) as usize).min(used - 1);
        }
    } else {
        let cols = (bar.width() / 2.0).ceil() as usize;
        for k in 0..cols {
            let x0 = bar.left() + k as f32 * 2.0;
            let c = gradient_eval(stops_used, (k as f32 + 0.5) / cols as f32, interp);
            painter.rect_filled(Rect::from_min_max(pos2(x0, bar.top()), pos2((x0 + 2.0).min(bar.right()), bar.bottom())), 0.0, to_color32(c));
        }
        painter.rect_stroke(bar, 0.0, Stroke::new(1.0, theme::FIELD_BORDER), egui::StrokeKind::Outside);
        // Stop markers under the bar.
        let (marks, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
        let mp = ui.painter_at(marks.expand(2.0));
        for i in 0..used {
            let x = bar.left() + stops_used[i].pos.unwrap_or(0.0).clamp(0.0, 1.0) * bar.width();
            let hit = Rect::from_center_size(pos2(x, marks.center().y), vec2(12.0, 14.0));
            let r = ui.interact(hit, id.with(("stop", i)), Sense::click_and_drag());
            if r.clicked() || r.drag_started() {
                selected = i;
            }
            if r.dragged() {
                let lo = if i > 0 { stops_used[i - 1].pos.unwrap_or(0.0) } else { 0.0 };
                let hi = if i + 1 < used { stops_used[i + 1].pos.unwrap_or(1.0) } else { 1.0 };
                let p = stops_used[i].pos.unwrap_or(0.0) + r.drag_delta().x / bar.width();
                stops_used[i].pos = Some(p.clamp(lo, hi));
                resp.mark_changed();
            }
            let tri = vec![pos2(x, marks.top()), pos2(x - 6.0, marks.bottom()), pos2(x + 6.0, marks.bottom())];
            let stroke = if i == selected { Stroke::new(2.0, Color32::WHITE) } else { Stroke::new(1.0, Color32::from_gray(20)) };
            mp.add(egui::Shape::convex_polygon(tri, to_color32(srgb(stops_used[i].color)), stroke));
            resp = resp.union(r);
        }
    }
    // Selected stop: color, position, count.
    ui.horizontal(|ui| {
        if let Some(s) = stops_used.get_mut(selected) {
            let mut c = to_color32(srgb(s.color));
            let r = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
            if r.changed() {
                s.color = linear(from_color32(c));
                resp.mark_changed();
            }
            resp = resp.union(r);
            if let Some(p) = &mut s.pos {
                let r = ui.add(egui::DragValue::new(p).range(0.0..=1.0).speed(0.005).max_decimals(3)).on_hover_text(t("Stop position"));
                resp = resp.union(r);
            }
        }
        if let Some((c, min, max)) = count {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(*c < max, egui::Button::new("+").small()).on_hover_text(t("Add a stop")).clicked() {
                    *c += 1;
                    resp.mark_changed();
                }
                ui.label(format!("{c}"));
                if ui.add_enabled(*c > min, egui::Button::new("−").small()).on_hover_text(t("Remove the last stop")).clicked() {
                    *c -= 1;
                    resp.mark_changed();
                }
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(sel_id, selected));
    resp
}

// ------------------------------------------------------------------------------- range

/// Double-handled slider for `lo` / `hi` in `min..=max` (`lo <= hi` kept when `ordered`).
pub fn range(ui: &mut egui::Ui, id: egui::Id, lo: &mut f32, hi: &mut f32, min: f32, max: f32, ordered: bool, colors: [Color32; 2]) -> Response {
    let width = ui.available_width().max(60.0);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let track = Rect::from_min_max(pos2(rect.left() + 7.0, rect.center().y - 2.0), pos2(rect.right() - 7.0, rect.center().y + 2.0));
    let to_x = |v: f32| track.left() + ((v - min) / (max - min).max(1e-9)).clamp(0.0, 1.0) * track.width();
    let from_x = |x: f32| min + ((x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0) * (max - min);
    // Which handle a drag moves: chosen at drag start (nearest), remembered meanwhile.
    let grab_id = id.with("grab");
    if resp.drag_started() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let which = if (p.x - to_x(*lo)).abs() <= (p.x - to_x(*hi)).abs() { 0u8 } else { 1u8 };
            ui.data_mut(|d| d.insert_temp(grab_id, which));
        }
    }
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let which: u8 = ui.data(|d| d.get_temp(grab_id)).unwrap_or(0);
        let v = from_x(p.x);
        if which == 0 {
            *lo = if ordered { v.min(*hi) } else { v };
        } else {
            *hi = if ordered { v.max(*lo) } else { v };
        }
        resp.mark_changed();
    }
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(track, 2.0, theme::FIELD);
    let (a, b) = (to_x(*lo), to_x(*hi));
    painter.rect_filled(Rect::from_min_max(pos2(a.min(b), track.top()), pos2(a.max(b), track.bottom())), 2.0, theme::ACCENT);
    for (x, c) in [(a, colors[0]), (b, colors[1])] {
        painter.circle(pos2(x, rect.center().y), 6.0, c, Stroke::new(1.5, Color32::from_gray(20)));
    }
    resp
}

// ------------------------------------------------------------------------------- levels

fn levels_curve(x: f32, in_b: f32, in_w: f32, gamma: f32, out_b: f32, out_w: f32) -> f32 {
    let u = ((x - in_b) / (in_w - in_b).max(1e-5)).clamp(0.0, 1.0);
    let u = u.powf(1.0 / gamma.max(1e-3));
    out_b + (out_w - out_b) * u
}

/// Levels: the transfer curve, an input strip (black / gamma / white handles) and an output
/// range, plus numeric fields.
pub fn levels(ui: &mut egui::Ui, id: egui::Id, v: &mut [f32; 5]) -> Response {
    let [in_b, in_w, gamma, out_b, out_w] = v;
    let width = ui.available_width().max(80.0);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 96.0), Sense::hover());
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, 2.0, theme::FIELD);
    for k in 1..4 {
        let f = k as f32 / 4.0;
        let x = rect.left() + f * rect.width();
        let y = rect.bottom() - f * rect.height();
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(48)));
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, Color32::from_gray(48)));
    }
    p.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, Color32::from_gray(64)));
    let pts: Vec<Pos2> = (0..=96)
        .map(|k| {
            let x = k as f32 / 96.0;
            let y = levels_curve(x, *in_b, *in_w, *gamma, *out_b, *out_w).clamp(0.0, 1.0);
            pos2(rect.left() + x * rect.width(), rect.bottom() - y * rect.height())
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, theme::ACCENT)));

    // Input strip: black / white range plus the gamma (midtone) handle between them.
    ui.label(egui::RichText::new(t("Input")).small().color(theme::TEXT_FAINT));
    let r = range(ui, id.with("in"), in_b, in_w, 0.0, 1.0, true, [Color32::BLACK, Color32::WHITE]);
    let strip = r.rect;
    resp = resp.union(r);
    let track_l = strip.left() + 7.0;
    let track_w = strip.width() - 14.0;
    let mid_u = 0.5f32.powf(*gamma);
    let mid_x = track_l + (*in_b + mid_u * (*in_w - *in_b)).clamp(0.0, 1.0) * track_w;
    let mid = Rect::from_center_size(pos2(mid_x, strip.bottom() + 6.0), vec2(14.0, 12.0));
    let mr = ui.interact(mid, id.with("gamma"), Sense::drag()).on_hover_text(t("Midtones (gamma)"));
    if mr.dragged()
        && let Some(pp) = mr.interact_pointer_pos()
    {
        let x = ((pp.x - track_l) / track_w).clamp(0.0, 1.0);
        let u = ((x - *in_b) / (*in_w - *in_b).max(1e-5)).clamp(0.01, 0.99);
        *gamma = (u.ln() / 0.5f32.ln()).clamp(0.1, 5.0);
        resp.mark_changed();
    }
    ui.painter().add(egui::Shape::convex_polygon(
        vec![pos2(mid_x, mid.top()), pos2(mid_x - 6.0, mid.bottom()), pos2(mid_x + 6.0, mid.bottom())],
        Color32::from_gray(140),
        Stroke::new(1.0, Color32::from_gray(20)),
    ));
    resp = resp.union(mr);
    ui.add_space(10.0);
    ui.label(egui::RichText::new(t("Output")).small().color(theme::TEXT_FAINT));
    let r = range(ui, id.with("out"), out_b, out_w, 0.0, 1.0, false, [Color32::BLACK, Color32::WHITE]);
    resp = resp.union(r);
    ui.horizontal(|ui| {
        for (val, tip, lo, hi) in [(&mut *in_b, "In Black", 0.0, 1.0), (&mut *gamma, "Gamma", 0.1, 5.0), (&mut *in_w, "In White", 0.0, 1.0), (&mut *out_b, "Out Black", 0.0, 1.0), (&mut *out_w, "Out White", 0.0, 1.0)] {
            let r = ui.add(egui::DragValue::new(val).range(lo..=hi).speed(0.005).max_decimals(2)).on_hover_text(t(tip));
            resp = resp.union(r);
        }
    });
    resp
}

// ------------------------------------------------------------------------------- direction

/// Shaded sphere: drag to aim the direction (world axes: X right, Y up, Z towards you; the
/// back hemisphere with Shift held or when already pointing away).
pub fn direction_sphere(ui: &mut egui::Ui, d: &mut [f32; 3], size: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let c = rect.center();
    let r = size * 0.5 - 2.0;
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    let dir = [d[0] / len, d[1] / len, d[2] / len];
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let mut x = (p.x - c.x) / r;
        let mut y = -(p.y - c.y) / r;
        let l = (x * x + y * y).sqrt();
        if l > 1.0 {
            x /= l;
            y /= l;
        }
        let back = ui.input(|i| i.modifiers.shift) || (dir[2] < 0.0 && !resp.drag_started() && !resp.clicked());
        let z = (1.0 - x * x - y * y).max(0.0).sqrt() * if back { -1.0 } else { 1.0 };
        *d = [x, y, z];
        resp.mark_changed();
    }
    let painter = ui.painter_at(rect.expand(1.0));
    // Fake lit sphere: concentric discs drifting towards the light.
    let light = vec2(dir[0], -dir[1]);
    let steps = 14;
    for k in 0..steps {
        let f = k as f32 / steps as f32;
        let shade = if dir[2] >= 0.0 { 40.0 + 170.0 * f } else { 40.0 + 70.0 * f };
        painter.circle_filled(c + light * r * 0.45 * f, r * (1.0 - f * 0.92), Color32::from_gray(shade as u8));
    }
    painter.circle_stroke(c, r, Stroke::new(1.0, theme::FIELD_BORDER));
    painter.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], Stroke::new(1.0, Color32::from_black_alpha(60)));
    painter.line_segment([pos2(c.x, c.y - r), pos2(c.x, c.y + r)], Stroke::new(1.0, Color32::from_black_alpha(60)));
    let dot = c + light * r;
    if dir[2] >= 0.0 {
        painter.circle_filled(dot, 4.0, Color32::from_rgb(255, 210, 120));
    } else {
        painter.circle_stroke(dot, 4.0, Stroke::new(1.5, Color32::from_rgb(255, 210, 120)));
    }
    resp.on_hover_text(t("Drag to aim · Shift: back side"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_eval_matches_stops() {
        let near = |a: [f32; 3], b: f32| a.iter().all(|v| (v - b).abs() < 1e-5);
        let stops = [Stop { color: [0.0; 3], pos: Some(0.2) }, Stop { color: [1.0; 3], pos: Some(0.8) }];
        assert!(near(gradient_eval(&stops, 0.0, 0), 0.0));
        assert!(near(gradient_eval(&stops, 1.0, 0), 1.0));
        assert!(near(gradient_eval(&stops, 0.5, 0), 0.5), "linear midpoint in sRGB");
        assert!(near(gradient_eval(&stops, 0.5, 2), 0.0), "constant");
        let pal = [Stop { color: [0.0; 3], pos: None }, Stop { color: [1.0; 3], pos: None }];
        assert!(near(gradient_eval(&pal, 1.0, 0), 1.0));
    }

    #[test]
    fn levels_curve_matches_shader() {
        assert_eq!(levels_curve(0.5, 0.0, 1.0, 1.0, 0.0, 1.0), 0.5);
        assert_eq!(levels_curve(0.1, 0.2, 0.8, 1.0, 0.0, 1.0), 0.0);
        assert_eq!(levels_curve(0.9, 0.2, 0.8, 1.0, 0.0, 1.0), 1.0);
        // Gamma handle: output 0.5 sits at in_b + 0.5^gamma * (in_w - in_b).
        let g = 2.0f32;
        let x = 0.5f32.powf(g);
        assert!((levels_curve(x, 0.0, 1.0, g, 0.0, 1.0) - 0.5).abs() < 1e-5);
    }
}
