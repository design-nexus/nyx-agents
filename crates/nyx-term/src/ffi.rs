//! Small helpers over the raw bindings.

use std::os::raw::c_void;

pub use ghostty_vt_sys::*;

use crate::{Error, Result, Rgb};

pub fn check(r: GhosttyResult) -> Result<()> {
    if r == GHOSTTY_SUCCESS { Ok(()) } else { Err(Error(r)) }
}

pub fn ptr<T>(v: &T) -> *const c_void {
    v as *const T as *const c_void
}

pub fn mut_ptr<T>(v: &mut T) -> *mut c_void {
    v as *mut T as *mut c_void
}

pub fn gstr(s: &[u8]) -> GhosttyString {
    GhosttyString { ptr: s.as_ptr(), len: s.len() }
}

/// Borrow a library string as bytes. Empty strings may carry a null pointer.
///
/// # Safety
/// `s` must point at `len` valid bytes for the returned lifetime.
pub unsafe fn bytes<'a>(s: GhosttyString) -> &'a [u8] {
    if s.ptr.is_null() || s.len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(s.ptr, s.len) } }
}

pub fn rgb(c: GhosttyColorRgb) -> Rgb {
    Rgb { r: c.r, g: c.g, b: c.b }
}

pub fn c_rgb(c: Rgb) -> GhosttyColorRgb {
    GhosttyColorRgb { r: c.r, g: c.g, b: c.b }
}

/// Free a buffer the library allocated with the default allocator.
///
/// # Safety
/// `ptr`/`len` must come from a libghostty `_alloc` call with a null allocator.
pub unsafe fn take_alloc(ptr: *mut u8, len: usize) -> Vec<u8> {
    if ptr.is_null() {
        return Vec::new();
    }
    let v = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { ghostty_free(std::ptr::null(), ptr, len) };
    v
}

/// Initialise a sized struct: zeroed, with its leading `size` field set.
pub fn sized<T: Default>(set: impl FnOnce(&mut T, usize)) -> T {
    let mut v = T::default();
    set(&mut v, std::mem::size_of::<T>());
    v
}
