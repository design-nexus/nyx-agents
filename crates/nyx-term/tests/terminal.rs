use nyx_term::*;

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(&TerminalOptions { cols, rows, ..Default::default() }).unwrap()
}

fn row_text(r: &Row) -> String {
    let mut s = String::new();
    for run in &r.runs {
        while (s.chars().count() as u16) < run.col {
            s.push(' ');
        }
        s.push_str(&run.text);
    }
    s
}

#[test]
fn version_is_1_0() {
    assert!(version().starts_with("1.0"), "{}", version());
}

#[test]
fn renders_styled_runs() {
    let mut t = term(40, 5);
    t.write(b"Hello, \x1b[1;32mworld\x1b[0m!\r\n");
    let mut r = Renderer::new().unwrap();
    assert_eq!(r.update(&t).unwrap(), Dirty::All);
    let row = &r.frame.rows[0];
    assert_eq!(row_text(row), "Hello, world!");
    let world = row.runs.iter().find(|run| run.text == "world").expect("separate run for the styled word");
    assert!(world.flags.has(StyleFlags::BOLD));
    assert_eq!(world.col, 7);
    assert_ne!(world.fg, r.frame.foreground);
    // Nothing changed: nothing to redraw.
    assert_eq!(r.update(&t).unwrap(), Dirty::None);
    let v0 = r.frame.rows[0].version;
    t.write(b"second line");
    assert_eq!(r.update(&t).unwrap(), Dirty::Rows);
    assert_eq!(r.frame.rows[0].version, v0, "untouched row keeps its version");
    assert_eq!(row_text(&r.frame.rows[1]), "second line");
}

#[test]
fn wide_and_combining_characters_get_their_own_cells() {
    let mut t = term(20, 3);
    t.write("a界b e\u{301}".as_bytes());
    let mut r = Renderer::new().unwrap();
    r.update(&t).unwrap();
    let runs = &r.frame.rows[0].runs;
    let wide = runs.iter().find(|x| x.text == "界").unwrap();
    assert_eq!((wide.col, wide.cols), (1, 2));
    let b = runs.iter().find(|x| x.text.starts_with('b')).unwrap();
    assert_eq!(b.col, 3);
    assert!(runs.iter().any(|x| x.text == "e\u{301}"));
}

#[test]
fn program_status_osc_7501() {
    let mut t = term(80, 24);
    // msg is base64 for "Apply?"
    t.write(b"\x1b]7501;state=blocked:kind=permission:app=claude-code:msg=QXBwbHk/\x1b\\");
    let fx = t.take_effects();
    assert_eq!(fx.status.len(), 1);
    let s = &fx.status[0];
    assert_eq!(s.state, ProgramState::Blocked);
    assert_eq!(s.kind, StatusKind::Permission);
    assert_eq!(s.app, "claude-code");
    assert_eq!(s.message, "Apply?");
    // A support query is answered on the pty.
    t.write(b"\x1b]7501;?\x1b\\");
    let fx = t.take_effects();
    assert!(String::from_utf8_lossy(&fx.reply).contains("states=idle,working,done,blocked,error"));
}

#[test]
fn title_and_queries_come_back_as_effects() {
    let mut t = term(80, 24);
    t.write(b"\x1b]2;my title\x07\x1b[6n");
    let fx = t.take_effects();
    assert!(fx.title_changed);
    assert_eq!(t.title(), "my title");
    assert_eq!(fx.reply, b"\x1b[1;1R");
}

#[test]
fn clipboard_writes_are_capped() {
    let mut t = Terminal::new(&TerminalOptions { clipboard_limit: 8, ..Default::default() }).unwrap();
    t.write(b"\x1b]52;c;aGk=\x07"); // "hi"
    t.write(b"\x1b]52;c;bG9uZ2VyIHRoYW4gZWlnaHQ=\x07"); // too long
    let fx = t.take_effects();
    assert_eq!(fx.clipboard, vec![ClipboardWrite { primary: false, text: "hi".into() }]);
}

#[test]
fn snapshot_round_trip_keeps_screen_and_scrollback() {
    let mut t = term(20, 4);
    for i in 0..30 {
        t.write(format!("line {i}\r\n").as_bytes());
    }
    let bytes = t.snapshot().unwrap();
    let restored = Terminal::from_snapshot(&bytes, &TerminalOptions::default()).unwrap();
    assert_eq!(restored.text(None).unwrap(), t.text(None).unwrap());
    assert!(restored.text(None).unwrap().contains("line 0"));
    assert_eq!((restored.cols(), restored.rows()), (20, 4));
    assert!(t.memory_usage() > 0);
}

#[test]
fn bracketed_paste_and_unsafe_paste() {
    let mut t = term(80, 24);
    assert_eq!(t.paste("ls", false).unwrap(), b"ls");
    assert_eq!(t.paste("rm -rf x\n", false), Err(true), "a newline could run a command");
    t.write(b"\x1b[?2004h");
    assert_eq!(t.paste("a\nb", false).unwrap(), b"\x1b[200~a\nb\x1b[201~");
}

#[test]
fn keys_encode_per_program_mode() {
    let t = term(80, 24);
    let mut k = KeyEncoder::new().unwrap();
    let c = Key::from_evdev(46);
    let bytes = k.encode(&t, c, KeyAction::Press, Mods(Mods::CTRL), Mods(0), "c", Some('c'), false);
    assert_eq!(bytes, [0x03]);
    let up = Key::from_evdev(103);
    assert_eq!(k.encode(&t, up, KeyAction::Press, Mods(0), Mods(0), "", None, false), b"\x1b[A");
    let mut t = t;
    t.write(b"\x1b[?1h"); // application cursor keys
    assert_eq!(k.encode(&t, up, KeyAction::Press, Mods(0), Mods(0), "", None, false), b"\x1bOA");
}

#[test]
fn search_finds_matches_in_scrollback() {
    let mut t = term(30, 5);
    for i in 0..50 {
        t.write(format!("row {i} {}\r\n", if i % 10 == 0 { "needle" } else { "hay" }).as_bytes());
    }
    let mut s = Search::new(&t).unwrap();
    s.set_needle("needle");
    s.run();
    assert_eq!(s.total(), 5);
    assert!(s.step(true).is_some());
}

#[test]
fn selection_text() {
    let mut t = term(40, 5);
    t.write(b"one two three");
    // A single press only anchors; dragging selects cell by cell.
    assert!(t.select_gesture(SelectGesture::Press { col: 4, row: 0, x: 40.0, y: 5.0, time_ns: 1 }).is_none());
    // A cell joins once the pointer passes its middle (cells are 10 px here).
    let sel = t.select_gesture(SelectGesture::Drag { col: 6, row: 0, x: 69.0, y: 5.0, cell_w: 10, height: 100 }).unwrap();
    assert_eq!(t.text(Some(&sel)).unwrap(), "two");
    assert!(t.selection().is_some());
    t.select_gesture(SelectGesture::Release { col: 6, row: 0 });
    // A double click selects the word under the pointer.
    t.select_gesture(SelectGesture::Press { col: 9, row: 0, x: 90.0, y: 5.0, time_ns: 2_000_000_000 });
    let word = t.select_gesture(SelectGesture::Press { col: 9, row: 0, x: 90.0, y: 5.0, time_ns: 2_100_000_000 }).unwrap();
    assert_eq!(t.text(Some(&word)).unwrap(), "three");
    let all = t.select_all().unwrap();
    assert_eq!(t.text(Some(&all)).unwrap(), "one two three");
}

#[test]
fn vt_replay_leaves_the_palette_alone() {
    let mut t = term(20, 3);
    t.write(b"\x1b[31mred\x1b[0m text");
    let vt = String::from_utf8(t.vt().unwrap()).unwrap();
    assert!(!vt.contains("\x1b]4;"), "{vt:?}");
    assert!(vt.contains("red") && vt.contains("text"));
}
