//! Turning frame rows into render nodes.

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk, pango};
use nyx_term::{Rgb, Row, Run, StyleFlags, Underline};

/// Cell geometry for one font at one scale, in logical pixels.
#[derive(Clone, Debug)]
pub struct Metrics {
    pub cell_w: f32,
    pub cell_h: f32,
    pub baseline: f32,
    /// Letter spacing (Pango units) that makes the font's advance equal the cell.
    pub spacing: i32,
    pub regular: pango::FontDescription,
    pub bold: pango::FontDescription,
    pub italic: pango::FontDescription,
    pub bold_italic: pango::FontDescription,
}

impl Metrics {
    pub fn new(ctx: &pango::Context, font: &pango::FontDescription, scale: f64) -> Self {
        let metrics = ctx.metrics(Some(font), None);
        let px = |v: i32| v as f64 / pango::SCALE as f64;
        // Snap cells to whole device pixels so rows and columns stay crisp.
        let snap = |v: f64| ((v * scale).ceil() / scale) as f32;
        let layout = pango::Layout::new(ctx);
        layout.set_font_description(Some(font));
        layout.set_text("MMMMMMMMMM");
        let advance = layout.pixel_size().0 as f64 / 10.0;
        let advance = advance.max(px(metrics.approximate_digit_width()));
        let cell_w = snap(advance);
        let ascent = px(metrics.ascent());
        let height = px(metrics.height()).max(ascent + px(metrics.descent()));
        let cell_h = snap(height);
        let spacing = ((cell_w as f64 - layout.size().0 as f64 / 10.0 / pango::SCALE as f64) * pango::SCALE as f64) as i32;
        let with = |w: pango::Weight, s: pango::Style| {
            let mut d = font.clone();
            d.set_weight(w);
            d.set_style(s);
            d
        };
        Self {
            cell_w,
            cell_h,
            baseline: snap(ascent + (cell_h as f64 - height) / 2.0),
            spacing,
            regular: font.clone(),
            bold: with(pango::Weight::Bold, pango::Style::Normal),
            italic: with(pango::Weight::Normal, pango::Style::Italic),
            bold_italic: with(pango::Weight::Bold, pango::Style::Italic),
        }
    }
}

pub fn rgba(c: Rgb, alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, alpha)
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> graphene::Rect {
    graphene::Rect::new(x, y, w, h)
}

/// One row as a render node with its origin at the row's top-left.
pub fn row_node(
    ctx: &pango::Context,
    m: &Metrics,
    row: &Row,
    default_bg: Rgb,
    selection: &gdk::RGBA,
    width_cells: u16,
) -> Option<gsk::RenderNode> {
    let snap = gtk::Snapshot::new();
    for run in &row.runs {
        if let Some(bg) = run.bg
            && bg != default_bg
        {
            snap.append_color(&rgba(bg, 1.0), &rect(run.col as f32 * m.cell_w, 0.0, run.cols as f32 * m.cell_w, m.cell_h));
        }
    }
    if let Some((a, b)) = row.selection {
        let b = b.min(width_cells.saturating_sub(1));
        snap.append_color(selection, &rect(a as f32 * m.cell_w, 0.0, (b + 1 - a.min(b + 1)) as f32 * m.cell_w, m.cell_h));
    }
    for run in &row.runs {
        draw_run(&snap, ctx, m, run);
    }
    snap.to_node()
}

fn draw_run(snap: &gtk::Snapshot, ctx: &pango::Context, m: &Metrics, run: &Run) {
    let x = run.col as f32 * m.cell_w;
    let w = run.cols as f32 * m.cell_w;
    let alpha = if run.flags.has(StyleFlags::FAINT) { 0.6 } else { 1.0 };
    let color = rgba(run.fg, alpha);
    let mut chars = run.text.chars();
    let sprite = match (chars.next(), chars.next()) {
        (Some(c), None) => crate::sprites::draw(snap, c, x, w, m.cell_h, &color),
        _ => false,
    };
    if !sprite && !run.text.trim().is_empty() {
        let layout = pango::Layout::new(ctx);
        let font = match (run.flags.has(StyleFlags::BOLD), run.flags.has(StyleFlags::ITALIC)) {
            (true, true) => &m.bold_italic,
            (true, false) => &m.bold,
            (false, true) => &m.italic,
            (false, false) => &m.regular,
        };
        layout.set_font_description(Some(font));
        layout.set_text(&run.text);
        if run.text.len() > 1 && run.text.is_ascii() && m.spacing != 0 {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_letter_spacing(m.spacing));
            layout.set_attributes(Some(&attrs));
        }
        let (lw, _) = layout.pixel_size();
        let baseline = layout.baseline() as f32 / pango::SCALE as f32;
        // A lone glyph is centred in its cells (wide characters, symbols).
        let dx = if run.cols <= 2 && !run.text.is_ascii() { ((w - lw as f32) / 2.0).max(0.0) } else { 0.0 };
        snap.push_clip(&rect(x, 0.0, w.max(lw as f32), m.cell_h));
        snap.save();
        snap.translate(&graphene::Point::new(x + dx, m.baseline - baseline));
        snap.append_layout(&layout, &color);
        snap.restore();
        snap.pop();
    }
    let line = |y: f32, h: f32, c: &gdk::RGBA| snap.append_color(c, &rect(x, y, w, h));
    let ucolor = run.underline_color.map(|c| rgba(c, alpha)).unwrap_or(color);
    let thick = (m.cell_h / 16.0).max(1.0).round();
    let under_y = (m.baseline + thick).min(m.cell_h - thick);
    match run.underline {
        Underline::None => {}
        Underline::Double => {
            line(under_y - thick, thick, &ucolor);
            line(under_y + thick, thick, &ucolor);
        }
        Underline::Dotted | Underline::Dashed => {
            let seg = if run.underline == Underline::Dotted { thick * 2.0 } else { m.cell_w / 2.0 };
            let mut px = x;
            while px < x + w {
                snap.append_color(&ucolor, &rect(px, under_y, seg.min(x + w - px), thick));
                px += seg * 2.0;
            }
        }
        Underline::Curly => {
            // Two-level zig-zag, one step per half cell.
            let step = (m.cell_w / 2.0).max(2.0);
            let mut px = x;
            let mut up = true;
            while px < x + w {
                let y = if up { under_y - thick } else { under_y };
                snap.append_color(&ucolor, &rect(px, y, step.min(x + w - px), thick));
                px += step;
                up = !up;
            }
        }
        Underline::Single => line(under_y, thick, &ucolor),
    }
    if run.flags.has(StyleFlags::STRIKETHROUGH) {
        line((m.baseline * 0.65).round(), thick, &color);
    }
    if run.flags.has(StyleFlags::OVERLINE) {
        line(0.0, thick, &color);
    }
}
