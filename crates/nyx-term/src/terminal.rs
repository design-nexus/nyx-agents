use std::os::raw::c_void;
use std::ptr;

use crate::ffi::*;
use crate::{Colors, Result};

/// How a terminal is set up.
#[derive(Clone, Debug)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    /// Scrollback budget in bytes (whole ~400 KB pages in practice).
    pub scrollback_bytes: usize,
    pub colors: Colors,
    /// Reported for XTVERSION (CSI > q).
    pub xtversion: &'static str,
    /// Largest OSC 52 clipboard write passed on; bigger ones are refused.
    pub clipboard_limit: usize,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback_bytes: 10 * 1024 * 1024,
            colors: Colors::catppuccin_mocha(),
            xtversion: "nyx-agents",
            clipboard_limit: 1024 * 1024,
        }
    }
}

/// What a program said it is doing, via OSC 7501.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    /// Remove this record and everything beneath it.
    Clear,
}

/// What a blocked program needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusKind {
    None,
    Permission,
    Question,
    Auth,
}

/// One OSC 7501 report. Text is untrusted program output with control
/// characters already removed by the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReport {
    pub state: ProgramState,
    pub kind: StatusKind,
    /// 0..=100, or `None` when indeterminate.
    pub progress: Option<u8>,
    pub id: String,
    pub app: String,
    pub title: String,
    pub message: String,
}

/// A clipboard write the program asked for (OSC 52 and friends), text only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardWrite {
    /// True for the primary selection rather than the clipboard.
    pub primary: bool,
    pub text: String,
}

/// What the program asked of the host since the last [`Terminal::take_effects`].
#[derive(Clone, Debug, Default)]
pub struct Effects {
    /// Bytes to write back to the program's input (query replies).
    pub reply: Vec<u8>,
    pub title_changed: bool,
    pub pwd_changed: bool,
    pub bells: u32,
    pub status: Vec<StatusReport>,
    /// The last render-hold change: `Some(true)` while synchronized output is on.
    pub render_hold: Option<bool>,
    pub clipboard: Vec<ClipboardWrite>,
    /// OSC 9 / OSC 777 desktop notifications: (title, body).
    pub notifications: Vec<(String, String)>,
    /// A new shell prompt started (OSC 133), which ends working/blocked status.
    pub prompt_started: bool,
    /// The program did a full reset.
    pub reset: bool,
}

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.reply.is_empty()
            && !self.title_changed
            && !self.pwd_changed
            && self.bells == 0
            && self.status.is_empty()
            && self.render_hold.is_none()
            && self.clipboard.is_empty()
            && self.notifications.is_empty()
            && !self.prompt_started
            && !self.reset
    }
}

struct Host {
    effects: Effects,
    xtversion: &'static str,
    clipboard_limit: usize,
}

/// Where to move the viewport.
#[derive(Clone, Copy, Debug)]
pub enum ScrollTo {
    Top,
    Bottom,
    /// Rows; up is negative.
    Delta(isize),
    /// Absolute row offset from the top of scrollback.
    Row(usize),
    /// Jump this many shell prompts (OSC 133); up is negative.
    Prompt(isize),
}

/// The viewport's place in the scrollback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// A selection on the terminal's screen. Only valid until the next write.
#[derive(Clone, Copy)]
pub struct Selection(pub(crate) GhosttySelection);

/// A terminal: screen, scrollback, modes and parser state.
pub struct Terminal {
    raw: GhosttyTerminal,
    host: Box<Host>,
    gesture: GhosttySelectionGesture,
    gesture_event: GhosttySelectionGestureEvent,
}

// libghostty-vt creates no threads and keeps no thread-local state, so a
// terminal can move between threads as long as one thread uses it at a time.
unsafe impl Send for Terminal {}

impl Terminal {
    pub fn new(opts: &TerminalOptions) -> Result<Self> {
        let mut raw: GhosttyTerminal = ptr::null_mut();
        check(unsafe { ghostty_terminal_new(ptr::null(), &mut raw, opts.cols.max(1), opts.rows.max(1)) })?;
        let mut term = Self::wrap(raw, opts)?;
        term.set_colors(&opts.colors);
        let bytes = opts.scrollback_bytes;
        check(unsafe { ghostty_terminal_set(term.raw, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, ptr(&bytes)) })?;
        Ok(term)
    }

    /// Restore a terminal from [`Terminal::snapshot`] bytes.
    pub fn from_snapshot(bytes: &[u8], opts: &TerminalOptions) -> Result<Self> {
        let mut dec: GhosttySnapshotDecoder = ptr::null_mut();
        check(unsafe { ghostty_snapshot_decoder_new_buf(ptr::null(), &mut dec, bytes.as_ptr(), bytes.len()) })?;
        let mut raw: GhosttyTerminal = ptr::null_mut();
        let r = unsafe { ghostty_snapshot_decoder_decode(dec, &mut raw) };
        unsafe { ghostty_snapshot_decoder_free(dec) };
        check(r)?;
        Self::wrap(raw, opts)
    }

    fn wrap(raw: GhosttyTerminal, opts: &TerminalOptions) -> Result<Self> {
        let mut host =
            Box::new(Host { effects: Effects::default(), xtversion: opts.xtversion, clipboard_limit: opts.clipboard_limit });
        let ud: *mut c_void = (&mut *host as *mut Host).cast();
        let mut gesture: GhosttySelectionGesture = ptr::null_mut();
        let mut gesture_event: GhosttySelectionGestureEvent = ptr::null_mut();
        unsafe {
            check(ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_USERDATA, ud))?;
            let set = |opt, f: *const c_void| check(ghostty_terminal_set(raw, opt, f));
            set(GHOSTTY_TERMINAL_OPT_WRITE_PTY, on_write_pty as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_BELL, on_bell as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_XTVERSION, on_xtversion as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, on_title as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_PWD_CHANGED, on_pwd as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_RENDER_HOLD, on_render_hold as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS, on_program_status as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, on_clipboard_write as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION, on_notification as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT, on_semantic_prompt as *const c_void)?;
            set(GHOSTTY_TERMINAL_OPT_RESET, on_reset as *const c_void)?;
            let name = b"xterm-256color";
            check(ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_TERMINFO_NAME, ptr(&gstr(name))))?;
            check(ghostty_selection_gesture_new(ptr::null(), &mut gesture))?;
            check(ghostty_selection_gesture_event_new(
                ptr::null(),
                &mut gesture_event,
                GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS,
            ))?;
        }
        Ok(Self { raw, host, gesture, gesture_event })
    }

    pub(crate) fn raw(&self) -> GhosttyTerminal {
        self.raw
    }

    /// Feed program output.
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) }
    }

    /// Everything the program asked for since the last call.
    pub fn take_effects(&mut self) -> Effects {
        std::mem::take(&mut self.host.effects)
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) -> Result<()> {
        check(unsafe { ghostty_terminal_resize(self.raw, cols.max(1), rows.max(1), cell_w.max(1), cell_h.max(1)) })
    }

    pub fn reset(&mut self) {
        unsafe { ghostty_terminal_reset(self.raw) }
    }

    pub fn set_colors(&mut self, c: &Colors) {
        let fg = c_rgb(c.foreground);
        let bg = c_rgb(c.background);
        let palette: Vec<GhosttyColorRgb> = c.palette().iter().map(|&p| c_rgb(p)).collect();
        unsafe {
            ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, ptr(&fg));
            ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, ptr(&bg));
            match c.cursor {
                Some(cur) => ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr(&c_rgb(cur))),
                None => ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, ptr::null()),
            };
            ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette.as_ptr().cast());
        }
    }

    pub fn scroll(&mut self, to: ScrollTo) {
        let mut s = GhosttyTerminalScrollViewport::default();
        match to {
            ScrollTo::Top => s.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP,
            ScrollTo::Bottom => s.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
            ScrollTo::Delta(d) => {
                s.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA;
                s.value.delta = d;
            }
            ScrollTo::Row(r) => {
                s.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW;
                s.value.row = r;
            }
            ScrollTo::Prompt(d) => {
                s.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA_PROMPT;
                s.value.delta_prompt = d;
            }
        }
        unsafe { ghostty_terminal_scroll_viewport(self.raw, s) };
    }

    fn get<T: Default>(&self, data: GhosttyTerminalData) -> Option<T> {
        let mut v = T::default();
        let r = unsafe { ghostty_terminal_get(self.raw, data, mut_ptr(&mut v)) };
        (r == GHOSTTY_SUCCESS).then_some(v)
    }

    fn get_string(&self, data: GhosttyTerminalData) -> String {
        self.get::<GhosttyString>(data).map(|s| String::from_utf8_lossy(unsafe { bytes(s) }).into_owned()).unwrap_or_default()
    }

    pub fn cols(&self) -> u16 {
        self.get(GHOSTTY_TERMINAL_DATA_COLS).unwrap_or(0)
    }

    pub fn rows(&self) -> u16 {
        self.get(GHOSTTY_TERMINAL_DATA_ROWS).unwrap_or(0)
    }

    pub fn title(&self) -> String {
        self.get_string(GHOSTTY_TERMINAL_DATA_TITLE)
    }

    /// The working directory the shell last reported (OSC 7, raw).
    pub fn pwd(&self) -> String {
        self.get_string(GHOSTTY_TERMINAL_DATA_PWD)
    }

    pub fn scrollbar(&self) -> Scrollbar {
        self.get::<GhosttyTerminalScrollbar>(GHOSTTY_TERMINAL_DATA_SCROLLBAR)
            .map(|s| Scrollbar { total: s.total, offset: s.offset, len: s.len })
            .unwrap_or_default()
    }

    /// True on the alternate screen (full-screen programs).
    pub fn alternate_screen(&self) -> bool {
        self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN) == Some(GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
    }

    /// True while the program wants mouse events.
    pub fn mouse_tracking(&self) -> bool {
        self.get::<bool>(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING).unwrap_or(false)
    }

    /// True while the viewport is at the bottom (following output).
    pub fn viewport_at_bottom(&self) -> bool {
        self.get::<bool>(GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE).unwrap_or(true)
    }

    /// Resident bytes of screen and scrollback pages, both screens, plus
    /// stored images. Walks every page; don't call it per write.
    pub fn memory_usage(&self) -> usize {
        let mut m = sized::<GhosttyTerminalMemoryUsage>(|m, s| m.size = s);
        if unsafe { ghostty_terminal_get(self.raw, GHOSTTY_TERMINAL_DATA_MEMORY_USAGE, mut_ptr(&mut m)) } != GHOSTTY_SUCCESS {
            return 0;
        }
        (m.primary_resident_bytes + m.alternate_resident_bytes + m.primary_image_bytes + m.alternate_image_bytes) as usize
    }

    /// The complete state (screen, scrollback, modes) as an opaque snapshot.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        let mut p: *mut u8 = ptr::null_mut();
        let mut len = 0usize;
        check(unsafe { ghostty_snapshot_encode_alloc(self.raw, ptr::null(), &mut p, &mut len) })?;
        Ok(unsafe { take_alloc(p, len) })
    }

    /// Track unfinished escape sequences so a snapshot taken mid-sequence
    /// still restores. Needed when output is snapshotted at arbitrary points.
    pub fn track_continuation(&mut self, max_bytes: usize) {
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES, ptr(&max_bytes)) };
    }

    /// Plain text of the whole screen and scrollback, or of a selection.
    pub fn text(&self, selection: Option<&Selection>) -> Result<String> {
        self.format(GHOSTTY_FORMATTER_FORMAT_PLAIN, selection).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The screen as VT sequences that redraw it, with styles.
    pub fn vt(&self) -> Result<Vec<u8>> {
        self.format(GHOSTTY_FORMATTER_FORMAT_VT, None)
    }

    fn format(&self, emit: GhosttyFormatterFormat, selection: Option<&Selection>) -> Result<Vec<u8>> {
        let mut f: GhosttyFormatter = ptr::null_mut();
        check(unsafe { ghostty_formatter_terminal_new(ptr::null(), &mut f, self.raw) })?;
        let res = (|| unsafe {
            check(ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_EMIT, ptr(&emit)))?;
            let unwrap = true;
            check(ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_UNWRAP, ptr(&unwrap)))?;
            let trim = true;
            check(ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_TRIM, ptr(&trim)))?;
            if let Some(sel) = selection {
                check(ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_SELECTION, ptr(&sel.0)))?;
            }
            let mut p: *mut u8 = ptr::null_mut();
            let mut len = 0usize;
            check(ghostty_formatter_format_alloc(f, ptr::null(), &mut p, &mut len))?;
            Ok(take_alloc(p, len))
        })();
        unsafe { ghostty_formatter_free(f) };
        res
    }

    fn grid_ref(&self, col: u16, row: u32) -> Option<GhosttyGridRef> {
        let mut p = GhosttyPoint { tag: GHOSTTY_POINT_TAG_VIEWPORT, ..Default::default() };
        p.value.coordinate = GhosttyPointCoordinate { x: col, y: row };
        let mut r = GhosttyGridRef::default();
        (unsafe { ghostty_terminal_grid_ref(self.raw, p, &mut r) } == GHOSTTY_SUCCESS).then_some(r)
    }

    /// The OSC 8 hyperlink at a viewport cell.
    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let r = self.grid_ref(col, row as u32)?;
        let mut len = 0usize;
        let res = unsafe { ghostty_grid_ref_hyperlink_uri(&r, ptr::null_mut(), 0, &mut len) };
        if len == 0 || (res != GHOSTTY_OUT_OF_SPACE && res != GHOSTTY_SUCCESS) {
            return None;
        }
        let mut buf = vec![0u8; len];
        check(unsafe { ghostty_grid_ref_hyperlink_uri(&r, buf.as_mut_ptr(), buf.len(), &mut len) }).ok()?;
        buf.truncate(len);
        String::from_utf8(buf).ok()
    }

    /// The text of the viewport row, for finding plain URLs and paths.
    pub fn row_text(&self, row: u16) -> String {
        let mut out = String::new();
        for col in 0..self.cols() {
            let Some(r) = self.grid_ref(col, row as u32) else { break };
            let mut cps = [0u32; 16];
            let mut n = 0usize;
            if unsafe { ghostty_grid_ref_graphemes(&r, cps.as_mut_ptr(), cps.len(), &mut n) } != GHOSTTY_SUCCESS || n == 0 {
                out.push(' ');
                continue;
            }
            out.extend(cps[..n.min(16)].iter().filter_map(|&c| char::from_u32(c)));
        }
        out
    }

    /// Feed a pointer event to the selection gesture (click counts, word
    /// and line selection, drags). Returns the selection it produces, which
    /// is installed on the terminal.
    pub fn select_gesture(&mut self, g: SelectGesture) -> Option<Selection> {
        let (ty, col, row, x, y, time_ns, cell_w, height) = match g {
            SelectGesture::Press { col, row, x, y, time_ns } => {
                (GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS, col, row, x, y, time_ns, 1, 1)
            }
            SelectGesture::Drag { col, row, x, y, cell_w, height } => {
                (GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG, col, row, x, y, 0, cell_w, height)
            }
            SelectGesture::Release { col, row } => (GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE, col, row, 0.0, 0.0, 0, 1, 1),
        };
        let ev = self.gesture_event;
        let grid = self.grid_ref(col, row as u32);
        let geometry = GhosttySelectionGestureGeometry {
            columns: self.cols() as u32,
            cell_width: cell_w.max(1),
            padding_left: 0,
            screen_height: height.max(1),
        };
        let pos = GhosttySurfacePosition { x, y };
        let mut out = GhosttySelection::default();
        let res = unsafe {
            ghostty_selection_gesture_event_free(ev);
            let mut ev: GhosttySelectionGestureEvent = ptr::null_mut();
            ghostty_selection_gesture_event_new(ptr::null(), &mut ev, ty);
            self.gesture_event = ev;
            if let Some(gr) = grid.as_ref() {
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, ptr(gr));
            } else if ty != GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE {
                return None;
            }
            if ty != GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE {
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, ptr(&pos));
            }
            if ty == GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG {
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, ptr(&geometry));
            }
            if ty == GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS && time_ns > 0 {
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS, ptr(&time_ns));
                let interval: u64 = 500_000_000;
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS, ptr(&interval));
                let distance: f64 = 8.0;
                ghostty_selection_gesture_event_set(ev, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE, ptr(&distance));
            }
            ghostty_selection_gesture_event(self.gesture, self.raw, ev, &mut out)
        };
        if res != GHOSTTY_SUCCESS {
            if ty == GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS {
                unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null()) };
            }
            return None;
        }
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr(&out)) };
        Some(Selection(out))
    }

    /// The terminal's current selection.
    pub fn selection(&self) -> Option<Selection> {
        self.get::<GhosttySelection>(GHOSTTY_TERMINAL_DATA_SELECTION).filter(|s| !s.start.node.is_null()).map(Selection)
    }

    pub fn clear_selection(&mut self) {
        unsafe {
            ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null());
            ghostty_selection_gesture_reset(self.gesture, self.raw);
        }
    }

    pub fn select_all(&mut self) -> Option<Selection> {
        let mut s = GhosttySelection::default();
        check(unsafe { ghostty_terminal_select_all(self.raw, &mut s) }).ok()?;
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr(&s)) };
        Some(Selection(s))
    }

    pub fn set_selection(&mut self, s: &Selection) {
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr(&s.0)) };
    }

    /// Paste text, bracketed when the program asked for it. Returns the bytes
    /// to send to the program. Text that could run commands is refused
    /// unless `allow_unsafe`; `Err(true)` means it was refused for that.
    pub fn paste(&mut self, text: &str, allow_unsafe: bool) -> std::result::Result<Vec<u8>, bool> {
        struct Src<'a>(&'a [u8]);
        unsafe extern "C" fn read(userdata: *mut c_void, _mime: GhosttyString, writer: GhosttyWriter) -> bool {
            let src = unsafe { &*(userdata as *const Src) };
            match writer.write {
                Some(w) => unsafe { w(writer.userdata, src.0.as_ptr(), src.0.len()) },
                None => false,
            }
        }
        let mime = gstr(b"text/plain;charset=utf-8");
        let src = Src(text.as_bytes());
        let paste = sized::<GhosttyPaste>(|p, size| {
            p.size = size;
            p.location = GHOSTTY_CLIPBOARD_LOCATION_STANDARD;
            p.source = GHOSTTY_PASTE_SOURCE_CLIPBOARD;
            p.mimes = &mime;
            p.mimes_len = 1;
            p.reader = GhosttyMimeReader { read: Some(read), userdata: &src as *const Src as *mut c_void };
            p.allow_unsafe = allow_unsafe;
        });
        let mut pasted = false;
        let before = self.host.effects.reply.len();
        let r = unsafe { ghostty_terminal_paste(self.raw, &paste, &mut pasted) };
        if r == GHOSTTY_REJECTED {
            return Err(true);
        }
        if r != GHOSTTY_SUCCESS {
            return Err(false);
        }
        Ok(self.host.effects.reply.split_off(before))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ghostty_selection_gesture_event_free(self.gesture_event);
            ghostty_selection_gesture_free(self.gesture, self.raw);
            ghostty_terminal_free(self.raw);
        }
    }
}

/// A pointer event for [`Terminal::select_gesture`], in viewport cells and
/// surface pixels.
#[derive(Clone, Copy, Debug)]
pub enum SelectGesture {
    Press {
        col: u16,
        row: u16,
        x: f64,
        y: f64,
        time_ns: u64,
    },
    /// `cell_w` and `height` are the cell width and surface height in pixels.
    Drag {
        col: u16,
        row: u16,
        x: f64,
        y: f64,
        cell_w: u32,
        height: u32,
    },
    Release {
        col: u16,
        row: u16,
    },
}

unsafe fn host<'a>(ud: *mut c_void) -> &'a mut Host {
    unsafe { &mut *(ud as *mut Host) }
}

fn text(s: GhosttyString) -> String {
    String::from_utf8_lossy(unsafe { bytes(s) }).into_owned()
}

unsafe extern "C" fn on_write_pty(_t: GhosttyTerminal, ud: *mut c_void, data: *const u8, len: usize) {
    if !data.is_null() {
        unsafe { host(ud).effects.reply.extend_from_slice(std::slice::from_raw_parts(data, len)) };
    }
}

unsafe extern "C" fn on_bell(_t: GhosttyTerminal, ud: *mut c_void) {
    unsafe { host(ud).effects.bells += 1 };
}

unsafe extern "C" fn on_xtversion(_t: GhosttyTerminal, ud: *mut c_void) -> GhosttyString {
    gstr(unsafe { host(ud) }.xtversion.as_bytes())
}

unsafe extern "C" fn on_title(_t: GhosttyTerminal, ud: *mut c_void) {
    unsafe { host(ud).effects.title_changed = true };
}

unsafe extern "C" fn on_pwd(_t: GhosttyTerminal, ud: *mut c_void) {
    unsafe { host(ud).effects.pwd_changed = true };
}

unsafe extern "C" fn on_render_hold(_t: GhosttyTerminal, ud: *mut c_void, held: bool) {
    unsafe { host(ud).effects.render_hold = Some(held) };
}

unsafe extern "C" fn on_reset(_t: GhosttyTerminal, ud: *mut c_void) {
    unsafe { host(ud).effects.reset = true };
}

unsafe extern "C" fn on_program_status(_t: GhosttyTerminal, ud: *mut c_void, r: *const GhosttyTerminalProgramStatus) {
    let r = unsafe { &*r };
    let state = match r.state {
        GHOSTTY_PROGRAM_STATUS_STATE_IDLE => ProgramState::Idle,
        GHOSTTY_PROGRAM_STATUS_STATE_WORKING => ProgramState::Working,
        GHOSTTY_PROGRAM_STATUS_STATE_DONE => ProgramState::Done,
        GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED => ProgramState::Blocked,
        GHOSTTY_PROGRAM_STATUS_STATE_ERROR => ProgramState::Error,
        GHOSTTY_PROGRAM_STATUS_STATE_CLEAR => ProgramState::Clear,
        _ => return,
    };
    let kind = match r.kind {
        GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION => StatusKind::Permission,
        GHOSTTY_PROGRAM_STATUS_KIND_QUESTION => StatusKind::Question,
        GHOSTTY_PROGRAM_STATUS_KIND_AUTH => StatusKind::Auth,
        _ => StatusKind::None,
    };
    let report = StatusReport {
        state,
        kind,
        progress: (0..=100).contains(&r.progress).then_some(r.progress as u8),
        id: text(r.id),
        app: text(r.app),
        title: text(r.title),
        message: text(r.message),
    };
    unsafe { host(ud).effects.status.push(report) };
}

unsafe extern "C" fn on_clipboard_write(_t: GhosttyTerminal, ud: *mut c_void, w: *const GhosttyClipboardWrite) {
    let w = unsafe { &*w };
    let h = unsafe { host(ud) };
    let contents = if w.contents.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(w.contents, w.contents_len) } };
    let plain = contents.iter().find(|c| unsafe { bytes(c.mime) }.starts_with(b"text/plain"));
    let mut result = GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    if let Some(c) = plain {
        let data = unsafe { bytes(c.data) };
        if data.len() > h.clipboard_limit {
            result = GHOSTTY_CLIPBOARD_WRITE_RESULT_DENIED;
        } else if let Ok(s) = std::str::from_utf8(data) {
            h.effects
                .clipboard
                .push(ClipboardWrite { primary: w.location != GHOSTTY_CLIPBOARD_LOCATION_STANDARD, text: s.to_string() });
            result = GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
        } else {
            result = GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
        }
    }
    let reply = sized::<GhosttyClipboardWriteReply>(|r, size| {
        r.size = size;
        r.result = result;
    });
    if let Some(f) = w.reply {
        unsafe { f(w, &reply) };
    }
}

unsafe extern "C" fn on_notification(_t: GhosttyTerminal, ud: *mut c_void, n: *const GhosttyTerminalDesktopNotification) {
    let n = unsafe { &*n };
    unsafe { host(ud).effects.notifications.push((text(n.title), text(n.body))) };
}

unsafe extern "C" fn on_semantic_prompt(_t: GhosttyTerminal, ud: *mut c_void, e: *const GhosttyTerminalSemanticPrompt) {
    if unsafe { (*e).kind } == GHOSTTY_SEMANTIC_PROMPT_PROMPT_START {
        unsafe { host(ud).effects.prompt_started = true };
    }
}
