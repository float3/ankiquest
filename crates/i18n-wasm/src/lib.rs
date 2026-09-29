//! The WebAssembly interface of `ankiquest-i18n`, used by `web/i18n.ts`.
//!
//! Strings cross as UTF-8: the page copies each argument into memory from
//! `alloc`, and a result comes back as `pointer << 32 | length`, which the page
//! reads and then releases with `dealloc`.

use std::alloc::{Layout, alloc as allocate, dealloc as release};

/// # Safety
/// Only for the page: the returned block must be released with `dealloc(ptr, len)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::dangling().as_ptr();
    }
    unsafe { allocate(Layout::array::<u8>(len).unwrap()) }
}

/// # Safety
/// `ptr` and `len` must come from `alloc` or a returned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if len != 0 {
        unsafe { release(ptr, Layout::array::<u8>(len).unwrap()) }
    }
}

/// Takes ownership of an argument block the page filled.
unsafe fn argument(ptr: *mut u8, len: usize) -> String {
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into())
}

fn result(value: String) -> u64 {
    let bytes = value.into_bytes().into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *mut u8;
    ((ptr as usize as u64) << 32) | len as u64
}

/// # Safety
/// The arguments must be blocks from `alloc`, which this takes ownership of.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn normalize(ptr: *mut u8, len: usize) -> u64 {
    let language = unsafe { argument(ptr, len) };
    result(ankiquest_i18n::normalize(&language).into())
}

/// # Safety
/// The arguments must be blocks from `alloc`, which this takes ownership of.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn translate(
    language: *mut u8,
    language_len: usize,
    ptr: *mut u8,
    len: usize,
) -> u64 {
    let language = unsafe { argument(language, language_len) };
    let source = unsafe { argument(ptr, len) };
    result(ankiquest_i18n::translate(&source, &language))
}

/// # Safety
/// The arguments must be blocks from `alloc`, which this takes ownership of.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn translate_html(
    language: *mut u8,
    language_len: usize,
    ptr: *mut u8,
    len: usize,
) -> u64 {
    let language = unsafe { argument(language, language_len) };
    let source = unsafe { argument(ptr, len) };
    result(ankiquest_i18n::translate_html(&source, &language))
}
