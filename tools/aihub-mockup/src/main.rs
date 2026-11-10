//! A mockup of the AI Hub as a Nyx Bar plugin, drawn by the bar's own view
//! renderer (nyx-plugin-view), theme and stylesheet. MOCK_TAB picks which
//! popover view (views/<tab>.json) is open.

mod live;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use nyx_plugin_protocol::Node;
use nyx_plugin_view as view;

fn cc_mode() -> bool {
    std::env::var("MOCK_TAB").is_ok_and(|t| t.starts_with("cc"))
}

fn theme() {
    nyx_theme::use_design_2();
    nyx_theme::use_shell_scope();
    let (window_class, css) = if cc_mode() {
        ("nyx-control-center", concat!(include_str!("cc.css"), "\n", include_str!("mock.css")))
    } else {
        ("nyx-bar", concat!(include_str!("bar.css"), "\n", include_str!("mock.css")))
    };
    nyx_theme::install(nyx_theme::Style {
        window_class,
        base_skip: &[],
        css,
        no_glow_css: "",
        extra_colors: None,
        prefs: || {
            let suite = nyx_theme::read_suite();
            nyx_theme::ThemePrefs {
                mode: suite.as_ref().map(|s| s.mode.clone()).unwrap_or(nyx_theme::ThemeMode::Omarchy),
                theme: suite.map(|s| s.theme).unwrap_or_default(),
                glow: false,
                reduce_motion: false,
            }
        },
        set_theme: None,
    });
}

const VIEWS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/views/");

fn value(name: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(format!("{VIEWS}{name}.json")).unwrap_or_else(|e| panic!("{name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}.json: {e}"))
}

fn node(v: serde_json::Value, name: &str) -> Node {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{name}.json: {e}"))
}

fn load(name: &str) -> Node {
    node(value(name), name)
}

/// A view's body, with what the plugin would patch for it: `sessions-open` is the
/// Sessions tab after a click on its first row.
fn body(view: &str) -> serde_json::Value {
    match view {
        "sessions-open" => {
            let mut v = value("sessions");
            open_row(&mut v, "s3");
            v
        }
        _ => value(view),
    }
}

fn open_row(v: &mut serde_json::Value, id: &str) {
    if v["id"].as_str() == Some(id) {
        v["class"] = serde_json::json!(["picked"]);
    }
    if v["id"].as_str() == Some(&format!("{id}-actions")) {
        v["visible"] = true.into();
    }
    if let Some(children) = v["children"].as_array_mut() {
        children.iter_mut().for_each(|c| open_row(c, id));
    }
}

/// The tab a view belongs to.
fn tab_of(view: &str) -> &str {
    if view.starts_with("agent-") {
        "usage"
    } else if view.starts_with("sessions") {
        "sessions"
    } else {
        view
    }
}

/// The popover for a view: the header and tab strip from agents.json, then the view's
/// body. Without the header for the Control Center, which has its own.
fn popover_value(view: &str, header: bool) -> serde_json::Value {
    let mut root = value("agents");
    let tab = tab_of(view);
    let children = root["children"].as_array_mut().unwrap();
    for b in children[1]["children"].as_array_mut().unwrap() {
        let on = b["action"].as_str() == Some(&format!("tab:{tab}"));
        b["class"] = if on { serde_json::json!(["tab", "on"]) } else { serde_json::json!(["tab"]) };
    }
    if view != "agents" {
        children.truncate(2);
        children.extend(body(view)["children"].as_array().unwrap().iter().cloned());
    }
    let page: Vec<serde_json::Value> = children.drain(2..).collect();
    children.push(serde_json::json!({
        "node": "scroll", "max_height": if header { 520 } else { 600 },
        "child": { "node": "box", "orientation": "vertical", "align": "fill", "spacing": 8, "children": page },
    }));
    if !header {
        children.remove(0);
        root["class"] = serde_json::json!([]);
    }
    root
}

fn popover(view: &str) -> Node {
    // A view saved from the real plugin, drawn as it is.
    if view == "file" {
        let path = std::env::var("MOCK_FILE").expect("MOCK_FILE");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        return node(v.get("view").cloned().unwrap_or(v), view);
    }
    node(popover_value(view, true), view)
}

fn inner_scroll(w: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut c = w.first_child();
    while let Some(child) = c {
        if let Some(s) = child.downcast_ref::<gtk::ScrolledWindow>() {
            return Some(s.clone());
        }
        if let Some(s) = inner_scroll(&child) {
            return Some(s);
        }
        c = child.next_sibling();
    }
    None
}

/// Scrolls the page under the header and tabs to MOCK_SCROLL (0-1) once it has its
/// size (and, live, once the plugin has drawn it).
fn scroll_to(outer: &gtk::ScrolledWindow) {
    let Some(f) = std::env::var("MOCK_SCROLL").ok().and_then(|v| v.parse::<f64>().ok()).filter(|f| *f > 0.0) else { return };
    let outer = outer.clone();
    let at = std::env::var("MOCK_SCROLL_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(700);
    glib::timeout_add_local_once(std::time::Duration::from_millis(at), move || {
        let s = outer.child().and_then(|c| inner_scroll(&c)).unwrap_or(outer.clone());
        let a = s.vadjustment();
        a.set_value(f * (a.upper() - a.page_size()));
    });
}

/// The plugin's folder: the real plugin's with MOCK_PLUGIN_DIR, else the mockup's views.
fn plugin_dir() -> String {
    std::env::var("MOCK_PLUGIN_DIR").map(|d| format!("{}/", d.trim_end_matches('/'))).unwrap_or_else(|_| VIEWS.to_string())
}

fn live_mode() -> bool {
    std::env::var_os("MOCK_PLUGIN").is_some()
}

thread_local! {
    /// In live mode: where the plugin's item, compact tile and popover go.
    static SLOTS: RefCell<(Option<gtk::Box>, Option<gtk::Box>, Option<gtk::Box>)> = RefCell::default();
}

fn slot() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.set_vexpand(true);
    b
}

fn plugin_css() {
    let dir = plugin_dir();
    let css = std::fs::read_to_string(format!("{dir}style.css")).unwrap();
    let p = gtk::CssProvider::new();
    p.load_from_string(&view::css::scope(&css, "nyx.ai-hub", 4));
    let display = gtk::gdk::Display::default().unwrap();
    gtk::style_context_add_provider_for_display(&display, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    gtk::IconTheme::for_display(&display).add_search_path(format!("{dir}icons"));
}

fn ctx(max_height: Option<i32>) -> view::Ctx {
    view::Ctx {
        plugin: "nyx.ai-hub".into(),
        plugin_dir: plugin_dir().into(),
        storage_dir: "/tmp".into(),
        trusted: true,
        max_height,
        on_event: Rc::new(|e| eprintln!("event {e:?}")),
    }
}

fn item(node: &Node, plugin_class: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("bar-item");
    b.add_css_class(plugin_class);
    b.set_valign(gtk::Align::Fill);
    b.append(&view::render(node, &ctx(Some(32))).root);
    b
}

fn label_item(text: &str, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("bar-item");
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    b.append(&l);
    b
}

fn icon_item(icon: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("bar-item");
    b.append(&gtk::Image::from_icon_name(icon));
    b
}

const CELL: (i32, i32) = (87, 48);
const GAP: i32 = 8;

fn px(cols: i32, rows: i32) -> (i32, i32) {
    (cols * CELL.0 + (cols - 1) * GAP, rows * CELL.1 + (rows - 1) * GAP)
}

fn cc_label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(1);
    l.set_hexpand(true);
    l
}

fn cc_frame(cols: i32, rows: i32) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("cc-tile");
    b.add_css_class(&format!("cc-{cols}x{rows}"));
    let (w, h) = px(cols, rows);
    b.set_size_request(w, h);
    b
}

fn cc_chip(icon: &str, on: bool) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("cc-chip");
    if on {
        b.add_css_class("on");
    }
    b.set_valign(gtk::Align::Center);
    b.set_halign(gtk::Align::Start);
    let i = gtk::Image::from_icon_name(icon);
    i.set_pixel_size(17);
    i.set_hexpand(true);
    b.append(&i);
    b.set_hexpand(false);
    b
}

/// The Control Center's native split tile (plugin_tiles::template, 2×1 with a detail view).
pub fn cc_split(icon: &str, name: &str, status: &str, on: bool, plugin: Option<&str>) -> gtk::Box {
    let root = cc_frame(2, 1);
    root.add_css_class("cc-split");
    if on {
        root.add_css_class("on");
    }
    if let Some(p) = plugin {
        root.add_css_class("cc-plugin");
        root.add_css_class(&view::css::plugin_class(p));
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.set_vexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    main.add_css_class("cc-split-main");
    main.set_hexpand(true);
    main.append(&cc_chip(icon, on));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.append(&cc_label(name, "cc-title"));
    text.append(&cc_label(status, "cc-status"));
    main.append(&text);
    let more = gtk::Button::from_icon_name("go-next-symbolic");
    more.add_css_class("cc-split-more");
    row.append(&main);
    row.append(&more);
    root.append(&row);
    root
}

fn cc_toggle(icon: &str, on: bool) -> gtk::Box {
    let root = cc_frame(1, 1);
    root.add_css_class("cc-toggle");
    let chip = cc_chip(icon, on);
    chip.set_halign(gtk::Align::Center);
    chip.set_vexpand(true);
    root.append(&chip);
    root
}

fn cc_icon_button(icon: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("cc-icon-button");
    b
}

fn cc_sliders() -> gtk::Box {
    let tile = cc_frame(4, 2);
    tile.add_css_class("cc-sliders");
    tile.set_homogeneous(true);
    for (icon, v) in [("audio-volume-high-symbolic", 0.64), ("display-brightness-symbolic", 0.8)] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("cc-slider");
        row.append(&cc_icon_button(icon));
        let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        s.set_draw_value(false);
        s.set_hexpand(true);
        s.set_value(v);
        row.append(&s);
        let l = gtk::Label::new(Some(&format!("{}%", (v * 100.0) as i32)));
        l.add_css_class("cc-value");
        l.set_width_chars(4);
        l.set_xalign(1.0);
        row.append(&l);
        tile.append(&row);
    }
    tile
}

/// A stand-in Control Center: its header, then the dashboard or the AI Hub detail page.
fn cc_panel(detail: Option<&str>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("cc-panel");
    root.set_size_request(400, -1);
    if let Some(view) = detail {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("cc-detail");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.add_css_class("cc-detail-head");
        head.append(&cc_icon_button("go-previous-symbolic"));
        let t = cc_label("AI Hub", "cc-detail-title");
        t.set_hexpand(true);
        head.append(&t);
        page.append(&head);
        let body = gtk::ScrolledWindow::new();
        body.set_hscrollbar_policy(gtk::PolicyType::Never);
        body.set_propagate_natural_height(true);
        body.set_max_content_height(680);
        body.add_css_class("cc-plugin-detail");
        if live_mode() {
            let pop = slot();
            body.set_child(Some(&pop));
            SLOTS.with(|s| s.borrow_mut().2 = Some(pop));
        } else {
            let rendered = view::render(&node(popover_value(view, false), view), &ctx(None));
            left_align_rows(&rendered.root);
            body.set_child(Some(&rendered.root));
        }
        scroll_to(&body);
        page.append(&body);
        root.append(&page);
        return root;
    }
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.add_css_class("cc-header");
    let avatar = gtk::Label::new(Some("K"));
    avatar.add_css_class("cc-avatar");
    avatar.set_valign(gtk::Align::Center);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&cc_label("Ken", "cc-name"));
    text.append(&cc_label("Battery 82% · 4 h 10 min left", "cc-sub"));
    header.append(&avatar);
    header.append(&text);
    header.append(&cc_icon_button("emblem-system-symbolic"));
    header.append(&cc_icon_button("system-shutdown-symbolic"));
    root.append(&header);

    let grid = gtk::Grid::new();
    grid.add_css_class("cc-grid");
    grid.set_column_homogeneous(true);
    grid.set_row_homogeneous(true);
    grid.set_column_spacing(GAP as u32);
    grid.set_row_spacing(GAP as u32);
    grid.attach(&cc_split("network-wireless-signal-excellent-symbolic", "Wi-Fi", "Home 5G", true, None), 0, 0, 2, 1);
    grid.attach(&cc_split("bluetooth-active-symbolic", "Bluetooth", "AirPods Pro", true, None), 2, 0, 2, 1);
    if live_mode() {
        let compact = gtk::Box::new(gtk::Orientation::Vertical, 0);
        grid.attach(&compact, 0, 1, 2, 1);
        SLOTS.with(|s| s.borrow_mut().1 = Some(compact));
    } else {
        grid.attach(&cc_split("nyx-ai-hub-symbolic", "AI Hub", "1 needs you", true, Some("nyx.ai-hub")), 0, 1, 2, 1);
    }
    grid.attach(&cc_toggle("notifications-disabled-symbolic", false), 2, 1, 1, 1);
    grid.attach(&cc_toggle("weather-clear-night-symbolic", false), 3, 1, 1, 1);

    let tile = cc_frame(4, 2);
    tile.add_css_class("cc-plugin");
    tile.add_css_class("cc-plugin-view");
    tile.add_css_class("cc-clickable");
    tile.add_css_class(&view::css::plugin_class("nyx.ai-hub"));
    if live_mode() {
        let b = slot();
        tile.append(&b);
        SLOTS.with(|s| s.borrow_mut().0 = Some(b));
    } else {
        let r = view::render(&load("cc-tile"), &ctx(None));
        r.root.set_vexpand(true);
        tile.append(&r.root);
    }
    grid.attach(&tile, 0, 2, 4, 2);
    grid.attach(&cc_sliders(), 0, 4, 4, 2);
    root.append(&grid);
    root
}

fn build(app: &gtk::Application) {
    let tab = std::env::var("MOCK_TAB").unwrap_or_else(|_| "agents".into());
    if tab.starts_with("cc") {
        let window = gtk::ApplicationWindow::builder().application(app).default_width(1280).default_height(800).decorated(false).build();
        window.add_css_class("nyx-control-center");
        window.add_css_class("mock-desktop");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bar.add_css_class("mock-bar");
        bar.set_height_request(32);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&bar);
        let detail = match tab.as_str() {
            "cc" => None,
            "cc-detail" => Some("agents"),
            t => t.strip_prefix("cc-"),
        };
        let panel = cc_panel(detail);
        panel.set_halign(gtk::Align::End);
        panel.set_valign(gtk::Align::Start);
        panel.set_margin_top(6);
        panel.set_margin_end(6);
        column.append(&panel);
        window.set_child(Some(&column));
        window.present();
        if live_mode() {
            let (tile, compact, pop) = SLOTS.with(|s| s.borrow_mut().clone());
            let detail = pop.is_some();
            live::start(
                live::Live {
                    item_box: tile.unwrap_or_else(slot),
                    popover_box: pop.unwrap_or_else(slot),
                    kind: "control-center".into(),
                    item: "overview".into(),
                    compact: if detail { None } else { compact },
                },
                ctx(None),
                ctx(None),
            );
        }
        return;
    }
    let window = gtk::ApplicationWindow::builder().application(app).default_width(1280).default_height(800).decorated(false).build();
    window.add_css_class("nyx-bar");
    window.add_css_class("mock-desktop");

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar-sections");
    bar.add_css_class("mock-bar");
    bar.set_height_request(32);
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for (n, active) in [("1", false), ("2", true), ("3", false), ("4", false)] {
        left.append(&label_item(n, if active { &["active"] } else { &["dim"] }));
    }
    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&label_item("Tue Nov 10  01:42", &[])));
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let hub = if live_mode() {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("bar-item");
        b.add_css_class(&view::css::plugin_class("nyx.ai-hub"));
        b.set_valign(gtk::Align::Fill);
        b
    } else {
        item(&load("item"), &view::css::plugin_class("nyx.ai-hub"))
    };
    if tab != "item" {
        hub.add_css_class("open");
    }
    right.append(&hub);
    for i in ["network-wireless-signal-excellent-symbolic", "audio-volume-medium-symbolic", "battery-good-symbolic", "preferences-system-notifications-symbolic"] {
        right.append(&icon_item(i));
    }
    bar.set_end_widget(Some(&right));

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&bar);
    let desktop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    desktop.set_vexpand(true);
    column.append(&desktop);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&column));
    window.set_child(Some(&overlay));

    let live_pop = slot();
    if tab != "item" {
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_propagate_natural_width(true);
        scroll.set_max_content_height(640);
        scroll.set_max_content_width(480);
        if live_mode() {
            scroll.set_child(Some(&live_pop));
        } else {
            let rendered = view::render(&popover(&tab), &ctx(None));
            left_align_rows(&rendered.root);
            scroll.set_child(Some(&rendered.root));
        }
        scroll_to(&scroll);
        let pop = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pop.add_css_class("mock-popover");
        pop.append(&scroll);
        pop.set_halign(gtk::Align::End);
        pop.set_valign(gtk::Align::Start);
        pop.set_margin_top(38);
        pop.set_margin_end(std::env::var("MOCK_END").ok().and_then(|v| v.parse().ok()).unwrap_or(40));
        overlay.add_overlay(&pop);
    }
    window.present();
    if live_mode() {
        live::start(
            live::Live { item_box: hub, popover_box: live_pop, kind: "bar".into(), item: "hub".into(), compact: None },
            ctx(Some(32)),
            ctx(None),
        );
    }
}

/// A row wider than its content keeps the content at the start, not centred.
pub fn left_align_rows(w: &gtk::Widget) {
    if w.has_css_class("plugin-row") {
        if let Some(c) = w.first_child() {
            c.set_halign(gtk::Align::Start);
        }
    }
    let mut c = w.first_child();
    while let Some(child) = c {
        left_align_rows(&child);
        c = child.next_sibling();
    }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id("io.github.design_nexus.nyx.AppTest").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| {
        theme();
        plugin_css();
    });
    app.connect_activate(build);
    app.run_with_args::<&str>(&[])
}
