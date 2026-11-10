//! Timed terminal workloads. Each prints one JSON object and quits.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use nyx_term::ScrollTo;
use nyx_term_gtk::{Stats, TerminalView};
use serde_json::{Value, json};

use crate::session::Session;

pub fn run(app: &gtk::Application, name: &str) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Terminal lab bench")
        .default_width(1280)
        .default_height(800)
        .build();
    window.set_size_request(1280, 800);
    window.present();
    let app = app.clone();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        // Let the window map and settle at the output's size.
        glib::timeout_future(Duration::from_millis(500)).await;
        let result = match name.as_str() {
            "throughput" => throughput(&window, false, 100).await,
            "mirror" => throughput(&window, true, 100).await,
            "scrollback" => scrollback(&window).await,
            "latency" => latency(&window, false).await,
            "latency-load" => latency(&window, true).await,
            "multi1" => multi(&window, 1).await,
            "multi4" => multi(&window, 4).await,
            "multi8" => multi(&window, 8).await,
            "resize" => resize(&window).await,
            other => {
                if let Some(path) = other.strip_prefix("shot=") {
                    shot(&window, PathBuf::from(path)).await
                } else if let Some(path) = other.strip_prefix("replay=") {
                    replay(&window, PathBuf::from(path)).await
                } else {
                    json!({"error": format!("unknown benchmark {other}")})
                }
            }
        };
        let mut out = json!({ "bench": name, "libghostty_vt": nyx_term::version() });
        if let (Value::Object(o), Value::Object(r)) = (&mut out, result) {
            o.extend(r);
        }
        o_merge(&mut out, process_usage());
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        app.quit();
    });
}

fn o_merge(a: &mut Value, b: Value) {
    if let (Value::Object(a), Value::Object(b)) = (a, b) {
        a.extend(b);
    }
}

/// CPU time and peak memory of this process and its finished children.
fn process_usage() -> Value {
    let usage = |who| {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(who, &mut u) };
        let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
        (t(u.ru_utime), t(u.ru_stime))
    };
    let (su, ss) = usage(libc::RUSAGE_SELF);
    let (cu, cs) = usage(libc::RUSAGE_CHILDREN);
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        status.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok())
    };
    json!({
        "cpu_user_s": round(su), "cpu_sys_s": round(ss),
        "children_cpu_s": round(cu + cs),
        "rss_mb": kb("VmRSS:").map(|k| k / 1024),
        "peak_rss_mb": kb("VmHWM:").map(|k| k / 1024),
    })
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn ms(d: Duration) -> f64 {
    round(d.as_secs_f64() * 1000.0)
}

/// Generated output resembling a build log: colours, paths, a little CJK.
fn workload(mb: usize) -> PathBuf {
    let path = std::env::temp_dir().join(format!("nyx-term-lab-{mb}mb.txt"));
    if path.metadata().map(|m| m.len() as usize >= mb << 20).unwrap_or(false) {
        return path;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    let mut n = 0u64;
    let mut written = 0usize;
    while written < mb << 20 {
        let c = 31 + (n % 6);
        let line = format!(
            "{n:08} \x1b[{c}mINFO\x1b[0m \x1b[1mCompiling\x1b[0m crate-{} v1.{}.{} (/home/ken/Projects/nyx-agents/crates/x) 構築 ✓ {:.1}ms\r\n",
            n % 97,
            n % 13,
            n % 7,
            (n % 1000) as f64 / 10.0
        );
        f.write_all(line.as_bytes()).unwrap();
        written += line.len();
        n += 1;
    }
    f.flush().unwrap();
    path
}

/// Draws frames on demand. A headless output only paints while captured, so
/// the benchmarks draw every terminal view themselves, at its place in the
/// window, and render the node with the window's own GSK renderer, as the
/// frame clock would at 60 Hz.
struct Pump {
    window: gtk::ApplicationWindow,
    views: Vec<TerminalView>,
    renderer: Option<gsk::Renderer>,
    owns_renderer: bool,
    gpu: RefCell<Vec<Duration>>,
    snapshot: RefCell<Vec<Duration>>,
}

/// The renderer frames go through. NYX_BENCH_RENDERER=vulkan|gl renders
/// offscreen on the GPU without a surface (needed on Xvfb, which cannot
/// present GPU frames); otherwise the window's own renderer.
fn bench_renderer(window: &gtk::ApplicationWindow) -> (Option<gsk::Renderer>, bool) {
    let r: gsk::Renderer = match std::env::var("NYX_BENCH_RENDERER").as_deref() {
        Ok("vulkan") => gsk::VulkanRenderer::new().upcast(),
        Ok("gl") => gsk::GLRenderer::new().upcast(),
        _ => return (window.native().and_then(|n| n.renderer()), false),
    };
    match r.realize_for_display(&gtk::prelude::WidgetExt::display(window)) {
        Ok(()) => (Some(r), true),
        Err(e) => {
            eprintln!("bench renderer: {e}");
            (window.native().and_then(|n| n.renderer()), false)
        }
    }
}

impl Pump {
    fn new(window: &gtk::ApplicationWindow, views: &[&TerminalView]) -> Self {
        let (renderer, owns_renderer) = bench_renderer(window);
        Self {
            window: window.clone(),
            views: views.iter().map(|v| (*v).clone()).collect(),
            renderer,
            owns_renderer,
            gpu: RefCell::new(Vec::new()),
            snapshot: RefCell::new(Vec::new()),
        }
    }

    fn renderer_name(&self) -> String {
        self.renderer.as_ref().map(|r| r.type_().name().to_string()).unwrap_or_default()
    }

    /// Render one frame now.
    fn draw(&self) {
        // Apply pending size requests first, as the frame clock's layout phase would.
        let (w, h) = (self.window.width(), self.window.height());
        if let Some(child) = self.window.child() {
            child.allocate(w, h, -1, None);
        }
        let t = Instant::now();
        let snap = gtk::Snapshot::new();
        for v in &self.views {
            let Some(p) = v.compute_point(&self.window, &gtk::graphene::Point::new(0.0, 0.0)) else { continue };
            snap.save();
            snap.translate(&p);
            v.render_into(&snap);
            snap.restore();
        }
        let node = snap.to_node();
        self.snapshot.borrow_mut().push(t.elapsed());
        if let (Some(node), Some(r)) = (node, self.renderer.as_ref()) {
            let t = Instant::now();
            let _ = r.render_texture(&node, None);
            self.gpu.borrow_mut().push(t.elapsed());
        }
    }

    /// Wait one 60 Hz interval, letting output arrive, then render.
    async fn frame(&self) {
        glib::timeout_future(Duration::from_micros(16_667)).await;
        self.draw();
    }

    fn json(&self) -> Value {
        let mut out = json!({ "renderer": self.renderer_name() });
        o_merge(&mut out, percentiles("snapshot", &mut self.snapshot.borrow_mut()));
        o_merge(&mut out, percentiles("gpu_render", &mut self.gpu.borrow_mut()));
        out
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        if self.owns_renderer
            && let Some(r) = &self.renderer
        {
            r.unrealize();
        }
    }
}

fn stats_json(s: &Stats) -> Value {
    json!({
        "frames": s.frames,
        "rows_built": s.rows_built,
        "avg_draw_ms": if s.frames > 0 { ms(s.draw_time / s.frames as u32) } else { 0.0 },
        "max_draw_ms": ms(s.max_draw),
        "parse_ms": ms(s.parse_time),
        "bytes_fed": s.bytes_fed,
    })
}

async fn settle(pump: &Pump) {
    for _ in 0..10 {
        pump.frame().await;
    }
}

async fn throughput(window: &gtk::ApplicationWindow, mirror: bool, mb: usize) -> Value {
    let file = workload(mb);
    let size = file.metadata().unwrap().len();
    let view = TerminalView::new(crate::style(), false);
    let viewer = mirror.then(|| TerminalView::new(crate::style(), true));
    match &viewer {
        Some(v) => {
            let paned = gtk::Paned::new(gtk::Orientation::Vertical);
            paned.set_start_child(Some(&view));
            paned.set_end_child(Some(v));
            paned.set_position(window.height() * 2 / 3);
            window.set_child(Some(&paned));
        }
        None => window.set_child(Some(&view)),
    }
    let mut shown = vec![&view];
    shown.extend(viewer.as_ref());
    let pump = Pump::new(window, &shown);
    settle(&pump).await;
    let done = Rc::new(RefCell::new(false));
    let start = Instant::now();
    let path = file.to_string_lossy().to_string();
    let s = Session::spawn(&["cat", &path], &view).unwrap();
    if let Some(v) = &viewer {
        s.add_mirror(v);
    }
    let d = done.clone();
    s.on_exit(move || *d.borrow_mut() = true);
    while !*done.borrow() {
        pump.frame().await;
    }
    pump.draw();
    let took = start.elapsed();
    let st = view.stats();
    let mut out = json!({
        "workload_mb": round(size as f64 / 1048576.0),
        "grid": view.grid(),
        "seconds": round(took.as_secs_f64()),
        "mb_per_s": round(size as f64 / 1048576.0 / took.as_secs_f64()),
        "terminal_memory_mb": round(view.terminal(|t| t.memory_usage()) as f64 / 1048576.0),
    });
    o_merge(&mut out, stats_json(&st));
    o_merge(&mut out, pump.json());
    if let Some(v) = &viewer {
        o_merge(&mut out, json!({ "mirror": stats_json(&v.stats()) }));
    }
    drop(s);
    out
}

async fn scrollback(window: &gtk::ApplicationWindow) -> Value {
    let view = TerminalView::new(crate::style(), false);
    window.set_child(Some(&view));
    let pump = Pump::new(window, &[&view]);
    settle(&pump).await;
    let text = std::fs::read(workload(5)).unwrap();
    view.feed(&text);
    pump.draw();
    let sb = view.terminal(|t| t.scrollbar());
    let rows = view.grid().1 as isize;
    let mut times = Vec::new();
    for up in [true, false] {
        for _ in 0..200 {
            glib::timeout_future(Duration::from_millis(4)).await;
            let t = Instant::now();
            view.scroll_to(ScrollTo::Delta(if up { -rows } else { rows }));
            pump.draw();
            times.push(t.elapsed());
        }
    }
    let mut out = json!({ "scrollback_rows": sb.total, "page_flips": times.len() });
    o_merge(&mut out, percentiles("page_to_frame", &mut times));
    o_merge(&mut out, stats_json(&view.stats()));
    out
}

fn percentiles(name: &str, v: &mut [Duration]) -> Value {
    v.sort();
    let p = |q: f64| v.get(((v.len() as f64 - 1.0) * q).round() as usize).copied().map(ms);
    json!({ name: { "n": v.len(), "p50_ms": p(0.5), "p95_ms": p(0.95), "p99_ms": p(0.99), "max_ms": v.last().copied().map(ms) } })
}

/// Keypress to a rendered frame showing its echo. The program is `cat` on a
/// tty, so the echo comes from the kernel line discipline as with a shell.
/// The frame is rendered as soon as the echo arrives; on a real display the
/// compositor adds up to one refresh interval on top.
async fn latency(window: &gtk::ApplicationWindow, load: bool) -> Value {
    let view = TerminalView::new(crate::style(), false);
    let busy = load.then(|| TerminalView::new(crate::style(), false));
    match &busy {
        Some(b) => {
            let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
            paned.set_start_child(Some(&view));
            paned.set_end_child(Some(b));
            paned.set_position(window.width() / 2);
            window.set_child(Some(&paned));
        }
        None => window.set_child(Some(&view)),
    }
    let mut shown = vec![&view];
    shown.extend(busy.as_ref());
    let pump = Rc::new(Pump::new(window, &shown));
    settle(&pump).await;
    let s = Session::spawn(&["cat"], &view).unwrap();
    let _load_session = busy.as_ref().map(|b| {
        let path = workload(100).to_string_lossy().to_string();
        Session::spawn(&["sh", "-c", &format!("while :; do cat '{path}'; done")], b).unwrap()
    });
    // Under load the other terminal keeps rendering at 60 Hz.
    let ticking = Rc::new(std::cell::Cell::new(load));
    if load {
        let (p, t) = (pump.clone(), ticking.clone());
        glib::spawn_future_local(async move {
            while t.get() {
                p.frame().await;
            }
        });
    }
    settle(&pump).await;
    let echoed = Rc::new(RefCell::new(None::<Instant>));
    let e = echoed.clone();
    s.on_output(move |_| {
        e.borrow_mut().get_or_insert_with(Instant::now);
    });
    let mut to_echo = Vec::new();
    let mut to_frame = Vec::new();
    for i in 0..200 {
        *echoed.borrow_mut() = None;
        let t0 = Instant::now();
        s.write(if i % 50 == 49 { b"\r" } else { b"x" });
        loop {
            glib::timeout_future(Duration::ZERO).await;
            if let Some(t1) = *echoed.borrow() {
                pump.draw();
                to_echo.push(t1 - t0);
                to_frame.push(t0.elapsed());
                break;
            }
            if t0.elapsed() > Duration::from_secs(2) {
                break;
            }
        }
        glib::timeout_future(Duration::from_millis(8)).await;
    }
    ticking.set(false);
    let mut out = json!({ "under_load": load });
    o_merge(&mut out, percentiles("key_to_echo", &mut to_echo));
    o_merge(&mut out, percentiles("key_to_frame", &mut to_frame));
    out
}

async fn multi(window: &gtk::ApplicationWindow, n: usize) -> Value {
    let grid = gtk::Grid::new();
    grid.set_row_homogeneous(true);
    grid.set_column_homogeneous(true);
    let cols = if n <= 1 {
        1
    } else if n <= 4 {
        2
    } else {
        4
    };
    let views: Vec<TerminalView> = (0..n).map(|_| TerminalView::new(crate::style(), false)).collect();
    for (i, v) in views.iter().enumerate() {
        grid.attach(v, (i % cols) as i32, (i / cols) as i32, 1, 1);
    }
    window.set_child(Some(&grid));
    let pump = Pump::new(window, &views.iter().collect::<Vec<_>>());
    settle(&pump).await;
    let path = workload(20).to_string_lossy().to_string();
    let left = Rc::new(RefCell::new(n));
    let start = Instant::now();
    let sessions: Vec<Session> = views
        .iter()
        .map(|v| {
            let s = Session::spawn(&["cat", &path], v).unwrap();
            let l = left.clone();
            s.on_exit(move || *l.borrow_mut() -= 1);
            s
        })
        .collect();
    while *left.borrow() > 0 {
        pump.frame().await;
    }
    pump.draw();
    let took = start.elapsed();
    let total: u64 = views.iter().map(|v| v.stats().bytes_fed).sum();
    let worst = views.iter().map(|v| v.stats().max_draw).max().unwrap_or_default();
    let mem: usize = views.iter().map(|v| v.terminal(|t| t.memory_usage())).sum();
    drop(sessions);
    let mut out = json!({
        "terminals": n,
        "grid_each": views[0].grid(),
        "seconds": round(took.as_secs_f64()),
        "total_mb": round(total as f64 / 1048576.0),
        "mb_per_s": round(total as f64 / 1048576.0 / took.as_secs_f64()),
        "worst_view_draw_ms": ms(worst),
        "terminal_memory_mb": round(mem as f64 / 1048576.0),
    });
    o_merge(&mut out, pump.json());
    out
}

async fn resize(window: &gtk::ApplicationWindow) -> Value {
    let view = TerminalView::new(crate::style(), false);
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.append(&view);
    view.set_vexpand(false);
    view.set_hexpand(false);
    view.set_halign(gtk::Align::Start);
    view.set_valign(gtk::Align::Start);
    window.set_child(Some(&holder));
    let pump = Pump::new(window, &[&view]);
    settle(&pump).await;
    let path = workload(5).to_string_lossy().to_string();
    let s = Session::spawn(&["sh", "-c", &format!("cat '{path}'; exec cat")], &view).unwrap();
    for _ in 0..30 {
        pump.frame().await;
    }
    let mut times = Vec::new();
    let mut grids = Vec::new();
    for i in 0..60 {
        let (w, h) = if i % 2 == 0 { (1100, 700) } else { (640, 420) };
        let t = Instant::now();
        view.set_size_request(w, h);
        // Layout runs before drawing; the view resizes its terminal there.
        pump.draw();
        times.push(t.elapsed());
        grids.push(view.grid());
        glib::timeout_future(Duration::from_millis(16)).await;
    }
    drop(s);
    grids.dedup();
    let mut out = json!({ "resizes": times.len(), "grids_seen": grids.len() });
    o_merge(&mut out, percentiles("resize_to_frame", &mut times));
    o_merge(&mut out, stats_json(&view.stats()));
    out
}

/// Feed a recorded pty capture as fast as it parses and draws.
async fn replay(window: &gtk::ApplicationWindow, path: PathBuf) -> Value {
    let Ok(data) = std::fs::read(&path) else { return json!({"error": format!("cannot read {}", path.display())}) };
    let view = TerminalView::new(crate::style(), false);
    window.set_child(Some(&view));
    let pump = Pump::new(window, &[&view]);
    settle(&pump).await;
    let start = Instant::now();
    let mut last = Instant::now();
    for chunk in data.chunks(16 * 1024) {
        view.feed(chunk);
        if last.elapsed() >= Duration::from_micros(16_667) {
            pump.draw();
            last = Instant::now();
        }
    }
    pump.draw();
    let took = start.elapsed();
    let mut out = json!({
        "capture_mb": round(data.len() as f64 / 1048576.0),
        "seconds": round(took.as_secs_f64()),
    });
    o_merge(&mut out, stats_json(&view.stats()));
    o_merge(&mut out, pump.json());
    out
}

/// Render a test pattern (styles, colours, wide text, a selection, a search
/// hit and the cursor) to a PNG through the GPU renderer.
async fn shot(window: &gtk::ApplicationWindow, path: PathBuf) -> Value {
    let view = TerminalView::new(crate::style(), false);
    let viewer = TerminalView::new(crate::style(), true);
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.set_start_child(Some(&view));
    paned.set_end_child(Some(&viewer));
    paned.set_position(window.height() * 3 / 5);
    window.set_child(Some(&paned));
    let pump = Pump::new(window, &[&view, &viewer]);
    settle(&pump).await;
    let mut p = String::new();
    p.push_str("\x1b]2;pattern\x07\x1b[1mBold\x1b[0m \x1b[3mItalic\x1b[0m \x1b[2mFaint\x1b[0m \x1b[4mUnder\x1b[0m \x1b[4:3mCurly\x1b[0m \x1b[21mDouble\x1b[0m \x1b[9mStrike\x1b[0m \x1b[7mInverse\x1b[0m \x1b[53mOver\x1b[0m\r\n");
    for i in 0..16 {
        p.push_str(&format!("\x1b[48;5;{i}m  \x1b[0m"));
    }
    p.push_str("  ");
    for i in 0..16 {
        p.push_str(&format!("\x1b[38;5;{i}m██\x1b[0m"));
    }
    p.push_str("\r\n");
    for i in 0..72 {
        let v = i * 255 / 71;
        p.push_str(&format!("\x1b[48;2;{v};{};{}m \x1b[0m", 255 - v, (v * 2) % 255));
    }
    p.push_str("\r\n日本語の文字 한국어 中文  emoji: ✅ 🚀 🦀  math: ∑ ∫ ≠ ≤  e\u{301} a\u{308}\r\n");
    p.push_str("┌──────┬──────┐ ╭──────╮ ▁▂▃▄▅▆▇█  \u{e0b0}\u{e0b2} \u{f115} \u{f121} \u{e7a8}\r\n");
    p.push_str("│ box  │ grid │ │ round│ ░▒▓█\r\n└──────┴──────┘ ╰──────╯\r\n");
    p.push_str("\x1b]8;;https://ghostty.org\x1b\\a hyperlink\x1b]8;;\x1b\\ and https://example.com/plain\r\n");
    p.push_str("\x1b[32m❯\x1b[0m cargo build --release\r\n   \x1b[1;32mCompiling\x1b[0m nyx-term v0.1.0\r\n    \x1b[1;32mFinished\x1b[0m `release` profile in 1m 57s\r\n\x1b[32m❯\x1b[0m find me here\r\n");
    p.push_str("\x1b[32m❯\x1b[0m ");
    view.feed(p.as_bytes());
    view.find("find me");
    view.terminal(|t| {
        t.select_gesture(nyx_term::SelectGesture::Press { col: 0, row: 3, x: 1.0, y: 1.0, time_ns: 0 });
    });
    let (cw, _, _) = view.cell_size();
    view.terminal(|t| {
        t.select_gesture(nyx_term::SelectGesture::Drag {
            col: 6,
            row: 3,
            x: 6.6 * cw as f64,
            y: 1.0,
            cell_w: cw as u32,
            height: 400,
        });
    });
    viewer.terminal(|t| {
        let _ = t.resize(100, 8, 1, 1);
    });
    viewer.feed(b"\x1b[2mread-only viewer\x1b[0m \x1b[36m[subagent: explore]\x1b[0m\r\n\x1b[33m\xe2\x97\x8f\x1b[0m Reading crates/nyx-term/src/render.rs\r\n\x1b[32m\xe2\x9c\x93\x1b[0m Found 3 matches\r\n");
    view.grab_focus();
    pump.draw();
    let snap = gtk::Snapshot::new();
    for v in [&view, &viewer] {
        let Some(pt) = v.compute_point(window, &gtk::graphene::Point::new(0.0, 0.0)) else { continue };
        snap.save();
        snap.translate(&pt);
        v.render_into(&snap);
        snap.restore();
    }
    let Some(node) = snap.to_node() else { return json!({"error": "nothing drawn"}) };
    let Some(r) = pump.renderer.as_ref() else { return json!({"error": "no renderer"}) };
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32);
    let tex = r.render_texture(&node, Some(&bounds));
    match tex.save_to_png(&path) {
        Ok(()) => json!({ "png": path, "renderer": pump.renderer_name(), "grid": view.grid() }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}
