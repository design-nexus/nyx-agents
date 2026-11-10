//! Starts a real headless sway, opens foot in it, types, clicks and captures.
//! Only with `--features headless-it`; it needs sway and foot installed.
//!
//!   cargo test -p nyx-agent-workspace --features headless-it -- --test-threads=1
//!
//! It fails if the user's Hyprland focus changes or a window appears on the
//! user's desktop. The pointer is only reported: the user moving the mouse
//! would move it too.
#![cfg(feature = "headless-it")]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use nyx_agent_workspace::{Capturer, Clipboard, Input, Instance, Options, png};

fn hyprctl(what: &str) -> Option<String> {
    let out = Command::new("hyprctl").args([what, "-j"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The focused window and every window address on the user's desktop.
fn user_desktop() -> (Option<String>, Vec<String>) {
    let focus = hyprctl("activewindow").and_then(|j| {
        let v: serde_json::Value = serde_json::from_str(&j).ok()?;
        v["address"].as_str().map(String::from)
    });
    let mut windows: Vec<String> = hyprctl("clients")
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|w| w["address"].as_str().map(String::from)).collect()))
        .unwrap_or_default();
    windows.sort();
    (focus, windows)
}

fn scratch() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("nyx-agent-ws-it-{}", std::process::id()));
    (base.join("run"), base.join("data"))
}

#[test]
fn a_workspace_runs_foot_and_takes_input() {
    let before = user_desktop();
    let pointer_before = hyprctl("cursorpos");
    let (runtime, data) = scratch();
    let ws = Instance::start("it", &runtime, &data, Options::default()).expect("sway starts headless");
    assert!(ws.alive());
    let log = std::fs::read_to_string(&ws.log).unwrap_or_default();
    assert!(log.contains("headless"), "sway should report the headless backend:\n{log}");

    if ws.display.is_none() {
        let log = std::fs::read_to_string(&ws.log).unwrap_or_default();
        eprintln!(
            "{}",
            log.lines().filter(|l| l.to_lowercase().contains("xwayland") || l.contains("ERROR")).collect::<Vec<_>>().join("\n")
        );
        eprintln!(
            "{} log lines; X11 sockets: {:?}",
            log.lines().count(),
            std::fs::read_dir("/tmp/.X11-unix").map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
        );
        let fds: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", ws.sway_pid()))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .map(|p| p.display().to_string())
            .collect();
        eprintln!("sway fds: {fds:?}");
    }
    assert!(ws.display.as_deref().is_some_and(|d| d.starts_with(':')), "X11 programs get a display: {:?}", ws.display);
    let typed = runtime.parent().unwrap().join("typed.txt");
    let cat = format!("cat > {}", typed.display());
    ws.launch(&["foot".into(), "--".into(), "sh".into(), "-c".into(), cat], None).expect("foot starts");
    let start = Instant::now();
    let foot = loop {
        if let Some(w) = ws.windows().unwrap().into_iter().find(|w| w.app_id.as_deref() == Some("foot")) {
            break w;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "foot never mapped a window");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(foot.width > 600 && foot.height > 400, "foot fills the workspace: {foot:?}");

    let (w, h) = Options::default().logical();
    let mut input = Input::connect(&ws.wayland, w, h).expect("virtual input");
    input.click(Some((foot.x + 50, foot.y + 50)), nyx_agent_workspace::Button::Left, 1).unwrap();
    // Keys land on a US layout's keycodes, punctuation and shifted keys included.
    let line = "Hello, World! ~/x_y=1+2 [ok] \"q\" <a|b> é\n";
    input.type_text(line).unwrap();
    input.key("ctrl+d").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(std::fs::read_to_string(&typed).unwrap_or_default(), line, "typed text arrives as typed");
    assert_eq!(input.position(), ((foot.x + 50) as u32, (foot.y + 50) as u32));

    // cat has ended and foot with it; open another to have something on screen.
    ws.launch(&["foot".into()], None).expect("foot starts again");
    let start = Instant::now();
    while !ws.windows().unwrap().iter().any(|w| w.app_id.as_deref() == Some("foot")) {
        assert!(start.elapsed() < Duration::from_secs(10), "the second foot never mapped");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));

    let writer = Clipboard::connect(&ws.wayland).expect("clipboard");
    writer.set_text("from the test").unwrap();
    let reader = Clipboard::connect(&ws.wayland).expect("a second clipboard client");
    assert_eq!(reader.get_text().unwrap().as_deref(), Some("from the test"), "the clipboard passes between clients");
    drop((writer, reader));

    let mut cap = Capturer::connect(&ws.wayland).expect("capture");
    let frame = cap.capture().expect("a frame");
    assert_eq!((frame.width, frame.height), (1280, 800));
    let bytes = png(&frame).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"));
    assert!(frame.data.chunks(4).any(|p| p[..3] != frame.data[..3]), "the frame is not blank");

    ws.resize(Options { width: 1024, height: 768, scale: 1.0 }).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let frame = cap.capture().unwrap();
    assert_eq!((frame.width, frame.height), (1024, 768));

    drop(cap);
    drop(input);
    ws.stop();
    assert!(!ws.alive());
    assert!(!ws.dir.exists(), "the workspace folder is removed");
    let _ = std::fs::remove_dir_all(runtime.parent().unwrap());
    eprintln!("pointer before {:?}, after {:?}", pointer_before, hyprctl("cursorpos"));
    let after = user_desktop();
    assert_eq!(after.0, before.0, "the user's focus changed");
    let new: Vec<&String> = after.1.iter().filter(|w| !before.1.contains(w)).collect();
    assert!(new.is_empty(), "windows appeared on the user's desktop: {new:?}");
}
