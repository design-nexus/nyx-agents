//! What a scanner has already read of each append-only record file: the byte
//! offset parsed so far and whatever it added up from the lines before it, one
//! cache file per record (`~/.cache/nyx-agents/usage/<agent>/logs`). Each scan
//! reads only what was appended since. A file that shrank was rewritten and is
//! read again whole; a half-written last line waits for the next scan.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Stored<E> {
    v: u32,
    offset: u64,
    entry: E,
}

pub struct LogCache {
    dir: PathBuf,
    version: u32,
    live: HashSet<String>,
}

/// A record's entry, and the file's modification time (Unix seconds).
pub struct Record<E> {
    pub entry: E,
    pub mtime: f64,
}

impl LogCache {
    /// `version` is bumped when what a scanner records changes, so caches are rebuilt.
    pub fn new(agent: &str, version: u32) -> Self {
        Self { dir: super::cache_dir(agent).join("logs"), version, live: HashSet::new() }
    }

    fn file(&self, path: &Path) -> PathBuf {
        // FNV-1a: a stable name per record path.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in path.as_os_str().as_encoded_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        self.dir.join(format!("{h:016x}.json"))
    }

    /// The entry for `path` once `parse(entry, line)` has seen each line appended since
    /// the last scan.
    pub fn update<E: Serialize + DeserializeOwned + Default>(&mut self, path: &Path, mut parse: impl FnMut(&mut E, &[u8], f64)) -> Option<Record<E>> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let cache = self.file(path);
        self.live.insert(cache.file_name()?.to_string_lossy().into_owned());
        let stored: Option<Stored<E>> = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let mut s = match stored {
            Some(s) if s.v == self.version && s.offset <= meta.len() => s,
            _ => Stored { v: self.version, offset: 0, entry: E::default() },
        };
        if meta.len() > s.offset {
            let mut f = std::fs::File::open(path).ok()?;
            f.seek(SeekFrom::Start(s.offset)).ok()?;
            let mut data = Vec::with_capacity((meta.len() - s.offset) as usize);
            f.take(meta.len() - s.offset).read_to_end(&mut data).ok()?;
            let end = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            if end > 0 {
                for line in data[..end].split(|b| *b == b'\n') {
                    if !line.is_empty() {
                        parse(&mut s.entry, line, mtime);
                    }
                }
                s.offset += end as u64;
                if crate::paths::private_dir(&self.dir).is_ok()
                    && let Ok(bytes) = serde_json::to_vec(&s)
                {
                    let _ = crate::paths::write_private(&cache, &bytes);
                }
            }
        }
        Some(Record { entry: s.entry, mtime })
    }

    /// Drop the caches of records that no longer exist. Call after visiting every record.
    pub fn prune(&self) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return };
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && !self.live.contains(&name) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
