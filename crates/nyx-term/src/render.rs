use std::ptr;

use crate::ffi::*;
use crate::{Result, Rgb, Terminal};

/// How much changed since the last update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dirty {
    None,
    /// Some rows changed; see each [`Row::version`].
    Rows,
    /// Colours, size or something global changed; redraw everything.
    #[default]
    All,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StyleFlags(pub u16);

impl StyleFlags {
    pub const BOLD: u16 = 1 << 0;
    pub const ITALIC: u16 = 1 << 1;
    pub const FAINT: u16 = 1 << 2;
    pub const BLINK: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const INVISIBLE: u16 = 1 << 5;
    pub const STRIKETHROUGH: u16 = 1 << 6;
    pub const OVERLINE: u16 = 1 << 7;

    pub fn has(self, f: u16) -> bool {
        self.0 & f != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// A span of cells drawn with one style. Wide and non-ASCII graphemes get a
/// run of their own so a GUI can pin each to its cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub col: u16,
    /// Cells covered (a wide character covers two).
    pub cols: u16,
    pub text: String,
    /// Resolved foreground, after inverse.
    pub fg: Rgb,
    /// Resolved background, after inverse; `None` is the default background.
    pub bg: Option<Rgb>,
    pub flags: StyleFlags,
    pub underline: Underline,
    pub underline_color: Option<Rgb>,
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub runs: Vec<Run>,
    /// Selected columns, inclusive.
    pub selection: Option<(u16, u16)>,
    /// Bumped each time the row's content changes, for caching.
    pub version: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    #[default]
    Block,
    Underline,
    BlockHollow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Viewport cell, or `None` when the cursor is scrolled out of view.
    pub at: Option<(u16, u16)>,
    pub visible: bool,
    pub blinking: bool,
    pub wide: bool,
    pub style: CursorStyle,
    pub color: Option<Rgb>,
}

/// Everything needed to draw the viewport, updated in place.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub cols: u16,
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    pub foreground: Rgb,
    pub background: Rgb,
    pub dirty: Dirty,
    version: u64,
}

/// Reads a terminal into a [`Frame`], touching only rows that changed.
pub struct Renderer {
    state: GhosttyRenderState,
    rows: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    pub frame: Frame,
    utf8: Vec<u8>,
}

unsafe impl Send for Renderer {}

impl Renderer {
    pub fn new() -> Result<Self> {
        let mut state: GhosttyRenderState = ptr::null_mut();
        let mut rows: GhosttyRenderStateRowIterator = ptr::null_mut();
        let mut cells: GhosttyRenderStateRowCells = ptr::null_mut();
        unsafe {
            check(ghostty_render_state_new(ptr::null(), &mut state))?;
            check(ghostty_render_state_row_iterator_new(ptr::null(), &mut rows))?;
            check(ghostty_render_state_row_cells_new(ptr::null(), &mut cells))?;
        }
        Ok(Self { state, rows, cells, frame: Frame::default(), utf8: vec![0; 64] })
    }

    /// Bring the frame up to date. Returns how much changed.
    pub fn update(&mut self, term: &Terminal) -> Result<Dirty> {
        unsafe {
            check(ghostty_render_state_update(self.state, term.raw()))?;
        }
        let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        unsafe { ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_DIRTY, mut_ptr(&mut dirty)) };
        let mut cols: u16 = 0;
        let mut nrows: u16 = 0;
        unsafe {
            ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_COLS, mut_ptr(&mut cols));
            ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_ROWS, mut_ptr(&mut nrows));
        }
        let mut colors = sized::<GhosttyRenderStateColors>(|c, s| c.size = s);
        unsafe { ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_COLORS, mut_ptr(&mut colors)) };
        let fg = rgb(colors.foreground);
        let bg = rgb(colors.background);

        let mut all = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL;
        let f = &mut self.frame;
        if f.cols != cols || f.rows.len() != nrows as usize || f.foreground != fg || f.background != bg {
            all = true;
            f.cols = cols;
            f.rows.resize_with(nrows as usize, Row::default);
            f.foreground = fg;
            f.background = bg;
        }

        let mut cur = sized::<GhosttyRenderStateCursor>(|c, s| c.size = s);
        unsafe { ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_CURSOR, mut_ptr(&mut cur)) };
        f.cursor = Cursor {
            at: cur.viewport_has_value.then_some((cur.viewport_x, cur.viewport_y)),
            visible: cur.visible,
            blinking: cur.blinking,
            wide: cur.viewport_has_value && cur.wide_tail,
            style: match cur.visual_style {
                GHOSTTY_CURSOR_STYLE_BAR => CursorStyle::Bar,
                GHOSTTY_CURSOR_STYLE_UNDERLINE => CursorStyle::Underline,
                GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW => CursorStyle::BlockHollow,
                _ => CursorStyle::Block,
            },
            color: colors.cursor_has_value.then(|| rgb(colors.cursor)),
        };

        if dirty == GHOSTTY_RENDER_STATE_DIRTY_FALSE && !all {
            f.dirty = Dirty::None;
            return Ok(Dirty::None);
        }

        unsafe { ghostty_render_state_get(self.state, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, mut_ptr(&mut self.rows)) };
        let mut y = 0usize;
        while unsafe { ghostty_render_state_row_iterator_next(self.rows) } {
            if y >= f.rows.len() {
                break;
            }
            let mut row_dirty = false;
            unsafe { ghostty_render_state_row_get(self.rows, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, mut_ptr(&mut row_dirty)) };
            if all || row_dirty {
                f.version += 1;
                let row = &mut f.rows[y];
                row.version = f.version;
                let mut sel = sized::<GhosttyRenderStateRowSelection>(|s, size| s.size = size);
                let r = unsafe {
                    ghostty_render_state_row_get(self.rows, GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION, mut_ptr(&mut sel))
                };
                row.selection = (r == GHOSTTY_SUCCESS).then_some((sel.start_x, sel.end_x));
                unsafe { ghostty_render_state_row_get(self.rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, mut_ptr(&mut self.cells)) };
                read_row(self.cells, fg, bg, &mut self.utf8, &mut row.runs);
                let clean = false;
                unsafe { ghostty_render_state_row_set(self.rows, GHOSTTY_RENDER_STATE_ROW_OPT_DIRTY, ptr(&clean)) };
            }
            y += 1;
        }
        unsafe { ghostty_render_state_clean(self.state) };
        f.dirty = if all { Dirty::All } else { Dirty::Rows };
        Ok(f.dirty)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows);
            ghostty_render_state_free(self.state);
        }
    }
}

#[derive(PartialEq)]
struct Look {
    fg: Rgb,
    bg: Option<Rgb>,
    flags: u16,
    underline: Underline,
    underline_color: Option<Rgb>,
}

fn style_color(c: &GhosttyStyleColor, palette_fallback: Option<Rgb>) -> Option<Rgb> {
    match c.tag as u32 {
        GHOSTTY_STYLE_COLOR_RGB => Some(rgb(unsafe { c.value.rgb })),
        GHOSTTY_STYLE_COLOR_PALETTE => palette_fallback,
        _ => None,
    }
}

fn read_row(cells: GhosttyRenderStateRowCells, dfg: Rgb, dbg: Rgb, utf8: &mut Vec<u8>, runs: &mut Vec<Run>) {
    runs.clear();
    let mut col: u16 = 0;
    let mut current: Option<(Run, Look)> = None;
    while unsafe { ghostty_render_state_row_cells_next(cells) } {
        let x = col;
        col += 1;
        let mut raw: GhosttyCell = 0;
        unsafe { ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, mut_ptr(&mut raw)) };
        let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
        unsafe { ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, mut_ptr(&mut wide)) };
        if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD {
            continue;
        }

        let mut buf = GhosttyBuffer { ptr: utf8.as_mut_ptr(), cap: utf8.len(), len: 0 };
        let mut r = unsafe {
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, mut_ptr(&mut buf))
        };
        if r == GHOSTTY_OUT_OF_SPACE {
            utf8.resize(buf.len, 0);
            buf = GhosttyBuffer { ptr: utf8.as_mut_ptr(), cap: utf8.len(), len: 0 };
            r = unsafe {
                ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, mut_ptr(&mut buf))
            };
        }
        let text =
            if r == GHOSTTY_SUCCESS && buf.len > 0 { std::str::from_utf8(&utf8[..buf.len]).unwrap_or("\u{fffd}") } else { " " };

        let mut has_style = false;
        unsafe {
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING, mut_ptr(&mut has_style))
        };
        let mut fg = GhosttyColorRgb::default();
        let mut bg = GhosttyColorRgb::default();
        let fg = (unsafe {
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, mut_ptr(&mut fg))
        } == GHOSTTY_SUCCESS)
            .then(|| rgb(fg));
        let bg = (unsafe {
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, mut_ptr(&mut bg))
        } == GHOSTTY_SUCCESS)
            .then(|| rgb(bg));
        let (mut flags, mut underline, mut underline_color) = (0u16, Underline::None, None);
        if has_style {
            let mut st = GhosttyStyle::default();
            unsafe { ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, mut_ptr(&mut st)) };
            flags = st.flags & 0xff;
            underline = match ((st.flags >> 8) & 0x7) as u32 {
                GHOSTTY_SGR_UNDERLINE_SINGLE => Underline::Single,
                GHOSTTY_SGR_UNDERLINE_DOUBLE => Underline::Double,
                GHOSTTY_SGR_UNDERLINE_CURLY => Underline::Curly,
                GHOSTTY_SGR_UNDERLINE_DOTTED => Underline::Dotted,
                GHOSTTY_SGR_UNDERLINE_DASHED => Underline::Dashed,
                _ => Underline::None,
            };
            underline_color = style_color(&st.underline_color, fg);
        }
        let (mut fg, mut bg) = (fg.unwrap_or(dfg), bg);
        if flags & StyleFlags::INVERSE != 0 {
            let old_bg = bg.unwrap_or(dbg);
            bg = Some(fg);
            fg = old_bg;
        }
        let text = if flags & StyleFlags::INVISIBLE != 0 { " " } else { text };
        let cols = if wide == GHOSTTY_CELL_WIDE_WIDE { 2 } else { 1 };
        let simple = cols == 1 && text.len() == 1;
        let look = Look { fg, bg, flags, underline, underline_color };

        if simple
            && let Some((run, l)) = current.as_mut()
            && *l == look
            && run.col + run.cols == x
        {
            run.text.push_str(text);
            run.cols += 1;
            continue;
        }
        if let Some((run, _)) = current.take() {
            runs.push(run);
        }
        let run = Run { col: x, cols, text: text.to_string(), fg, bg, flags: StyleFlags(flags), underline, underline_color };
        if simple {
            current = Some((run, look));
        } else {
            runs.push(run);
        }
    }
    if let Some((run, _)) = current.take() {
        runs.push(run);
    }
    // Trailing blank cells with the default background draw nothing.
    while let Some(last) = runs.last_mut() {
        if last.bg.is_some() || last.underline != Underline::None || last.flags.has(StyleFlags::STRIKETHROUGH) {
            break;
        }
        let trimmed = last.text.trim_end_matches(' ').len();
        if trimmed == 0 {
            runs.pop();
        } else {
            let cut = last.text.len() - trimmed;
            last.text.truncate(trimmed);
            last.cols -= cut as u16;
            break;
        }
    }
}
