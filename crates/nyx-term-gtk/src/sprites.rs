//! Box drawing (U+2500-257F) and block elements (U+2580-259F) drawn as
//! shapes that fill the cell exactly, so lines join across cells and rows
//! with no gaps, whatever the font's glyph height.

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk};

use crate::draw::rect;

const NONE: u8 = 0;
const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Arms of a box-drawing character: up, right, down, left.
fn arms(c: char) -> Option<[u8; 4]> {
    let (n, l, h, d) = (NONE, LIGHT, HEAVY, DOUBLE);
    Some(match c {
        '─' | '┄' | '┈' | '╌' => [n, l, n, l],
        '━' | '┅' | '┉' | '╍' => [n, h, n, h],
        '│' | '┆' | '┊' | '╎' => [l, n, l, n],
        '┃' | '┇' | '┋' | '╏' => [h, n, h, n],
        '┌' => [n, l, l, n],
        '┍' => [n, h, l, n],
        '┎' => [n, l, h, n],
        '┏' => [n, h, h, n],
        '┐' => [n, n, l, l],
        '┑' => [n, n, l, h],
        '┒' => [n, n, h, l],
        '┓' => [n, n, h, h],
        '└' => [l, l, n, n],
        '┕' => [l, h, n, n],
        '┖' => [h, l, n, n],
        '┗' => [h, h, n, n],
        '┘' => [l, n, n, l],
        '┙' => [l, n, n, h],
        '┚' => [h, n, n, l],
        '┛' => [h, n, n, h],
        '├' => [l, l, l, n],
        '┝' => [l, h, l, n],
        '┞' => [h, l, l, n],
        '┟' => [l, l, h, n],
        '┠' => [h, l, h, n],
        '┡' => [h, h, l, n],
        '┢' => [l, h, h, n],
        '┣' => [h, h, h, n],
        '┤' => [l, n, l, l],
        '┥' => [l, n, l, h],
        '┦' => [h, n, l, l],
        '┧' => [l, n, h, l],
        '┨' => [h, n, h, l],
        '┩' => [h, n, l, h],
        '┪' => [l, n, h, h],
        '┫' => [h, n, h, h],
        '┬' => [n, l, l, l],
        '┭' => [n, l, l, h],
        '┮' => [n, h, l, l],
        '┯' => [n, h, l, h],
        '┰' => [n, l, h, l],
        '┱' => [n, l, h, h],
        '┲' => [n, h, h, l],
        '┳' => [n, h, h, h],
        '┴' => [l, l, n, l],
        '┵' => [l, l, n, h],
        '┶' => [l, h, n, l],
        '┷' => [l, h, n, h],
        '┸' => [h, l, n, l],
        '┹' => [h, l, n, h],
        '┺' => [h, h, n, l],
        '┻' => [h, h, n, h],
        '┼' => [l, l, l, l],
        '┽' => [l, l, l, h],
        '┾' => [l, h, l, l],
        '┿' => [l, h, l, h],
        '╀' => [h, l, l, l],
        '╁' => [l, l, h, l],
        '╂' => [h, l, h, l],
        '╃' => [h, l, l, h],
        '╄' => [h, h, l, l],
        '╅' => [l, l, h, h],
        '╆' => [l, h, h, l],
        '╇' => [h, h, l, h],
        '╈' => [l, h, h, h],
        '╉' => [h, l, h, h],
        '╊' => [h, h, h, l],
        '╋' => [h, h, h, h],
        '═' => [n, d, n, d],
        '║' => [d, n, d, n],
        '╔' => [n, d, d, n],
        '╗' => [n, n, d, d],
        '╚' => [d, d, n, n],
        '╝' => [d, n, n, d],
        '╠' => [d, d, d, n],
        '╣' => [d, n, d, d],
        '╦' => [n, d, d, d],
        '╩' => [d, d, n, d],
        '╬' => [d, d, d, d],
        '╴' => [n, n, n, l],
        '╵' => [l, n, n, n],
        '╶' => [n, l, n, n],
        '╷' => [n, n, l, n],
        '╸' => [n, n, n, h],
        '╹' => [h, n, n, n],
        '╺' => [n, h, n, n],
        '╻' => [n, n, h, n],
        '╼' => [n, h, n, l],
        '╽' => [l, n, h, n],
        '╾' => [n, l, n, h],
        '╿' => [h, n, l, n],
        _ => return None,
    })
}

/// Draw `c` in the cell at `x` (cell origin at row top). Returns false when
/// `c` is not something drawn here.
pub fn draw(snap: &gtk::Snapshot, c: char, x: f32, w: f32, h: f32, color: &gdk::RGBA) -> bool {
    let light = (h / 16.0).round().max(1.0);
    let heavy = light * 2.0;
    let cx = (x + w / 2.0 - light / 2.0).round() + light / 2.0;
    let cy = (h / 2.0 - light / 2.0).round() + light / 2.0;
    let fill = |r: graphene::Rect| snap.append_color(color, &r);

    if let Some(a) = arms(c) {
        let thick = |k: u8| if k == HEAVY { heavy } else { light };
        // How far each arm reaches past the centre, so joins have no notch.
        let reach_v = a[1].max(a[3]);
        let reach_h = a[0].max(a[2]);
        let ext = |other: u8| {
            if other == NONE {
                0.0
            } else if other == DOUBLE {
                light * 1.5
            } else {
                thick(other) / 2.0
            }
        };
        let gap = light;
        for (i, &k) in a.iter().enumerate() {
            if k == NONE {
                continue;
            }
            let vertical = i == 0 || i == 2;
            let e = if vertical { ext(reach_v) } else { ext(reach_h) };
            let offsets: &[f32] = if k == DOUBLE { &[-gap - light / 2.0, gap + light / 2.0] } else { &[0.0] };
            for &off in offsets {
                let t = if k == DOUBLE { light } else { thick(k) };
                let r = match i {
                    0 => rect(cx + off - t / 2.0, 0.0, t, cy + e),
                    2 => rect(cx + off - t / 2.0, cy - e, t, h - cy + e),
                    3 => rect(x, cy + off - t / 2.0, cx - x + e, t),
                    _ => rect(cx - e, cy + off - t / 2.0, x + w - cx + e, t),
                };
                fill(r);
            }
        }
        return true;
    }

    let block = |fx: f32, fy: f32, fw: f32, fh: f32| {
        fill(rect(x + (fx * w).round(), (fy * h).round(), (fw * w).round(), (fh * h).round()))
    };
    let shade = |alpha: f32| {
        let mut c = *color;
        c.set_alpha(color.alpha() * alpha);
        snap.append_color(&c, &rect(x, 0.0, w, h));
    };
    match c {
        '▀' => block(0.0, 0.0, 1.0, 0.5),
        '▁'..='▇' => {
            let eighths = (c as u32 - 0x2580) as f32;
            block(0.0, 1.0 - eighths / 8.0, 1.0, eighths / 8.0)
        }
        '█' => block(0.0, 0.0, 1.0, 1.0),
        '▉'..='▏' => {
            let eighths = (0x2590 - c as u32) as f32;
            block(0.0, 0.0, eighths / 8.0, 1.0)
        }
        '▐' => block(0.5, 0.0, 0.5, 1.0),
        '░' => shade(0.25),
        '▒' => shade(0.5),
        '▓' => shade(0.75),
        '▔' => block(0.0, 0.0, 1.0, 0.125),
        '▕' => block(0.875, 0.0, 0.125, 1.0),
        '▖'..='▟' => {
            // Quadrants: bit 0 upper-left, 1 upper-right, 2 lower-left, 3 lower-right.
            let q: u8 = match c {
                '▖' => 0b0100,
                '▗' => 0b1000,
                '▘' => 0b0001,
                '▙' => 0b1101,
                '▚' => 0b1001,
                '▛' => 0b0111,
                '▜' => 0b1011,
                '▝' => 0b0010,
                '▞' => 0b0110,
                _ => 0b1110,
            };
            for (bit, (fx, fy)) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)].into_iter().enumerate() {
                if q & (1 << bit) != 0 {
                    block(fx, fy, 0.5, 0.5);
                }
            }
        }
        '╭' | '╮' | '╯' | '╰' => {
            // A quarter curve from the cell edge into the centre lines.
            let (ex, ey) = match c {
                '╭' => (x + w, h),
                '╮' => (x, h),
                '╯' => (x, 0.0),
                _ => (x + w, 0.0),
            };
            let pb = gsk::PathBuilder::new();
            pb.move_to(cx, ey);
            let r = (w / 2.0).min(h / 2.0);
            let toward = |from: f32, to: f32| from + (to - from).signum() * r;
            pb.line_to(cx, toward(cy, ey));
            pb.quad_to(cx, cy, toward(cx, ex), cy);
            pb.line_to(ex, cy);
            snap.append_stroke(&pb.to_path(), &gsk::Stroke::new(light), color);
        }
        '╱' | '╲' | '╳' => {
            let pb = gsk::PathBuilder::new();
            if c != '╲' {
                pb.move_to(x + w, 0.0);
                pb.line_to(x, h);
            }
            if c != '╱' {
                pb.move_to(x, 0.0);
                pb.line_to(x + w, h);
            }
            snap.append_stroke(&pb.to_path(), &gsk::Stroke::new(light), color);
        }
        _ => return false,
    }
    true
}
