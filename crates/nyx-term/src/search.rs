use std::ptr;

use crate::ffi::*;
use crate::{Result, Selection, Terminal};

/// A match on screen, in viewport cells (columns inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub row: u16,
    pub start: u16,
    pub end: u16,
}

/// Find-in-scrollback over one terminal. Searching runs incrementally; call
/// [`Search::run`] after the needle or the terminal changes.
pub struct Search {
    raw: GhosttySearch,
}

unsafe impl Send for Search {}

impl Search {
    pub fn new(term: &Terminal) -> Result<Self> {
        let mut raw: GhosttySearch = ptr::null_mut();
        check(unsafe { ghostty_search_new(ptr::null(), &mut raw, term.raw()) })?;
        let scroll = true;
        unsafe { ghostty_search_set(raw, GHOSTTY_SEARCH_OPT_SELECT_SCROLL, ptr(&scroll)) };
        Ok(Self { raw })
    }

    pub fn set_needle(&mut self, needle: &str) {
        unsafe { ghostty_search_set(self.raw, GHOSTTY_SEARCH_OPT_NEEDLE, ptr(&gstr(needle.as_bytes()))) };
    }

    /// Search to completion.
    pub fn run(&mut self) {
        unsafe { ghostty_search_run(self.raw) };
    }

    fn get<T: Default>(&self, data: GhosttySearchData) -> Option<T> {
        let mut v = T::default();
        (unsafe { ghostty_search_get(self.raw, data, mut_ptr(&mut v)) } == GHOSTTY_SUCCESS).then_some(v)
    }

    pub fn total(&self) -> usize {
        self.get::<usize>(GHOSTTY_SEARCH_DATA_TOTAL_MATCHES).unwrap_or(0)
    }

    /// Index of the selected match, newest first.
    pub fn selected(&self) -> Option<usize> {
        self.get::<usize>(GHOSTTY_SEARCH_DATA_SELECTED_INDEX)
    }

    /// Move to the next (older) or previous (newer) match and scroll to it.
    pub fn step(&mut self, older: bool) -> Option<Selection> {
        let r = unsafe { if older { ghostty_search_select_next(self.raw) } else { ghostty_search_select_prev(self.raw) } };
        if r != GHOSTTY_SUCCESS {
            return None;
        }
        self.get::<GhosttySelection>(GHOSTTY_SEARCH_DATA_SELECTED_MATCH).map(Selection)
    }

    /// Matches covering the viewport, for highlights.
    pub fn viewport_matches(&self, term: &Terminal) -> Vec<SearchMatch> {
        let mut buf = GhosttySelectionBuffer { ptr: ptr::null_mut(), cap: 0, len: 0 };
        let r = unsafe { ghostty_search_get(self.raw, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, mut_ptr(&mut buf)) };
        if r != GHOSTTY_OUT_OF_SPACE && r != GHOSTTY_SUCCESS {
            return Vec::new();
        }
        if buf.len == 0 {
            return Vec::new();
        }
        let mut items = vec![GhosttySelection::default(); buf.len];
        buf = GhosttySelectionBuffer { ptr: items.as_mut_ptr(), cap: items.len(), len: 0 };
        if unsafe { ghostty_search_get(self.raw, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, mut_ptr(&mut buf)) } != GHOSTTY_SUCCESS {
            return Vec::new();
        }
        items.truncate(buf.len);
        let rows = term.rows();
        let mut out = Vec::new();
        for s in items {
            let mut a = GhosttyPointCoordinate::default();
            let mut b = GhosttyPointCoordinate::default();
            unsafe {
                if ghostty_terminal_point_from_grid_ref(term.raw(), &s.start, GHOSTTY_POINT_TAG_VIEWPORT, &mut a)
                    != GHOSTTY_SUCCESS
                    || ghostty_terminal_point_from_grid_ref(term.raw(), &s.end, GHOSTTY_POINT_TAG_VIEWPORT, &mut b)
                        != GHOSTTY_SUCCESS
                {
                    continue;
                }
            }
            // A match that wraps lines is drawn on each row it covers.
            for y in a.y..=b.y {
                if y >= rows as u32 {
                    break;
                }
                let start = if y == a.y { a.x } else { 0 };
                let end = if y == b.y { b.x } else { term.cols().saturating_sub(1) };
                out.push(SearchMatch { row: y as u16, start, end });
            }
        }
        out
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        unsafe { ghostty_search_free(self.raw) };
    }
}
