# Terminal: libghostty-vt in GTK4

Phase 0 result: **the Ghostty-based terminal works in GTK4 and meets the
performance targets.** No VTE anywhere.

## What Ghostty provides, and what we build

- **libghostty-vt 1.0** (MIT, released 2026-11-06, stable C ABI) is terminal
  *emulation*: VT parsing, screen and scrollback with reflow, a render state
  with dirty tracking, key and mouse encoders, selection, search, snapshots,
  OSC 7501 program status, Kitty graphics. It has **no renderer and no font
  handling**.
- Ghostty's full embedding API (`ghostty.h`) is macOS/iOS only. There is no
  GTK4 widget to embed, and running the `ghostty` binary cannot be placed
  inside our layout on Wayland.
- So the drawing is ours: `nyx-term-gtk::TerminalView` draws the render state
  with GtkSnapshot and Pango, and GSK renders it on the GPU (Vulkan or GL).
  This is the same split Herdr Desktop uses (it draws with GPUI instead).

## Crates

| Crate | Role |
|---|---|
| `ghostty-vt-sys` | Builds the pinned libghostty-vt 1.0 release statically with Zig 0.17, offline, from `vendor/libghostty-vt` (checksums and minisign checked); bindgen at build time. `NYX_GHOSTTY_VT_PREFIX` uses an existing install instead. |
| `nyx-term` | Safe wrapper: `Terminal` (write, resize, scroll, colours, snapshot, plain/VT text, hyperlinks, selection gestures, paste safety), `Effects` (query replies, title, pwd, bell, OSC 7501 status, clipboard writes capped at 1 MiB, notifications, render holds, prompt starts), `Renderer` → `Frame` of styled runs with per-row versions, `KeyEncoder`, `MouseEncoder`, `Search`. |
| `nyx-pty` | Spawn an argv on a new pty (own session, controlling tty), resize, signal the process group, foreground pgid. |
| `nyx-term-gtk` | `TerminalView`: per-row render-node cache rebuilt only for dirty rows, IME via GtkIMMulticontext, Kitty/legacy key encoding through libghostty, mouse reporting, selection (click, double-click word, drag), primary selection, Ctrl+Shift+C/V, find with highlights, Ctrl+click links (http, https, file, mailto only), scroll and alternate-scroll, synchronized-output holds (1 s cap), pixel-exact box drawing and block elements, read-only mode. |
| `apps/term-lab` | `nyx-term-lab`: a shell window (`--mirror` adds a read-only viewer below) and the benchmarks. |

The view never touches a pty: hosts feed it output and receive input bytes.
The same widget will draw daemon-owned sessions and the subagent inspector.

A read-only view sends nothing (no keys, no query replies, no resizes, no
clipboard writes), draws at the program's grid, and still allows select,
copy and find.

## Benchmarks

`nyx-term-lab --bench NAME` prints JSON. Results are in `docs/bench/`.

Two environments, because a headless Hyprland output only paints while it is
being captured:

- **hyprland-headless**: a private headless output; frames rendered by the
  window's own GSK renderer, driven by the benchmark at 60 Hz.
- **xvfb-vulkan**: a private Xvfb display and D-Bus session (no compositor
  involved); frames rendered offscreen on the Intel iGPU by `GskVulkanRenderer`
  (`NYX_BENCH_RENDERER=vulkan`). Xvfb also software-paints the window itself,
  which costs main-thread time, so single-terminal throughput there is lower
  than on Hyprland.

Machine: Intel Arrow Lake-P iGPU (Arc 130T/140T), JetBrainsMono Nerd Font 11,
1280x800 window, release build.

| Workload | Hyprland headless | Xvfb + Vulkan |
|---|---|---|
| 100 MB `cat` burst, one 140x40 terminal | 0.84 s, **120 MB/s** | 2.37 s, 42 MB/s |
| Same, with a read-only mirror attached | 0.72 s, 139 MB/s | 1.01 s, 99 MB/s |
| Parse cost of 100 MB (libghostty-vt) | 283 ms | 311 ms |
| Key → echo → drawn frame, idle (p50 / p99) | 2.3 / 4.0 ms | **1.3 / 2.8 ms** |
| Same, while another terminal floods output | 3.5 / 7.8 ms | 3.4 / 12.2 ms |
| Scrollback page flip, 8,283 rows (p50 / p99) | 4.9 / 6.8 ms | 4.0 / 6.3 ms |
| 4 terminals × 20 MB at once | — | 0.42 s, 194 MB/s total |
| 8 terminals × 20 MB at once | — | 0.75 s, 215 MB/s total |
| Resize 1100x700 ↔ 640x420, to drawn frame (p50 / p99) | — | 5.0 / 6.9 ms |
| GPU render of a full frame (p50 / p95) | 2.3 / 3.5 ms | 1.3 / 2.6 ms |
| Mirror (read-only) draw cost per frame | 0.79 ms | 0.99 ms |
| Terminal memory, 10 MB scrollback budget | — | 9.9 MB each |
| Process RSS, 8 terminals | — | 316 MB (GTK + Vulkan + Mesa ≈ 220 MB of it) |

Latency excludes the compositor's wait for the next refresh (up to one frame
on a real display). The first GPU frame of a run takes ~120 ms (shader
compilation) and is visible as the p99 outlier in some files.

`docs/bench/test-pattern.png` is a frame rendered through Vulkan: styles,
256/true colour, CJK, combining marks, colour emoji, Nerd Font icons, box
drawing, a selection, a search hit, the cursor and a read-only viewer.

Not yet measured: a recorded coding-agent session. `--bench replay=FILE`
replays a pty capture; Phase 1's daemon will record real Claude Code output
for it.

## Decisions

- **Parse on the GTK main thread for now.** 100 MB parses in under 0.3 s and
  typing stays under 13 ms p99 while another terminal floods. The daemon
  (Phase 1) parses its own copy for snapshots and status, so the app only
  parses what it displays.
- **GtkSnapshot + Pango, no custom glyph atlas.** Frame snapshots cost about
  2 ms and GPU rendering 1-3 ms for a full 140x40 grid; a GL/DMABUF atlas
  renderer is not needed at these sizes.
- **Box drawing and blocks are drawn as shapes**, as Ghostty does, so lines
  join across cells whatever the font's glyph height.
- **Status comes from libghostty-vt's OSC 7501 parser**, not our own.

## Known gaps

- Kitty graphics images are parsed but not drawn yet.
- Key repeat is reported as a press (GDK does not flag repeats).
- Focus reporting (mode 1004) is not sent yet.
- OSC 8 links have no hover underline yet.
- Ligatures are not enabled (runs are drawn with letter spacing that pins
  ASCII to the grid).

## Incident during testing

On 2026-11-09 at 22:59:33 the user's Hyprland session crashed during the
benchmark runs, about 0.3 s after the test helper re-applied `hl.monitor` to
its own headless output. A debug Hyprland build from another session had
aborted nine seconds earlier. The cause is not established; no core dump was
written. Benchmarks since then run on a private Xvfb display with no
compositor calls.
