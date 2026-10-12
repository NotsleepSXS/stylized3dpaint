//! Compact Photoshop-style color picker: saturation/brightness square, hue strip,
//! hex and RGB fields. Works in sRGB-encoded HSB like Photoshop.

use egui::{Color32, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, vec2};

use crate::theme;

/// Remembers hue/saturation while the color is gray or black (where RGB loses them).
#[derive(Default)]
pub struct PickerState {
    rgb: [f32; 3],
    hsb: [f32; 3],
}

pub fn rgb_to_hsb([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

pub fn hsb_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = |n: f32| {
        let k = (n + h6) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

fn c32(rgb: [f32; 3]) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(rgb[0]), b(rgb[1]), b(rgb[2]))
}

/// Returns true when the color changed.
pub fn picker(ui: &mut egui::Ui, state: &mut PickerState, rgb: &mut [f32; 3], height: f32) -> bool {
    if state.rgb != *rgb {
        let mut hsb = rgb_to_hsb(*rgb);
        // Keep the old hue / saturation where they are undefined.
        if hsb[2] <= 0.0 {
            hsb[0] = state.hsb[0];
            hsb[1] = state.hsb[1];
        } else if hsb[1] <= 0.0 {
            hsb[0] = state.hsb[0];
        }
        state.hsb = hsb;
        state.rgb = *rgb;
    }
    let mut hsb = state.hsb;
    let width = ui.available_width();

    // Saturation (x) / brightness (y) square.
    let (sv_rect, sv) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let mut mesh = Mesh::default();
    // Brightness gradient is drawn as a grid so the falloff to black looks right.
    let rows = 16;
    for row in 0..=rows {
        let v = 1.0 - row as f32 / rows as f32;
        let y = sv_rect.top() + sv_rect.height() * row as f32 / rows as f32;
        mesh.colored_vertex(Pos2::new(sv_rect.left(), y), c32([v, v, v]));
        mesh.colored_vertex(Pos2::new(sv_rect.right(), y), c32(hsb_to_rgb([hsb[0], 1.0, v])));
        if row > 0 {
            let i = (row * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(sv_rect, 0.0, Stroke::new(1.0, theme::SEPARATOR), StrokeKind::Outside);
    if let Some(p) = sv.interact_pointer_pos().filter(|_| sv.is_pointer_button_down_on() || sv.clicked()) {
        hsb[1] = ((p.x - sv_rect.left()) / sv_rect.width()).clamp(0.0, 1.0);
        hsb[2] = 1.0 - ((p.y - sv_rect.top()) / sv_rect.height()).clamp(0.0, 1.0);
    }
    let marker = Pos2::new(sv_rect.left() + hsb[1] * sv_rect.width(), sv_rect.top() + (1.0 - hsb[2]) * sv_rect.height());
    ui.painter().circle_stroke(marker, 5.0, Stroke::new(2.5, Color32::BLACK));
    ui.painter().circle_stroke(marker, 5.0, Stroke::new(1.5, Color32::WHITE));

    // Hue strip.
    ui.add_space(6.0);
    let (hue_rect, hue_resp) = ui.allocate_exact_size(vec2(width, 12.0), Sense::click_and_drag());
    let mut mesh = Mesh::default();
    let steps = 12;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = hue_rect.left() + hue_rect.width() * t;
        let c = c32(hsb_to_rgb([t, 1.0, 1.0]));
        mesh.colored_vertex(Pos2::new(x, hue_rect.top()), c);
        mesh.colored_vertex(Pos2::new(x, hue_rect.bottom()), c);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    if let Some(p) = hue_resp.interact_pointer_pos().filter(|_| hue_resp.is_pointer_button_down_on() || hue_resp.clicked()) {
        hsb[0] = ((p.x - hue_rect.left()) / hue_rect.width()).clamp(0.0, 0.9999);
    }
    let hx = hue_rect.left() + hsb[0] * hue_rect.width();
    let handle = Rect::from_center_size(Pos2::new(hx, hue_rect.center().y), vec2(5.0, 16.0));
    ui.painter().rect_stroke(handle, 1.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);

    let mut changed = false;
    if hsb != state.hsb {
        state.hsb = hsb;
        *rgb = hsb_to_rgb(hsb);
        state.rgb = *rgb;
        changed = true;
    }

    // Hex + RGB fields.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, c32(*rgb));
        ui.painter().rect_stroke(swatch, 2.0, Stroke::new(1.0, theme::FIELD_BORDER), StrokeKind::Inside);
        let id = ui.id().with("hex");
        let mut hex = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
        let c = c32(*rgb);
        let current = format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
        let edit = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(62.0).char_limit(7).hint_text(&current));
        if !edit.has_focus() {
            hex = current;
        } else if let Ok(c) = Color32::from_hex(&format!("#{}", hex.trim_start_matches('#')))
            && edit.changed()
        {
            *rgb = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(id, hex));
        for (label, i) in [("R", 0), ("G", 1), ("B", 2)] {
            let mut v = (rgb[i] * 255.0).round() as i32;
            ui.label(egui::RichText::new(label).color(theme::TEXT_FAINT));
            if ui.add(egui::DragValue::new(&mut v).range(0..=255).speed(1.0)).changed() {
                rgb[i] = v as f32 / 255.0;
                changed = true;
            }
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsb_roundtrip() {
        for rgb in [[1.0, 0.0, 0.0], [0.2, 0.6, 0.4], [0.9, 0.9, 0.1], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0]] {
            let back = hsb_to_rgb(rgb_to_hsb(rgb));
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1e-5, "{rgb:?} -> {back:?}");
            }
        }
    }
}
