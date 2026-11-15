//! gala's webservices.dll (Windows Web Services): the XML reader the
//! Office Deployment Tool's setup.exe delay-loads to read the
//! configuration service's answer (ARCHITECTURE "Other DLLs"), and the
//! heap and error objects every call of it takes.
//!
//! What is implemented: heaps (the memory a read hands back, freed with
//! the heap), error objects and their strings, readers over a buffer of
//! text XML, reading to and into an element and reading an element as a
//! string or a number (`reader`), and writers as objects alone. The
//! document is read node by node with gala's XML reader (`gala_xml`'s
//! `Scan`, shared with galapkg and msxml6), so a malformed document
//! fails at the read that reaches it, as Windows' reader does, and not
//! when the input is set.
//!
//! What is not: the SOAP side (channels, service proxies, calls), which
//! SQL Server LocalDB imports and which stays in `unimplemented`; and,
//! in a read, every WS_TYPE and mapping but the few below, each
//! recorded in the missing-features ledger with the number it was
//! handed, so a program that needs more says which.

#![no_std]
#![allow(non_snake_case)]
// The exported functions take the caller's pointers; each dereference
// carries its own SAFETY comment.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;

use gala_heap as _;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

mod reader;
mod unimplemented;

// The x64 build's memory routines come from gala's ntdll (as other
// DLLs on `alloc` take them).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

pub(crate) type HResult = i32;

pub(crate) const S_OK: HResult = 0;
pub(crate) const E_INVALIDARG: HResult = 0x8007_0057u32 as HResult;
// winerror.h's WS_* codes, the ones gala's reader answers with.
pub(crate) const WS_E_INVALID_FORMAT: HResult = 0x803d_0000u32 as HResult;
pub(crate) const WS_E_OBJECT_FAULTED: HResult = 0x803d_0001u32 as HResult;
pub(crate) const WS_E_NUMERIC_OVERFLOW: HResult = 0x803d_0002u32 as HResult;
pub(crate) const WS_E_INVALID_OPERATION: HResult = 0x803d_0003u32 as HResult;
pub(crate) const WS_E_QUOTA_EXCEEDED: HResult = 0x803d_0008u32 as HResult;
pub(crate) const WS_E_NOT_SUPPORTED: HResult = 0x803d_0017u32 as HResult;

/// The ledger's "not implemented".
pub(crate) const NOT_IMPLEMENTED: u32 = 3;
/// The ledger's "other".
pub(crate) const OTHER: u32 = 5;

/// Reports what this DLL was asked for and cannot do to the
/// missing-features ledger, as `webservices.dll!<function>: <note>`
/// with the number it was handed (a WS_TYPE, a mapping, an input kind),
/// so the ledger names what a program needs next.
pub(crate) fn record(function: &[u8], note: &[u8], kind: u32, detail: u64) {
    let mut text = [0u8; 128];
    let mut n = 0;
    for part in [&b"webservices.dll!"[..], function, b": ", note] {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer of `n` bytes.
    unsafe { GalaRecordMissing(text.as_ptr(), n, kind, detail) };
}

/// An object gala handed out as a handle: its kind, then the object
/// itself. The kind is checked on every call, so a handle of the wrong
/// kind (or one from another library) is reported and refused instead
/// of read as this kind and acted on.
#[repr(C)]
pub(crate) struct Object<T> {
    kind: u32,
    body: T,
}

pub(crate) const HEAP: u32 = 0x5753_0001;
pub(crate) const ERROR: u32 = 0x5753_0002;
pub(crate) const READER: u32 = 0x5753_0003;
pub(crate) const WRITER: u32 = 0x5753_0004;

/// A new object of `kind`, as the handle a caller keeps.
pub(crate) fn new_object<T>(kind: u32, body: T) -> *mut c_void {
    Box::into_raw(Box::new(Object { kind, body })).cast()
}

/// The object a handle names, when the handle is one of ours of that
/// kind.
///
/// # Safety
/// `handle` is null or a handle this DLL gave out and has not freed.
pub(crate) unsafe fn body<'a, T>(
    handle: *mut c_void,
    kind: u32,
    function: &[u8],
) -> Option<&'a mut T> {
    if handle.is_null() {
        return None;
    }
    let object = handle.cast::<Object<T>>();
    // SAFETY: the caller's handle, which the contract says is ours.
    if unsafe { (*object).kind } != kind {
        record(function, b"a handle of another kind", OTHER, 0);
        return None;
    }
    // SAFETY: as above, and the body outlives the handle.
    Some(unsafe { &mut (*object).body })
}

/// Takes an object back from its handle, to drop it.
///
/// # Safety
/// As [`body`]; the handle is not used again.
pub(crate) unsafe fn take<T>(
    handle: *mut c_void,
    kind: u32,
    function: &[u8],
) -> Option<Box<Object<T>>> {
    if handle.is_null() {
        return None;
    }
    let object = handle.cast::<Object<T>>();
    // SAFETY: the caller's handle, which the contract says is ours.
    if unsafe { (*object).kind } != kind {
        record(function, b"a handle of another kind", OTHER, 0);
        return None;
    }
    // SAFETY: the handle came from `new_object`, so it is this Box.
    Some(unsafe { Box::from_raw(object) })
}

/// A WS_HEAP: the memory the reads hand back, which the program keeps
/// until the heap is freed or reset.
pub(crate) struct Heap {
    /// What the heap has handed out, in blocks of 8-byte words so that
    /// every block is aligned for the values a read puts in it (a
    /// string's UTF-16 units among them). Each block is an allocation
    /// of its own, so a block handed out stays where it is however many
    /// more are asked for.
    blocks: Vec<Box<[u64]>>,
    /// The size the heap was created with, as WS_HEAP_PROPERTY_MAX_SIZE
    /// reports it and as the total it allows.
    max: usize,
    used: usize,
}

impl Heap {
    /// `n` bytes of the heap, or None when they would take it over the
    /// size it was created with (WS_E_QUOTA_EXCEEDED to the caller).
    pub(crate) fn alloc(&mut self, n: usize) -> Option<*mut u8> {
        if self.used.saturating_add(n) > self.max {
            return None;
        }
        self.used += n;
        let words = n.div_ceil(size_of::<u64>()).max(1);
        let mut block: Box<[u64]> = alloc::vec![0u64; words].into_boxed_slice();
        let p = block.as_mut_ptr().cast::<u8>();
        self.blocks.push(block);
        Some(p)
    }
}

/// A WS_ERROR: what failed, as the strings WsGetErrorString gives.
/// Newest first, which is the order Windows' own reader leaves them in
/// (webservices_test's `faulted` section measured three strings with
/// the last failure's at index 0).
pub(crate) struct Error {
    strings: Vec<Vec<u16>>,
    /// WS_ERROR_PROPERTY_ORIGINAL_ERROR_CODE: the error the failure
    /// came from, 0 when it came from no other.
    original: u32,
    /// WS_ERROR_PROPERTY_LANGID: the language of the strings. gala's
    /// are English, so 0x0409.
    langid: u16,
}

impl Error {
    /// Records what failed, as the string a program then reads.
    pub(crate) fn push(&mut self, text: &str) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        self.strings.insert(0, units);
    }
}

/// WS_ERROR_PROPERTY_ID (WebServices.h).
const WS_ERROR_PROPERTY_STRING_COUNT: u32 = 0;
const WS_ERROR_PROPERTY_ORIGINAL_ERROR_CODE: u32 = 1;
const WS_ERROR_PROPERTY_LANGID: u32 = 2;

/// WebServices.h's WS_STRING: a UTF-16 run and its length in
/// characters.
#[repr(C)]
pub struct WsString {
    length: u32,
    chars: *mut u16,
}

/// WsCreateHeap: a heap of at most `max_size` bytes. `trim_size` is the
/// amount Windows keeps allocated between resets; gala frees each block
/// as it goes, so it has nothing to trim and the value is accepted and
/// ignored.
#[unsafe(no_mangle)]
pub extern "system" fn WsCreateHeap(
    max_size: usize,
    _trim_size: usize,
    properties: *const c_void,
    property_count: u32,
    heap: *mut *mut c_void,
    error: *mut c_void,
) -> HResult {
    if heap.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 && properties.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 {
        record(b"WsCreateHeap", b"heap properties", NOT_IMPLEMENTED, u64::from(property_count));
    }
    let _ = error;
    let object = new_object(HEAP, Heap { blocks: Vec::new(), max: max_size, used: 0 });
    // SAFETY: the caller's handle, not null.
    unsafe { *heap = object };
    S_OK
}

/// WsFreeHeap: the heap and everything it handed out.
#[unsafe(no_mangle)]
pub extern "system" fn WsFreeHeap(heap: *mut c_void) {
    // SAFETY: a handle this DLL gave out, which the caller is done with.
    drop(unsafe { take::<Heap>(heap, HEAP, b"WsFreeHeap") });
}

/// WsResetHeap: frees what the heap handed out, keeping the heap.
#[unsafe(no_mangle)]
pub extern "system" fn WsResetHeap(heap: *mut c_void, _error: *mut c_void) -> HResult {
    // SAFETY: as above.
    let Some(heap) = (unsafe { body::<Heap>(heap, HEAP, b"WsResetHeap") }) else {
        return E_INVALIDARG;
    };
    heap.blocks = Vec::new();
    heap.used = 0;
    S_OK
}

/// WsCreateError: an error object, which every other call fills in when
/// it fails.
#[unsafe(no_mangle)]
pub extern "system" fn WsCreateError(
    properties: *const c_void,
    property_count: u32,
    error: *mut *mut c_void,
) -> HResult {
    if error.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 && properties.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 {
        record(b"WsCreateError", b"error properties", NOT_IMPLEMENTED, u64::from(property_count));
    }
    let object = new_object(ERROR, Error { strings: Vec::new(), original: 0, langid: 0x0409 });
    // SAFETY: the caller's handle, not null.
    unsafe { *error = object };
    S_OK
}

/// WsFreeError: the error object and its strings.
#[unsafe(no_mangle)]
pub extern "system" fn WsFreeError(error: *mut c_void) {
    // SAFETY: a handle this DLL gave out, which the caller is done with.
    drop(unsafe { take::<Error>(error, ERROR, b"WsFreeError") });
}

/// WsResetError: forgets what failed, keeping the object.
#[unsafe(no_mangle)]
pub extern "system" fn WsResetError(error: *mut c_void) -> HResult {
    // SAFETY: as above.
    let Some(error) = (unsafe { body::<Error>(error, ERROR, b"WsResetError") }) else {
        return E_INVALIDARG;
    };
    error.strings = Vec::new();
    error.original = 0;
    S_OK
}

/// WsGetErrorProperty: one property of the error object. The buffer
/// must be exactly the property's own size, as Windows requires (a
/// 2-byte buffer for the 4-byte string count is E_INVALIDARG, and the
/// buffer is left alone).
#[unsafe(no_mangle)]
pub extern "system" fn WsGetErrorProperty(
    error: *mut c_void,
    id: u32,
    buffer: *mut c_void,
    buffer_size: u32,
) -> HResult {
    if buffer.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a handle this DLL gave out.
    let Some(error) = (unsafe { body::<Error>(error, ERROR, b"WsGetErrorProperty") }) else {
        return E_INVALIDARG;
    };
    match id {
        WS_ERROR_PROPERTY_STRING_COUNT | WS_ERROR_PROPERTY_ORIGINAL_ERROR_CODE => {
            if buffer_size != 4 {
                return E_INVALIDARG;
            }
            let value = if id == WS_ERROR_PROPERTY_STRING_COUNT {
                error.strings.len() as u32
            } else {
                error.original
            };
            // SAFETY: the caller's buffer, of the four bytes it says.
            unsafe { buffer.cast::<u32>().write_unaligned(value) };
            S_OK
        }
        WS_ERROR_PROPERTY_LANGID => {
            if buffer_size != 2 {
                return E_INVALIDARG;
            }
            // SAFETY: the caller's buffer, of the two bytes it says.
            unsafe { buffer.cast::<u16>().write_unaligned(error.langid) };
            S_OK
        }
        _ => {
            record(b"WsGetErrorProperty", b"error property", NOT_IMPLEMENTED, u64::from(id));
            E_INVALIDARG
        }
    }
}

/// WsGetErrorString: one of the strings describing what failed, newest
/// first. Past the last one is E_INVALIDARG and the WS_STRING is left
/// as it was (measured on Windows, webservices_test).
#[unsafe(no_mangle)]
pub extern "system" fn WsGetErrorString(
    error: *mut c_void,
    index: u32,
    string: *mut WsString,
) -> HResult {
    if string.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a handle this DLL gave out.
    let Some(error) = (unsafe { body::<Error>(error, ERROR, b"WsGetErrorString") }) else {
        return E_INVALIDARG;
    };
    let Some(units) = error.strings.get_mut(index as usize) else {
        return E_INVALIDARG;
    };
    // The run is the string without its terminator; it lives as long as
    // the error object, which is what the caller may rely on.
    let value = WsString { length: units.len() as u32 - 1, chars: units.as_mut_ptr() };
    // SAFETY: the caller's WS_STRING, not null.
    unsafe { string.write(value) };
    S_OK
}
