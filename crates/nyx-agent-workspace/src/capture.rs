//! Copying the workspace's output through `wlr-screencopy`, which every sway
//! release offers, and encoding it as PNG.

use std::io::{Read, Seek};
use std::os::fd::AsFd;
use std::path::Path;
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1 as frame, zwlr_screencopy_manager_v1 as manager};

use crate::wl;

/// One copy of the output: tightly packed RGBA, top row first.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct State {
    manager: Option<(manager::ZwlrScreencopyManagerV1, u32)>,
    output: Option<wl_output::WlOutput>,
    shm: Option<wl_shm::WlShm>,
    buffer: Option<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    y_invert: bool,
    result: Option<bool>,
}

/// A capture connection to one workspace, kept open between frames.
pub struct Capturer {
    _conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    damaged_once: bool,
    cursor: bool,
}

impl Capturer {
    pub fn connect(socket: &Path) -> Result<Self, String> {
        let conn = wl::connect(socket)?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        if state.manager.is_none() || state.output.is_none() || state.shm.is_none() {
            return Err("the workspace doesn't offer screen capture".into());
        }
        Ok(Self { _conn: conn, queue, qh, state, damaged_once: false, cursor: false })
    }

    /// Draw the pointer into frames (screenshots for the agent leave it out).
    pub fn with_cursor(mut self) -> Self {
        self.cursor = true;
        self
    }

    /// The current frame.
    pub fn capture(&mut self) -> Result<Frame, String> {
        self.copy(false, None).map(|f| f.expect("no deadline"))
    }

    /// The next frame that differs from the last one, or `None` if nothing
    /// changed before `timeout`. The first call returns at once.
    pub fn next_change(&mut self, timeout: Duration) -> Result<Option<Frame>, String> {
        let damage = self.damaged_once;
        self.damaged_once = true;
        self.copy(damage, Some(Instant::now() + timeout))
    }

    fn copy(&mut self, damage: bool, deadline: Option<Instant>) -> Result<Option<Frame>, String> {
        let (m, version) = self.state.manager.clone().unwrap();
        let output = self.state.output.clone().unwrap();
        let shm = self.state.shm.clone().unwrap();
        self.state.buffer = None;
        self.state.buffer_done = false;
        self.state.y_invert = false;
        self.state.result = None;
        let fr = m.capture_output(self.cursor as i32, &output, &self.qh, ());
        let ok = wl::dispatch_until(&mut self.queue, &mut self.state, deadline, |s| {
            s.result == Some(false) || (s.buffer.is_some() && (s.buffer_done || version < 3))
        })?;
        let Some((format, w, h, stride)) = self.state.buffer.filter(|_| ok && self.state.result != Some(false)) else {
            fr.destroy();
            return if ok { Err("the workspace refused the capture".into()) } else { Ok(None) };
        };
        let len = (stride * h) as usize;
        let mut file = wl::memfd(len)?;
        let pool = shm.create_pool(file.as_fd(), len as i32, &self.qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &self.qh, ());
        if damage {
            fr.copy_with_damage(&buffer);
        } else {
            fr.copy(&buffer);
        }
        let ok = wl::dispatch_until(&mut self.queue, &mut self.state, deadline, |s| s.result.is_some());
        fr.destroy();
        buffer.destroy();
        pool.destroy();
        match ok? {
            false => return Ok(None),
            true if self.state.result != Some(true) => return Err("the capture failed".into()),
            true => {}
        }
        let mut raw = vec![0u8; len];
        file.seek(std::io::SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        file.read_exact(&mut raw).map_err(|e| e.to_string())?;
        to_rgba(&raw, format, w, h, stride, self.state.y_invert).map(Some)
    }
}

/// Repack a shm buffer as tight RGBA rows, top first.
fn to_rgba(raw: &[u8], format: wl_shm::Format, w: u32, h: u32, stride: u32, y_invert: bool) -> Result<Frame, String> {
    use wl_shm::Format as F;
    let (swap, opaque) = match format {
        F::Argb8888 => (true, false),
        F::Xrgb8888 => (true, true),
        F::Abgr8888 => (false, false),
        F::Xbgr8888 => (false, true),
        f => return Err(format!("unsupported capture format {f:?}")),
    };
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let src_y = if y_invert { h - 1 - y } else { y };
        let row = &raw[(src_y * stride) as usize..][..(w * 4) as usize];
        for px in row.as_chunks::<4>().0 {
            // Little-endian ARGB8888 is B, G, R, A in memory.
            let (r, g, b) = if swap { (px[2], px[1], px[0]) } else { (px[0], px[1], px[2]) };
            data.extend_from_slice(&[r, g, b, if opaque { 255 } else { px[3] }]);
        }
    }
    Ok(Frame { width: w, height: h, data })
}

/// Encode a frame as an opaque RGB PNG.
pub fn png(f: &Frame) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = ::png::Encoder::new(&mut out, f.width, f.height);
        enc.set_color(::png::ColorType::Rgb);
        enc.set_depth(::png::BitDepth::Eight);
        enc.set_compression(::png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        let rgb: Vec<u8> = f.data.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        w.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Scale a frame down to fit `max_w` × `max_h`, averaging each block of
/// source pixels. Never scales up.
pub fn shrink(f: &Frame, max_w: u32, max_h: u32) -> Frame {
    let (w, h) = (f.width, f.height);
    let scale = (w as f64 / max_w.max(1) as f64).max(h as f64 / max_h.max(1) as f64).max(1.0);
    let (tw, th) = (((w as f64 / scale).round() as u32).max(1), ((h as f64 / scale).round() as u32).max(1));
    if (tw, th) == (w, h) {
        return f.clone();
    }
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = (ty as u64 * h as u64 / th as u64) as u32;
        let y1 = (((ty + 1) as u64 * h as u64 / th as u64) as u32).max(y0 + 1).min(h);
        for tx in 0..tw {
            let x0 = (tx as u64 * w as u64 / tw as u64) as u32;
            let x1 = (((tx + 1) as u64 * w as u64 / tw as u64) as u32).max(x0 + 1).min(w);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                let row = (y * w * 4) as usize;
                for x in x0..x1 {
                    let i = row + (x * 4) as usize;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += f.data[i + c] as u32;
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Frame { width: tw, height: th, data: out }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(s: &mut Self, r: &wl_registry::WlRegistry, e: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = e {
            match interface.as_str() {
                "zwlr_screencopy_manager_v1" => {
                    let v = version.min(3);
                    s.manager = Some((r.bind(name, v, qh, ()), v));
                }
                // A workspace has one output.
                "wl_output" if s.output.is_none() => s.output = Some(r.bind(name, 1, qh, ())),
                "wl_shm" => s.shm = Some(r.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<frame::ZwlrScreencopyFrameV1, ()> for State {
    fn event(s: &mut Self, _: &frame::ZwlrScreencopyFrameV1, e: frame::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            frame::Event::Buffer { format: WEnum::Value(f), width, height, stride } => {
                s.buffer = Some((f, width, height, stride));
            }
            frame::Event::Flags { flags: WEnum::Value(f) } => s.y_invert = f.contains(frame::Flags::YInvert),
            frame::Event::BufferDone => s.buffer_done = true,
            frame::Event::Ready { .. } => s.result = Some(true),
            frame::Event::Failed => s.result = Some(false),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_output::WlOutput);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: manager::ZwlrScreencopyManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_pixels_become_rgba() {
        // One row of two pixels, stride padded to 12 bytes: red, then green.
        let raw = [0, 0, 255, 0, 0, 255, 0, 7, 9, 9, 9, 9];
        let f = to_rgba(&raw, wl_shm::Format::Xrgb8888, 2, 1, 12, false).unwrap();
        assert_eq!(f.data, [255, 0, 0, 255, 0, 255, 0, 255]);
        let f = to_rgba(&raw, wl_shm::Format::Argb8888, 2, 1, 12, false).unwrap();
        assert_eq!(f.data[7], 7);
        assert!(to_rgba(&raw, wl_shm::Format::Rgb565, 2, 1, 12, false).is_err());
    }

    #[test]
    fn inverted_frames_are_flipped() {
        let raw = [1, 1, 1, 0, 2, 2, 2, 0];
        let f = to_rgba(&raw, wl_shm::Format::Xbgr8888, 1, 2, 4, true).unwrap();
        assert_eq!(f.data, [2, 2, 2, 255, 1, 1, 1, 255]);
    }

    #[test]
    fn png_round_trips() {
        let f = Frame { width: 3, height: 2, data: (0..24).map(|i| (i * 10) as u8).collect() };
        let bytes = png(&f).unwrap();
        let mut dec = ::png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; dec.output_buffer_size().unwrap()];
        let info = dec.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (3, 2, ::png::ColorType::Rgb));
        assert_eq!(&buf[..6], &[0, 10, 20, 40, 50, 60]);
    }

    #[test]
    fn shrink_halves_and_never_grows() {
        let f = Frame { width: 4, height: 2, data: [[255u8; 4], [255; 4], [0; 4], [0; 4]].repeat(2).concat() };
        let s = shrink(&f, 2, 2);
        assert_eq!((s.width, s.height), (2, 1));
        assert_eq!(&s.data[..8], &[255, 255, 255, 255, 0, 0, 0, 0]);
        assert_eq!(shrink(&f, 100, 100).width, 4);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
