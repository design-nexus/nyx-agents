use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango};
use nyx_term::{
    Colors, Dirty, Effects, Key, KeyAction, KeyEncoder, Mods, MouseAction, MouseButton, MouseEncoder, Renderer, ScrollTo, Search,
    SearchMatch, SelectGesture, StatusReport, Terminal, TerminalOptions,
};

use crate::draw::{self, Metrics, rect, rgba};
use crate::links;

/// How a view looks.
#[derive(Clone, Debug)]
pub struct ViewStyle {
    pub colors: Colors,
    pub font: pango::FontDescription,
    pub selection: gdk::RGBA,
    pub search: gdk::RGBA,
    pub search_current: gdk::RGBA,
    /// Space around the grid, in logical pixels.
    pub padding: f32,
}

impl Default for ViewStyle {
    fn default() -> Self {
        Self {
            colors: Colors::catppuccin_mocha(),
            font: pango::FontDescription::from_string("monospace 11"),
            selection: gdk::RGBA::new(0.54, 0.71, 0.98, 0.35),
            search: gdk::RGBA::new(0.98, 0.89, 0.69, 0.35),
            search_current: gdk::RGBA::new(0.98, 0.70, 0.53, 0.6),
            padding: 6.0,
        }
    }
}

/// Drawing counters, for benchmarks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub rows_built: u64,
    pub draw_time: Duration,
    pub max_draw: Duration,
    pub bytes_fed: u64,
    pub parse_time: Duration,
}

type Handler<T> = RefCell<Vec<Box<dyn Fn(&T)>>>;

/// A drawn row: content version, selection it was drawn with, and the node.
type CachedRow = Option<(u64, Option<(u16, u16)>, gsk::RenderNode)>;

#[derive(Default)]
struct Handlers {
    input: Handler<[u8]>,
    resize: Handler<(u16, u16, u32, u32)>,
    title: Handler<str>,
    status: Handler<StatusReport>,
    bell: Handler<()>,
    notification: Handler<(String, String)>,
    frame: Handler<Stats>,
    image_paste: Handler<()>,
}

struct State {
    term: Terminal,
    renderer: Renderer,
    keys: KeyEncoder,
    mouse: MouseEncoder,
    search: Option<(String, Search)>,
    matches: Vec<SearchMatch>,
    style: ViewStyle,
    metrics: Option<Metrics>,
    cache: Vec<CachedRow>,
    grid: (u16, u16),
    held_until: Option<Instant>,
    preedit: String,
    stats: Stats,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TerminalView {
        pub(super) state: RefCell<Option<State>>,
        pub(super) handlers: Handlers,
        pub(super) read_only: Cell<bool>,
        pub(super) answers_queries: Cell<bool>,
        pub(super) fit: Cell<bool>,
        pub(super) focused: Cell<bool>,
        pub(super) buttons: Cell<u32>,
        pub(super) selecting: Cell<bool>,
        /// The button press under way goes to the program (Shift was held).
        pub(super) passing: Cell<bool>,
        pub(super) scroll_rest: Cell<f64>,
        pub(super) im: RefCell<Option<gtk::IMMulticontext>>,
        pub(super) in_key: Cell<bool>,
        pub(super) committed: RefCell<Option<String>>,
        pub(super) blink_on: Cell<bool>,
        /// Where the cursor was when assistive technology last heard of it.
        pub(super) told_caret: Cell<Option<(u16, u16)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TerminalView {
        const NAME: &'static str = "NyxTerminalView";
        type Type = super::TerminalView;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::AccessibleText,);

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("nyx-terminal");
            klass.set_accessible_role(gtk::AccessibleRole::Terminal);
        }
    }

    impl ObjectImpl for TerminalView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            self.blink_on.set(true);
        }
    }

    // The screen as text for screen readers: its rows, one per line, the caret at the
    // cursor. Only the caret's moves are announced, not each redraw.
    impl AccessibleTextImpl for TerminalView {
        fn contents(&self, start: u32, end: u32) -> Option<glib::Bytes> {
            let text: String = self.obj().screen_text().chars().skip(start as usize).take(end.saturating_sub(start) as usize).collect();
            Some(glib::Bytes::from_owned(text.into_bytes()))
        }

        fn contents_at(&self, offset: u32, granularity: gtk::AccessibleTextGranularity) -> Option<(u32, u32, glib::Bytes)> {
            let chars: Vec<char> = self.obj().screen_text().chars().collect();
            let at = (offset as usize).min(chars.len());
            let (a, b) = if granularity == gtk::AccessibleTextGranularity::Character {
                (at, (at + 1).min(chars.len()))
            } else {
                let a = chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
                let b = chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| at + i);
                (a, b)
            };
            let text: String = chars[a..b].iter().collect();
            Some((a as u32, b as u32, glib::Bytes::from_owned(text.into_bytes())))
        }

        fn caret_position(&self) -> u32 {
            self.obj().caret_offset()
        }

        fn selection(&self) -> Vec<gtk::AccessibleTextRange> {
            Vec::new()
        }

        fn attributes(&self, _offset: u32) -> Vec<(gtk::AccessibleTextRange, glib::GString, glib::GString)> {
            Vec::new()
        }

        fn default_attributes(&self) -> Vec<(glib::GString, glib::GString)> {
            Vec::new()
        }
    }

    impl WidgetImpl for TerminalView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();
            let (cw, ch, pad) = obj.cell_size();
            let nat = if orientation == gtk::Orientation::Horizontal { 80.0 * cw } else { 24.0 * ch };
            let min = if orientation == gtk::Orientation::Horizontal { 10.0 * cw } else { 3.0 * ch };
            ((min + 2.0 * pad) as i32, (nat + 2.0 * pad) as i32, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.obj().relayout(width, height);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }

        fn realize(&self) {
            self.parent_realize();
            self.obj().reset_metrics();
        }
    }
}

glib::wrapper! {
    pub struct TerminalView(ObjectSubclass<imp::TerminalView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleText, gtk::Buildable, gtk::ConstraintTarget;
}

impl TerminalView {
    /// A view over a fresh terminal.
    pub fn new(style: ViewStyle, read_only: bool) -> Self {
        let term = Terminal::new(&TerminalOptions { colors: style.colors.clone(), ..Default::default() }).expect("terminal");
        Self::with_terminal(term, style, read_only)
    }

    /// A view over an existing terminal (for example one restored from a snapshot).
    pub fn with_terminal(term: Terminal, style: ViewStyle, read_only: bool) -> Self {
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        let grid = (term.cols(), term.rows());
        *imp.state.borrow_mut() = Some(State {
            term,
            renderer: Renderer::new().expect("renderer"),
            keys: KeyEncoder::new().expect("key encoder"),
            mouse: MouseEncoder::new().expect("mouse encoder"),
            search: None,
            matches: Vec::new(),
            style,
            metrics: None,
            cache: Vec::new(),
            grid,
            held_until: None,
            preedit: String::new(),
            stats: Stats::default(),
        });
        imp.read_only.set(read_only);
        imp.answers_queries.set(true);
        obj.set_focusable(!read_only);
        obj.set_focus_on_click(!read_only);
        obj.install_controllers();
        obj
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(self.imp().state.borrow_mut().as_mut().expect("state"))
    }

    /// The viewport's text, a line per row, trailing blanks dropped.
    fn screen_text(&self) -> String {
        let st = self.imp().state.borrow();
        let Some(s) = st.as_ref() else { return String::new() };
        (0..s.term.rows()).map(|r| s.term.row_text(r).trim_end().to_string()).collect::<Vec<_>>().join("\n")
    }

    /// The cursor as an offset into [`Self::screen_text`].
    fn caret_offset(&self) -> u32 {
        let st = self.imp().state.borrow();
        let Some(s) = st.as_ref() else { return 0 };
        let Some((col, row)) = s.renderer.frame.cursor.at else { return 0 };
        let before: usize = (0..row).map(|r| s.term.row_text(r).trim_end().chars().count() + 1).sum();
        (before + (col as usize).min(s.term.row_text(row).trim_end().chars().count())) as u32
    }

    pub fn is_read_only(&self) -> bool {
        self.imp().read_only.get()
    }

    /// Whether this view answers the program's terminal queries (cursor
    /// position, colours, modes). Turn it off when something else owns the
    /// program's terminal and answers for it, such as a session holder;
    /// otherwise the program would get every answer twice.
    pub fn set_answers_queries(&self, answers: bool) {
        self.imp().answers_queries.set(answers);
    }

    /// Let a read-only view size its terminal to the widget, for content
    /// that is not a program's terminal (a rendered transcript). Mirrors of a
    /// real terminal keep the program's grid instead.
    pub fn set_fit(&self, fit: bool) {
        self.imp().fit.set(fit);
        self.queue_resize();
    }

    /// Run `f` on the terminal (for example to read text or take a snapshot).
    pub fn terminal<R>(&self, f: impl FnOnce(&mut Terminal) -> R) -> R {
        let r = self.with_state(|s| f(&mut s.term));
        self.queue_draw();
        r
    }

    /// Swap in another terminal, such as one restored from a daemon snapshot.
    pub fn replace_terminal(&self, term: Terminal) {
        self.with_state(|s| {
            s.term = term;
            s.renderer = Renderer::new().expect("renderer");
            s.cache.clear();
            s.search = None;
            s.matches.clear();
        });
        let (w, h) = (self.width(), self.height());
        if w > 0 && !self.is_read_only() {
            self.with_state(|s| s.grid = (0, 0));
            self.relayout(w, h);
        }
        self.queue_draw();
    }

    /// Bytes for the program: keys, mouse reports, pastes and query replies.
    pub fn connect_input(&self, f: impl Fn(&[u8]) + 'static) {
        self.imp().handlers.input.borrow_mut().push(Box::new(f));
    }

    /// Ctrl+V with only an image on the clipboard. Without a handler the key
    /// goes to the program (which reads the image itself); with one, the
    /// handler does that once it has made the image readable, through
    /// [`TerminalView::send_input`].
    pub fn connect_image_paste(&self, f: impl Fn(&()) + 'static) {
        self.imp().handlers.image_paste.borrow_mut().push(Box::new(f));
    }

    /// Bytes for the program, as if typed.
    pub fn send_input(&self, bytes: &[u8]) {
        self.send(bytes);
    }

    /// The grid changed: (cols, rows, width px, height px).
    pub fn connect_resize(&self, f: impl Fn(&(u16, u16, u32, u32)) + 'static) {
        self.imp().handlers.resize.borrow_mut().push(Box::new(f));
    }

    pub fn connect_title(&self, f: impl Fn(&str) + 'static) {
        self.imp().handlers.title.borrow_mut().push(Box::new(f));
    }

    /// OSC 7501 program status reports.
    pub fn connect_status(&self, f: impl Fn(&StatusReport) + 'static) {
        self.imp().handlers.status.borrow_mut().push(Box::new(f));
    }

    pub fn connect_bell(&self, f: impl Fn(&()) + 'static) {
        self.imp().handlers.bell.borrow_mut().push(Box::new(f));
    }

    /// OSC 9 / 777 desktop notifications the program asked for: (title, body).
    pub fn connect_notification(&self, f: impl Fn(&(String, String)) + 'static) {
        self.imp().handlers.notification.borrow_mut().push(Box::new(f));
    }

    /// Called after each drawn frame with running counters.
    pub fn connect_frame(&self, f: impl Fn(&Stats) + 'static) {
        self.imp().handlers.frame.borrow_mut().push(Box::new(f));
    }

    fn emit<T: ?Sized>(handlers: &Handler<T>, v: &T) {
        for h in handlers.borrow().iter() {
            h(v);
        }
    }

    fn send(&self, bytes: &[u8]) {
        if bytes.is_empty() || self.is_read_only() {
            return;
        }
        Self::emit(&self.imp().handlers.input, bytes);
    }

    /// Program output.
    pub fn feed(&self, bytes: &[u8]) {
        let t = Instant::now();
        let fx = self.with_state(|s| {
            s.term.write(bytes);
            s.stats.bytes_fed += bytes.len() as u64;
            s.stats.parse_time += t.elapsed();
            s.term.take_effects()
        });
        self.handle_effects(fx);
        let held = self.with_state(|s| s.held_until.is_some_and(|until| Instant::now() < until));
        if !held {
            self.queue_draw();
        }
    }

    fn handle_effects(&self, fx: Effects) {
        if fx.is_empty() {
            return;
        }
        let h = &self.imp().handlers;
        // A viewer must not answer for the program's real terminal.
        if !self.is_read_only() {
            if self.imp().answers_queries.get() {
                self.send(&fx.reply);
            }
            for c in &fx.clipboard {
                let clip = if c.primary { self.primary_clipboard() } else { self.clipboard() };
                clip.set_text(&c.text);
            }
        }
        if fx.title_changed {
            let title = self.with_state(|s| s.term.title());
            Self::emit(&h.title, title.as_str());
        }
        for s in &fx.status {
            Self::emit(&h.status, s);
        }
        if fx.bells > 0 {
            Self::emit(&h.bell, &());
        }
        for n in &fx.notifications {
            Self::emit(&h.notification, n);
        }
        if let Some(held) = fx.render_hold {
            // Synchronized output: hold drawing until the program finishes
            // its update, but never longer than a second.
            self.with_state(|s| s.held_until = held.then(|| Instant::now() + Duration::from_secs(1)));
            if held {
                let weak = self.downgrade();
                glib::timeout_add_local_once(Duration::from_millis(1000), move || {
                    if let Some(v) = weak.upgrade() {
                        v.queue_draw();
                    }
                });
            } else {
                self.queue_draw();
            }
        }
    }

    pub fn set_style(&self, style: ViewStyle) {
        self.with_state(|s| {
            s.term.set_colors(&style.colors);
            s.style = style;
            s.cache.clear();
        });
        self.reset_metrics();
    }

    pub fn style(&self) -> ViewStyle {
        self.with_state(|s| s.style.clone())
    }

    /// The terminal grid (cols, rows).
    pub fn grid(&self) -> (u16, u16) {
        self.with_state(|s| s.grid)
    }

    pub fn stats(&self) -> Stats {
        self.with_state(|s| s.stats)
    }

    fn reset_metrics(&self) {
        let ctx = self.pango_context();
        let scale = self.scale_factor() as f64;
        self.with_state(|s| {
            s.metrics = Some(Metrics::new(&ctx, &s.style.font, scale));
            s.cache.clear();
        });
        self.queue_resize();
        self.queue_draw();
    }

    /// (cell width, cell height, padding) in logical pixels.
    pub fn cell_size(&self) -> (f32, f32, f32) {
        let st = self.imp().state.borrow();
        match st.as_ref() {
            Some(State { metrics: Some(m), style, .. }) => (m.cell_w, m.cell_h, style.padding),
            Some(s) => (8.0, 16.0, s.style.padding),
            None => (8.0, 16.0, 6.0),
        }
    }

    fn relayout(&self, width: i32, height: i32) {
        let (cw, ch, pad) = self.cell_size();
        // Before layout settles a view can be a sliver; telling the program it
        // has one row or a few columns makes shells and TUIs misbehave.
        if (width as f32) < 10.0 * cw + 2.0 * pad || (height as f32) < 2.0 * ch + 2.0 * pad {
            return;
        }
        let cols = (((width as f32 - 2.0 * pad) / cw).floor() as i32).max(2) as u16;
        let rows = (((height as f32 - 2.0 * pad) / ch).floor() as i32).max(1) as u16;
        if self.is_read_only() && !self.imp().fit.get() {
            return;
        }
        let changed = self.with_state(|s| {
            if s.grid == (cols, rows) {
                return false;
            }
            s.grid = (cols, rows);
            let _ = s.term.resize(cols, rows, cw.round() as u32, ch.round() as u32);
            s.cache.clear();
            true
        });
        if changed && !self.is_read_only() {
            let px = ((cols as f32 * cw) as u32, (rows as f32 * ch) as u32);
            Self::emit(&self.imp().handlers.resize, &(cols, rows, px.0, px.1));
        }
    }

    /// Viewport cell under a widget point, clamped to the grid.
    fn cell_at(&self, x: f64, y: f64) -> (u16, u16) {
        let (cw, ch, pad) = self.cell_size();
        let (cols, rows) = self.with_state(|s| (s.term.cols(), s.term.rows()));
        let col = (((x as f32 - pad) / cw).floor().max(0.0) as u16).min(cols.saturating_sub(1));
        let row = (((y as f32 - pad) / ch).floor().max(0.0) as u16).min(rows.saturating_sub(1));
        (col, row)
    }

    /// Draw the view into `snap` now, outside the frame clock (benchmarks,
    /// thumbnails).
    pub fn render_into(&self, snap: &gtk::Snapshot) {
        self.draw(snap);
    }

    fn draw(&self, snap: &gtk::Snapshot) {
        let started = Instant::now();
        let w = self.width() as f32;
        let h = self.height() as f32;
        let ctx = self.pango_context();
        let focused = self.imp().focused.get();
        let blink_on = self.imp().blink_on.get();
        let read_only = self.is_read_only();
        let mut st = self.imp().state.borrow_mut();
        let Some(s) = st.as_mut() else { return };
        if s.metrics.is_none() {
            s.metrics = Some(Metrics::new(&ctx, &s.style.font, self.scale_factor() as f64));
        }
        if s.held_until.is_some_and(|until| Instant::now() >= until) {
            s.held_until = None;
        }
        if s.held_until.is_none() {
            let dirty = s.renderer.update(&s.term).unwrap_or(Dirty::All);
            if dirty != Dirty::None
                && let Some((_, search)) = s.search.as_mut()
            {
                s.matches = search.viewport_matches(&s.term);
            }
        }
        let m = s.metrics.clone().unwrap();
        let caret = s.renderer.frame.cursor.at;
        if self.imp().told_caret.replace(caret) != caret {
            let view = self.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(v) = view.upgrade() {
                    v.update_caret_position();
                }
            });
        }
        let frame = &s.renderer.frame;
        let bg = frame.background;
        snap.append_color(&rgba(bg, 1.0), &rect(0.0, 0.0, w, h));

        let pad = s.style.padding;
        if s.cache.len() != frame.rows.len() {
            s.cache.resize(frame.rows.len(), None);
        }
        let mut built = 0;
        for (y, row) in frame.rows.iter().enumerate() {
            let fresh = matches!(&s.cache[y], Some((v, sel, _)) if *v == row.version && *sel == row.selection);
            if !fresh {
                s.cache[y] = draw::row_node(&ctx, &m, row, bg, &s.style.selection, frame.cols)
                    .map(|n| (row.version, row.selection, n))
                    .or(Some((row.version, row.selection, gsk::ContainerNode::new(&[]).upcast())));
                built += 1;
            }
            if let Some((_, _, node)) = &s.cache[y] {
                snap.save();
                snap.translate(&graphene::Point::new(pad, pad + y as f32 * m.cell_h));
                snap.append_node(node);
                snap.restore();
            }
        }

        let selected = s.search.as_ref().and_then(|_| s.matches.first().copied());
        for mt in &s.matches {
            let c = if Some(*mt) == selected { &s.style.search_current } else { &s.style.search };
            let x = pad + mt.start as f32 * m.cell_w;
            let wd = (mt.end + 1 - mt.start.min(mt.end + 1)) as f32 * m.cell_w;
            snap.append_color(c, &rect(x, pad + mt.row as f32 * m.cell_h, wd, m.cell_h));
        }

        let cur = frame.cursor;
        if let Some((cx, cy)) = cur.at
            && cur.visible
            && !read_only
            && (blink_on || !cur.blinking || !focused)
        {
            let color = rgba(cur.color.unwrap_or(frame.foreground), 1.0);
            let x = pad + cx as f32 * m.cell_w;
            let y = pad + cy as f32 * m.cell_h;
            let cw = if cur.wide { m.cell_w * 2.0 } else { m.cell_w };
            let thick = (m.cell_w / 8.0).max(1.0).round();
            if !s.preedit.is_empty() {
                let layout = pango::Layout::new(&ctx);
                layout.set_font_description(Some(&m.regular));
                layout.set_text(&s.preedit);
                let (lw, _) = layout.pixel_size();
                snap.append_color(&rgba(bg, 1.0), &rect(x, y, lw as f32, m.cell_h));
                snap.save();
                snap.translate(&graphene::Point::new(x, y));
                snap.append_layout(&layout, &rgba(frame.foreground, 1.0));
                snap.restore();
                snap.append_color(&color, &rect(x, y + m.cell_h - thick, lw as f32, thick));
            } else if !focused {
                for r in [
                    rect(x, y, cw, thick),
                    rect(x, y + m.cell_h - thick, cw, thick),
                    rect(x, y, thick, m.cell_h),
                    rect(x + cw - thick, y, thick, m.cell_h),
                ] {
                    snap.append_color(&color, &r);
                }
            } else {
                match cur.style {
                    nyx_term::CursorStyle::Bar => snap.append_color(&color, &rect(x, y, thick, m.cell_h)),
                    nyx_term::CursorStyle::Underline => snap.append_color(&color, &rect(x, y + m.cell_h - thick, cw, thick)),
                    nyx_term::CursorStyle::BlockHollow => {
                        for r in [
                            rect(x, y, cw, thick),
                            rect(x, y + m.cell_h - thick, cw, thick),
                            rect(x, y, thick, m.cell_h),
                            rect(x + cw - thick, y, thick, m.cell_h),
                        ] {
                            snap.append_color(&color, &r);
                        }
                    }
                    nyx_term::CursorStyle::Block => {
                        snap.append_color(&color, &rect(x, y, cw, m.cell_h));
                        // Redraw the character under the cursor in the background colour.
                        if let Some(run) = frame
                            .rows
                            .get(cy as usize)
                            .and_then(|r| r.runs.iter().find(|run| run.col <= cx && cx < run.col + run.cols))
                        {
                            let offset = (cx - run.col) as usize;
                            let text: String = if run.text.is_ascii() {
                                run.text.chars().nth(offset).map(String::from).unwrap_or_default()
                            } else {
                                run.text.clone()
                            };
                            if !text.trim().is_empty() {
                                let layout = pango::Layout::new(&ctx);
                                layout.set_font_description(Some(&m.regular));
                                layout.set_text(&text);
                                let base = layout.baseline() as f32 / pango::SCALE as f32;
                                snap.save();
                                snap.translate(&graphene::Point::new(x, y + m.baseline - base));
                                snap.append_layout(&layout, &rgba(bg, 1.0));
                                snap.restore();
                            }
                        }
                    }
                }
            }
        }

        // A thin scroll indicator while looking back through history.
        let sb = s.term.scrollbar();
        if sb.total > sb.len && sb.offset + sb.len < sb.total {
            let track = h - 2.0 * pad;
            let len = (sb.len as f32 / sb.total as f32 * track).max(16.0);
            let top = pad + sb.offset as f32 / (sb.total - sb.len).max(1) as f32 * (track - len);
            snap.append_color(&rgba(frame.foreground, 0.35), &rect(w - 5.0, top, 3.0, len));
        }

        let took = started.elapsed();
        s.stats.frames += 1;
        s.stats.rows_built += built;
        s.stats.draw_time += took;
        s.stats.max_draw = s.stats.max_draw.max(took);
        let stats = s.stats;
        drop(st);
        Self::emit(&self.imp().handlers.frame, &stats);
    }

    /// Copy the selection to the clipboard. Returns false with nothing selected.
    pub fn copy_selection(&self) -> bool {
        let text = self.with_state(|s| s.term.selection().and_then(|sel| s.term.text(Some(&sel)).ok()));
        match text {
            Some(t) if !t.is_empty() => {
                self.clipboard().set_text(&t);
                true
            }
            _ => false,
        }
    }

    pub fn has_selection(&self) -> bool {
        self.with_state(|s| s.term.selection().is_some())
    }

    pub fn clear_selection(&self) {
        self.with_state(|s| s.term.clear_selection());
        self.queue_draw();
    }

    pub fn select_all(&self) {
        self.with_state(|s| s.term.select_all());
        self.queue_draw();
    }

    /// Paste text into the program (bracketed when it asked for that).
    /// Text that could run commands is only pasted with `allow_unsafe`.
    pub fn paste_text(&self, text: &str, allow_unsafe: bool) -> bool {
        let bytes = self.with_state(|s| s.term.paste(text, allow_unsafe));
        match bytes {
            Ok(b) => {
                self.with_state(|s| s.term.scroll(ScrollTo::Bottom));
                self.send(&b);
                true
            }
            Err(_) => false,
        }
    }

    fn paste_from(&self, clip: gdk::Clipboard) {
        if self.is_read_only() {
            return;
        }
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(Some(text)) = clip.read_text_future().await else { return };
            let Some(view) = weak.upgrade() else { return };
            // Multi-line pastes are allowed here; the view sends them
            // bracketed when the program supports it.
            if !view.paste_text(&text, false) {
                view.paste_text(&text, true);
            }
        });
    }

    pub fn paste_clipboard(&self) {
        self.paste_from(self.clipboard());
    }

    /// Search the screen and scrollback; returns the number of matches.
    pub fn find(&self, needle: &str) -> usize {
        let n = self.with_state(|s| {
            if needle.is_empty() {
                s.search = None;
                s.matches.clear();
                return 0;
            }
            if s.search.as_ref().is_none_or(|(old, _)| old != needle) {
                let Ok(mut search) = Search::new(&s.term) else { return 0 };
                search.set_needle(needle);
                search.run();
                s.search = Some((needle.to_string(), search));
            }
            let (_, search) = s.search.as_mut().unwrap();
            let total = search.total();
            if total > 0 && search.selected().is_none() {
                search.step(true);
            }
            s.matches = search.viewport_matches(&s.term);
            s.cache.clear();
            total
        });
        self.queue_draw();
        n
    }

    /// Step to the next older (or newer) match.
    pub fn find_step(&self, older: bool) {
        self.with_state(|s| {
            if let Some((_, search)) = s.search.as_mut() {
                search.step(older);
                s.matches = search.viewport_matches(&s.term);
            }
        });
        self.queue_draw();
    }

    pub fn scroll_to(&self, to: ScrollTo) {
        self.with_state(|s| s.term.scroll(to));
        self.queue_draw();
    }

    fn install_controllers(&self) {
        let im = gtk::IMMulticontext::new();
        im.set_use_preedit(true);
        let weak = self.downgrade();
        im.connect_commit(move |_, text| {
            let Some(view) = weak.upgrade() else { return };
            if view.imp().in_key.get() {
                *view.imp().committed.borrow_mut() = Some(text.to_string());
            } else {
                // Committed outside a key press (an input method finishing).
                view.send(text.as_bytes());
            }
        });
        let weak = self.downgrade();
        im.connect_preedit_changed(move |im| {
            let Some(view) = weak.upgrade() else { return };
            let (text, _, _) = im.preedit_string();
            view.with_state(|s| s.preedit = text.to_string());
            view.queue_draw();
        });
        *self.imp().im.borrow_mut() = Some(im);

        let keys = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        keys.connect_key_pressed(move |c, keyval, keycode, state| {
            let Some(view) = weak.upgrade() else { return glib::Propagation::Proceed };
            if view.is_read_only() {
                return view.read_only_key(keyval, state);
            }
            view.key(c, keyval, keycode, state, KeyAction::Press)
        });
        let weak = self.downgrade();
        keys.connect_key_released(move |c, keyval, keycode, state| {
            if let Some(view) = weak.upgrade().filter(|v| !v.is_read_only()) {
                view.key(c, keyval, keycode, state, KeyAction::Release);
            }
        });
        self.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_enter(move |_| {
            let Some(view) = weak.upgrade() else { return };
            view.imp().focused.set(true);
            if let Some(im) = view.imp().im.borrow().as_ref() {
                im.set_client_widget(Some(&view));
                im.focus_in();
            }
            view.start_blink();
            view.queue_draw();
        });
        let weak = self.downgrade();
        focus.connect_leave(move |_| {
            let Some(view) = weak.upgrade() else { return };
            view.imp().focused.set(false);
            if let Some(im) = view.imp().im.borrow().as_ref() {
                im.focus_out();
            }
            view.queue_draw();
        });
        self.add_controller(focus);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = self.downgrade();
        click.connect_pressed(move |g, n, x, y| {
            if let Some(view) = weak.upgrade() {
                view.pointer_press(g, n, x, y);
            }
        });
        let weak = self.downgrade();
        click.connect_released(move |g, _, x, y| {
            if let Some(view) = weak.upgrade() {
                view.pointer_release(g, x, y);
            }
        });
        self.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_motion(move |c, x, y| {
            if let Some(view) = weak.upgrade() {
                view.pointer_motion(c.current_event_state(), x, y);
            }
        });
        self.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = self.downgrade();
        scroll.connect_scroll(move |c, _dx, dy| {
            let Some(view) = weak.upgrade() else { return glib::Propagation::Proceed };
            view.scroll(c, dy);
            glib::Propagation::Stop
        });
        self.add_controller(scroll);
    }

    fn start_blink(&self) {
        let weak = self.downgrade();
        glib::timeout_add_local(Duration::from_millis(600), move || {
            let Some(view) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !view.imp().focused.get() {
                view.imp().blink_on.set(true);
                return glib::ControlFlow::Break;
            }
            let on = !view.imp().blink_on.get();
            view.imp().blink_on.set(on);
            let blinking = view.with_state(|s| s.renderer.frame.cursor.blinking);
            if blinking {
                view.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    fn key(
        &self,
        c: &gtk::EventControllerKey,
        keyval: gdk::Key,
        keycode: u32,
        state: gdk::ModifierType,
        action: KeyAction,
    ) -> glib::Propagation {
        use gdk::ModifierType as M;
        let ctrl_shift = state.contains(M::CONTROL_MASK | M::SHIFT_MASK);
        let ctrl_only = state.contains(M::CONTROL_MASK) && !state.intersects(M::SHIFT_MASK | M::ALT_MASK | M::SUPER_MASK);
        if action == KeyAction::Press {
            let lower = keyval.to_lower();
            if ctrl_shift && lower == gdk::Key::c {
                self.copy_selection();
                return glib::Propagation::Stop;
            }
            // Ctrl+C copies while text is selected; with nothing selected it interrupts as usual.
            if ctrl_only && lower == gdk::Key::c && self.copy_selection() {
                self.clear_selection();
                return glib::Propagation::Stop;
            }
            if (ctrl_shift && lower == gdk::Key::v) || (state.contains(M::SHIFT_MASK) && keyval == gdk::Key::Insert) {
                self.paste_clipboard();
                return glib::Propagation::Stop;
            }
            // Ctrl+V pastes text. With only an image on the clipboard the key goes
            // through, so programs that read images themselves (Claude Code) get it.
            if ctrl_only && lower == gdk::Key::v && has_text(&self.clipboard().formats()) {
                self.paste_clipboard();
                return glib::Propagation::Stop;
            }
            if ctrl_only && lower == gdk::Key::v && !self.imp().handlers.image_paste.borrow().is_empty() {
                Self::emit(&self.imp().handlers.image_paste, &());
                return glib::Propagation::Stop;
            }
            if state.contains(M::SHIFT_MASK) && !state.contains(M::CONTROL_MASK) {
                let page = self.with_state(|s| s.term.rows() as isize - 1).max(1);
                let to = match keyval {
                    gdk::Key::Page_Up => Some(ScrollTo::Delta(-page)),
                    gdk::Key::Page_Down => Some(ScrollTo::Delta(page)),
                    gdk::Key::Home => Some(ScrollTo::Top),
                    gdk::Key::End => Some(ScrollTo::Bottom),
                    _ => None,
                };
                if let Some(to) = to {
                    self.scroll_to(to);
                    return glib::Propagation::Stop;
                }
            }
        }

        let event = c.current_event();
        let imp = self.imp();
        let mut text = String::new();
        let mut composing = false;
        if action == KeyAction::Press
            && let (Some(ev), Some(im)) = (event.as_ref(), imp.im.borrow().as_ref())
        {
            imp.in_key.set(true);
            let filtered = im.filter_keypress(ev);
            imp.in_key.set(false);
            let committed = imp.committed.borrow_mut().take();
            let preedit = self.with_state(|s| !s.preedit.is_empty());
            match committed {
                Some(t) => text = t,
                None if filtered => {
                    // The input method kept the key (composing).
                    return glib::Propagation::Stop;
                }
                None => composing = preedit,
            }
        }

        let key = Key::from_evdev(keycode.saturating_sub(8));
        let mut mods = 0u16;
        if state.contains(M::SHIFT_MASK) {
            mods |= Mods::SHIFT;
        }
        if state.contains(M::CONTROL_MASK) {
            mods |= Mods::CTRL;
        }
        if state.contains(M::ALT_MASK) {
            mods |= Mods::ALT;
        }
        if state.contains(M::SUPER_MASK) {
            mods |= Mods::SUPER;
        }
        if state.contains(M::LOCK_MASK) {
            mods |= Mods::CAPS_LOCK;
        }
        let consumed = event
            .as_ref()
            .and_then(|e| e.downcast_ref::<gdk::KeyEvent>().map(|k| k.consumed_modifiers()))
            .map(|m| {
                let mut c = 0u16;
                if m.contains(M::SHIFT_MASK) {
                    c |= Mods::SHIFT;
                }
                if m.contains(M::ALT_MASK) {
                    c |= Mods::ALT;
                }
                c
            })
            .unwrap_or(0);
        let unshifted =
            self.display().translate_key(keycode, M::empty(), 0).and_then(|(k, ..)| k.to_unicode()).filter(|c| !c.is_control());
        if text.is_empty()
            && let Some(ch) = keyval.to_unicode()
            && !ch.is_control()
            && action == KeyAction::Press
            && mods & (Mods::CTRL | Mods::ALT) == 0
        {
            text = ch.to_string();
        }
        let bytes = self.with_state(|s| {
            let b = s.keys.encode(&s.term, key, action, Mods(mods), Mods(consumed), &text, unshifted, composing);
            if !b.is_empty() && !key.is_modifier() {
                s.term.scroll(ScrollTo::Bottom);
                s.term.clear_selection();
            }
            b
        });
        if bytes.is_empty() {
            return if action == KeyAction::Release { glib::Propagation::Proceed } else { glib::Propagation::Stop };
        }
        self.imp().blink_on.set(true);
        self.send(&bytes);
        glib::Propagation::Stop
    }

    /// Keys a view that only shows a screen still answers: copy and select all.
    fn read_only_key(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::ModifierType as M;
        if !state.contains(M::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        match keyval.to_lower() {
            gdk::Key::c if self.copy_selection() => glib::Propagation::Stop,
            gdk::Key::a if state.contains(M::SHIFT_MASK) => {
                self.select_all();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn mouse_mods(state: gdk::ModifierType) -> Mods {
        use gdk::ModifierType as M;
        let mut m = 0;
        if state.contains(M::SHIFT_MASK) {
            m |= Mods::SHIFT;
        }
        if state.contains(M::CONTROL_MASK) {
            m |= Mods::CTRL;
        }
        if state.contains(M::ALT_MASK) {
            m |= Mods::ALT;
        }
        Mods(m)
    }

    fn report_mouse(&self, action: MouseAction, button: MouseButton, state: gdk::ModifierType, x: f64, y: f64) -> bool {
        if self.is_read_only() {
            return false;
        }
        let (cw, ch, pad) = self.cell_size();
        let any = self.imp().buttons.get() != 0;
        let bytes = self.with_state(|s| {
            if !s.term.mouse_tracking() {
                return None;
            }
            let (cols, rows) = (s.term.cols(), s.term.rows());
            Some(s.mouse.encode(
                &s.term,
                action,
                button,
                Self::mouse_mods(state),
                any,
                ((x as f32 - pad).max(0.0), (y as f32 - pad).max(0.0)),
                ((cols as f32 * cw) as u32, (rows as f32 * ch) as u32),
                (cw.round() as u32, ch.round() as u32),
            ))
        });
        match bytes {
            Some(b) => {
                self.send(&b);
                true
            }
            None => false,
        }
    }

    fn gdk_button(b: u32) -> MouseButton {
        match b {
            1 => MouseButton::Left,
            2 => MouseButton::Middle,
            3 => MouseButton::Right,
            _ => MouseButton::None,
        }
    }

    fn pointer_press(&self, g: &gtk::GestureClick, _n: i32, x: f64, y: f64) {
        let button = g.current_button();
        let state = g.current_event_state();
        self.grab_focus();
        self.imp().buttons.set(self.imp().buttons.get() | (1 << button));
        // Clicks and drags select text, even in programs that ask for the
        // mouse; Shift sends them to the program instead.
        let pass = state.contains(gdk::ModifierType::SHIFT_MASK);
        self.imp().passing.set(pass);
        if pass && self.report_mouse(MouseAction::Press, Self::gdk_button(button), state, x, y) {
            return;
        }
        let (col, row) = self.cell_at(x, y);
        match button {
            1 if state.contains(gdk::ModifierType::CONTROL_MASK) => self.open_link(col, row),
            1 => {
                let time_ns = g.current_event_time() as u64 * 1_000_000;
                self.with_state(|s| s.term.select_gesture(SelectGesture::Press { col, row, x, y, time_ns }));
                self.imp().selecting.set(true);
                self.queue_draw();
            }
            2 => self.paste_from(self.primary_clipboard()),
            _ => {}
        }
    }

    fn pointer_release(&self, g: &gtk::GestureClick, x: f64, y: f64) {
        let button = g.current_button();
        let state = g.current_event_state();
        self.imp().buttons.set(self.imp().buttons.get() & !(1 << button));
        if self.imp().passing.replace(false) && self.report_mouse(MouseAction::Release, Self::gdk_button(button), state, x, y) {
            return;
        }
        if button == 1 && self.imp().selecting.replace(false) {
            let (col, row) = self.cell_at(x, y);
            let text = self.with_state(|s| {
                s.term.select_gesture(SelectGesture::Release { col, row });
                s.term.selection().and_then(|sel| s.term.text(Some(&sel)).ok())
            });
            if let Some(t) = text.filter(|t| !t.is_empty()) {
                self.primary_clipboard().set_text(&t);
            }
            self.queue_draw();
        }
    }

    fn pointer_motion(&self, state: gdk::ModifierType, x: f64, y: f64) {
        let held = self.imp().buttons.get();
        // Hover goes to programs that track the mouse; drags only when the press did.
        if (held == 0 || self.imp().passing.get()) && self.with_state(|s| s.term.mouse_tracking()) {
            let button =
                [1u32, 2, 3].into_iter().find(|b| held & (1 << b) != 0).map(Self::gdk_button).unwrap_or(MouseButton::None);
            if self.report_mouse(MouseAction::Motion, button, state, x, y) {
                return;
            }
        }
        if self.imp().selecting.get() {
            let (col, row) = self.cell_at(x, y);
            let (cw, _, pad) = self.cell_size();
            let height = self.height().max(1) as u32;
            self.with_state(|s| {
                s.term.select_gesture(SelectGesture::Drag { col, row, x: x - pad as f64, y, cell_w: cw.round() as u32, height })
            });
            self.queue_draw();
        }
    }

    fn scroll(&self, c: &gtk::EventControllerScroll, dy: f64) {
        let (_, ch, _) = self.cell_size();
        let lines = if c.unit() == gdk::ScrollUnit::Wheel { dy * 3.0 } else { dy / ch as f64 };
        let total = self.imp().scroll_rest.get() + lines;
        let whole = total.trunc();
        self.imp().scroll_rest.set(total - whole);
        let n = whole as isize;
        if n == 0 {
            return;
        }
        let state = c.current_event_state();
        let (x, y) = (self.width() as f64 / 2.0, self.height() as f64 / 2.0);
        let button = if n < 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        let tracked = self.with_state(|s| s.term.mouse_tracking());
        if tracked && !self.is_read_only() && !state.contains(gdk::ModifierType::SHIFT_MASK) {
            for _ in 0..n.unsigned_abs().min(10) {
                self.report_mouse(MouseAction::Press, button, state, x, y);
            }
            return;
        }
        let alt = self.with_state(|s| s.term.alternate_screen());
        if alt && !self.is_read_only() {
            // Full-screen programs without mouse reporting get arrow keys.
            let arrow: &[u8] = if n < 0 { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..n.unsigned_abs().min(10) {
                self.send(arrow);
            }
            return;
        }
        self.scroll_to(ScrollTo::Delta(n));
    }

    fn open_link(&self, col: u16, row: u16) {
        let uri =
            self.with_state(|s| s.term.hyperlink_at(col, row).or_else(|| links::url_at(&s.term.row_text(row), col as usize)));
        let Some(uri) = uri.filter(|u| links::allowed(u)) else { return };
        let window = self.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(&uri).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
    }
}

/// The clipboard holds text: as a local string, or offered by another program.
fn has_text(f: &gdk::ContentFormats) -> bool {
    f.contains_type(glib::Type::STRING)
        || ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"].iter().any(|m| f.contain_mime_type(m))
}
