//! webservices.dll's XML reader, which the Office Deployment Tool's
//! setup.exe delay-loads to parse the Office configuration service's
//! answer (ROADMAP OF). setup.exe's delay imports are the reader and
//! error subset alone -- no channel, no service proxy, no security
//! binding: WsCreateError, WsCreateHeap, WsCreateReader, WsCreateWriter,
//! WsFillReader, WsFreeError, WsFreeHeap, WsFreeReader, WsFreeWriter,
//! WsGetErrorProperty, WsGetErrorString, WsReadStartElement,
//! WsReadToStartElement, WsReadType and WsSetInput.
//!
//! The declarations and constants are those of the Windows SDK's
//! WebServices.h (10.0.26100.0, the pinned build's), and every expected
//! value is what real Windows answers (this program's run on CI's
//! runner and on the owner's machine).
//!
//! The same source builds `wow64_webservices_test`, the 32-bit (x86)
//! program. Prints `webservices_test: ok`, or `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "webservices_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_webservices_test";

const CRLF: &[u8] = &[13, 10];

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

// WebServices.h: the reader and error functions setup.exe imports.
#[cfg_attr(
    target_arch = "x86",
    link(name = "webservices", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "webservices", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WsCreateHeap(
        max_size: usize,
        trim_size: usize,
        properties: *const c_void,
        property_count: u32,
        heap: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeHeap(heap: *mut c_void);
    fn WsCreateError(
        properties: *const c_void,
        property_count: u32,
        error: *mut *mut c_void,
    ) -> i32;
    fn WsFreeError(error: *mut c_void);
    fn WsGetErrorProperty(
        error: *mut c_void,
        id: u32,
        buffer: *mut c_void,
        buffer_size: u32,
    ) -> i32;
    fn WsGetErrorString(error: *mut c_void, index: u32, string: *mut WsString) -> i32;
    fn WsCreateReader(
        properties: *const c_void,
        property_count: u32,
        reader: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeReader(reader: *mut c_void);
    fn WsCreateWriter(
        properties: *const c_void,
        property_count: u32,
        writer: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeWriter(writer: *mut c_void);
    fn WsSetInput(
        reader: *mut c_void,
        encoding: *const c_void,
        input: *const c_void,
        properties: *const c_void,
        property_count: u32,
        error: *mut c_void,
    ) -> i32;
    fn WsFillReader(
        reader: *mut c_void,
        min_size: u32,
        async_context: *const c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsReadToStartElement(
        reader: *mut c_void,
        local_name: *const WsXmlString,
        ns: *const WsXmlString,
        found: *mut i32,
        error: *mut c_void,
    ) -> i32;
    fn WsReadStartElement(reader: *mut c_void, error: *mut c_void) -> i32;
    fn WsReadType(
        reader: *mut c_void,
        type_mapping: u32,
        ws_type: u32,
        type_description: *const c_void,
        read_option: u32,
        heap: *mut c_void,
        value: *mut c_void,
        value_size: u32,
        error: *mut c_void,
    ) -> i32;
}

/// WebServices.h's WS_STRING: a UTF-16 run and its length in characters.
#[repr(C)]
#[derive(Clone, Copy)]
struct WsString {
    length: u32,
    chars: *mut u16,
}

/// WebServices.h's WS_XML_STRING: UTF-8 bytes, with the dictionary a
/// non-dictionary string leaves NULL.
#[repr(C)]
struct WsXmlString {
    length: u32,
    bytes: *const u8,
    dictionary: *const c_void,
    id: u32,
}

impl WsXmlString {
    const fn new(bytes: &'static [u8]) -> Self {
        Self { length: bytes.len() as u32, bytes: bytes.as_ptr(), dictionary: null(), id: 0 }
    }
}

/// WebServices.h's WS_XML_READER_TEXT_ENCODING: the base
/// WS_XML_READER_ENCODING (its type) and the character set.
#[repr(C)]
struct WsXmlReaderTextEncoding {
    encoding_type: u32,
    char_set: u32,
}

/// WebServices.h's WS_XML_READER_BUFFER_INPUT: the base
/// WS_XML_READER_INPUT (its type) and the bytes to read.
#[repr(C)]
struct WsXmlReaderBufferInput {
    input_type: u32,
    encoded_data: *const c_void,
    encoded_data_size: u32,
}

// WebServices.h's enumerations, the members this program uses.
const WS_XML_READER_INPUT_TYPE_BUFFER: u32 = 1;
const WS_XML_READER_ENCODING_TYPE_TEXT: u32 = 1;
const WS_CHARSET_UTF8: u32 = 1;
const WS_ERROR_PROPERTY_STRING_COUNT: u32 = 0;
const WS_ELEMENT_TYPE_MAPPING: u32 = 1;
const WS_UINT32_TYPE: u32 = 7;
const WS_WSZ_TYPE: u32 = 17;
const WS_READ_REQUIRED_VALUE: u32 = 1;
const WS_READ_REQUIRED_POINTER: u32 = 2;

const S_OK: i32 = 0;

/// One line of output, written with a single WriteFile: the kernel log
/// shares the serial port, so a line built from several writes can be
/// cut in half by a log line (WORKFLOW "Test programs").
struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 240], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(mut self, v: u64) -> Self {
        let mut digits = [0u8; 20];
        let mut n = 0;
        let mut v = v;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self = self.s(&[digits[n]]);
        }
        self
    }

    fn hex(mut self, v: u64) -> Self {
        self = self.s(b"0x");
        for i in (0..16).rev() {
            let nibble = ((v >> (i * 4)) & 0xf) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            self = self.s(&[c]);
        }
        self
    }

    fn print(self) {
        let mut written = 0u32;
        // SAFETY: `self.buf` is `self.n` bytes long and the handle is the
        // process's standard output.
        unsafe {
            WriteFile(
                GetStdHandle(STD_OUTPUT_HANDLE),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            );
        }
    }
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

/// The checks, numbered as they run so a failure names itself and the
/// exit status is the check that failed.
struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")")
            .s(CRLF)
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// An HRESULT, printed as the unsigned 32-bit value a reader expects.
    #[track_caller]
    fn hr(&mut self, got: i32, want: i32) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

fn section(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(CRLF).print();
}

// winerror.h and WebServices.h's own codes, the ones this program sees.
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const WS_E_INVALID_FORMAT: i32 = 0x803d_0000u32 as i32;
const WS_E_OBJECT_FAULTED: i32 = 0x803d_0001u32 as i32;

/// A value no call under test returns, so a check that sees it is
/// reading an output the call did not write.
const SENTINEL: u32 = 0xffff_ffff;

/// A document with one text element and one number, the shapes
/// setup.exe's answer is made of.
const DOC: &[u8] = b"<r><a>hello</a><n>42</n></r>";

/// The same, with an end tag that closes the wrong element.
const BAD: &[u8] = b"<r><a></r>";

/// `text`, NUL-terminated, against `want` as ASCII: the index of the
/// first differing unit and that unit, or None when the two are equal.
fn wide_diff(text: *const u16, want: &[u8]) -> Option<(usize, u64)> {
    for (i, &b) in want.iter().enumerate() {
        // SAFETY: a NUL-terminated string at least as long as the match
        // so far; the loop stops at the first unit that differs, so it
        // never reads past the terminator.
        let c = unsafe { *text.add(i) };
        if c != u16::from(b) {
            return Some((i, u64::from(c)));
        }
    }
    // SAFETY: every unit before this one matched `want`, so the string
    // is at least this long.
    let end = unsafe { *text.add(want.len()) };
    if end != 0 {
        return Some((want.len(), u64::from(end)));
    }
    None
}

/// A failing `wide_diff` as one number to report: the index above the
/// unit that differs.
fn diff_code(diff: Option<(usize, u64)>) -> u64 {
    diff.map_or(0, |(i, c)| ((i as u64) << 32) | c)
}

/// A heap and an error object, the two things every other call takes.
fn objects(c: &mut Checks) -> Result<(*mut c_void, *mut c_void), u32> {
    let mut heap: *mut c_void = null_mut();
    let mut error: *mut c_void = null_mut();

    // SAFETY: the shapes WebServices.h declares; this program owns both
    // objects and frees them at the end of `run`.
    unsafe {
        c.hr(WsCreateHeap(1 << 16, 0, null(), 0, &mut heap, null_mut()), S_OK)?;
        c.ok(!heap.is_null(), 0)?;
        // No heap to write the handle to is a bad argument, not a crash.
        c.hr(WsCreateHeap(1 << 16, 0, null(), 0, null_mut(), null_mut()), E_INVALIDARG)?;

        c.hr(WsCreateError(null(), 0, &mut error), S_OK)?;
        c.ok(!error.is_null(), 0)?;
    }
    Ok((heap, error))
}

/// A fresh error object carries no strings, and asking for one says so.
fn errors(c: &mut Checks, error: *mut c_void) -> Result<(), u32> {
    let mut count = SENTINEL;
    let mut string = WsString { length: SENTINEL, chars: null_mut() };

    // SAFETY: `count` and `string` are this frame's, and the sizes
    // passed are their own.
    unsafe {
        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 4),
            S_OK,
        )?;
        c.eq(u64::from(count), 0)?;
        // A buffer too small for the property type is a bad argument.
        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 2),
            E_INVALIDARG,
        )?;
        c.eq(u64::from(count), 0)?;
        // String 0 of none: a bad argument, and the WS_STRING is left as
        // it was.
        c.hr(WsGetErrorString(error, 0, &raw mut string), E_INVALIDARG)?;
        c.eq(u64::from(string.length), u64::from(SENTINEL))?;
        c.ok(string.chars.is_null(), 0)?;
    }
    Ok(())
}

/// Reading the document: to an element, into it, and its content as a
/// string and as a number -- the sequence setup.exe reads its answer
/// with.
fn reading(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let encoding = WsXmlReaderTextEncoding {
        encoding_type: WS_XML_READER_ENCODING_TYPE_TEXT,
        char_set: WS_CHARSET_UTF8,
    };
    let input = WsXmlReaderBufferInput {
        input_type: WS_XML_READER_INPUT_TYPE_BUFFER,
        encoded_data: DOC.as_ptr().cast(),
        encoded_data_size: DOC.len() as u32,
    };
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");
    let number_element = WsXmlString::new(b"n");

    let mut reader: *mut c_void = null_mut();
    let mut writer: *mut c_void = null_mut();
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut number = SENTINEL;

    // SAFETY: the reader and writer are this function's and are freed
    // before it returns; `text` is the heap's and outlives the reads.
    unsafe {
        c.hr(WsCreateReader(null(), 0, &mut reader, error), S_OK)?;
        c.ok(!reader.is_null(), 0)?;
        c.hr(WsCreateWriter(null(), 0, &mut writer, error), S_OK)?;
        c.ok(!writer.is_null(), 0)?;

        c.hr(
            WsSetInput(
                reader,
                (&raw const encoding).cast(),
                (&raw const input).cast(),
                null(),
                0,
                error,
            ),
            S_OK,
        )?;
        // The whole document is in the buffer, so a fill of its length
        // is satisfied at once.
        c.hr(WsFillReader(reader, DOC.len() as u32, null(), error), S_OK)?;

        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;

        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        // WS_ELEMENT_TYPE_MAPPING reads the element the reader is on,
        // start tag to end tag, so the string comes back without a
        // WsReadStartElement of its own.
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            S_OK,
        )?;
        c.ok(!text.is_null(), 0)?;
        let diff = wide_diff(text, b"hello");
        c.ok(diff.is_none(), diff_code(diff))?;

        c.hr(WsReadToStartElement(reader, &number_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_UINT32_TYPE,
                null(),
                WS_READ_REQUIRED_VALUE,
                heap,
                (&raw mut number).cast(),
                4,
                error,
            ),
            S_OK,
        )?;
        c.eq(u64::from(number), 42)?;

        WsFreeWriter(writer);
        WsFreeReader(reader);
    }
    Ok(())
}

/// A document that does not close its elements in order: the read that
/// meets the mismatch says so, every later call on that reader says the
/// object is faulted, and the error object carries what happened.
fn faulted(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let encoding = WsXmlReaderTextEncoding {
        encoding_type: WS_XML_READER_ENCODING_TYPE_TEXT,
        char_set: WS_CHARSET_UTF8,
    };
    let input = WsXmlReaderBufferInput {
        input_type: WS_XML_READER_INPUT_TYPE_BUFFER,
        encoded_data: BAD.as_ptr().cast(),
        encoded_data_size: BAD.len() as u32,
    };
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");

    let mut reader: *mut c_void = null_mut();
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut count = SENTINEL;
    let mut string = WsString { length: SENTINEL, chars: null_mut() };

    // SAFETY: as in `reading`; the reader is freed before returning.
    unsafe {
        c.hr(WsCreateReader(null(), 0, &mut reader, error), S_OK)?;
        c.hr(
            WsSetInput(
                reader,
                (&raw const encoding).cast(),
                (&raw const input).cast(),
                null(),
                0,
                error,
            ),
            S_OK,
        )?;
        c.hr(WsFillReader(reader, BAD.len() as u32, null(), error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        // <a> is there to be found; it is reading past it that meets
        // the end tag of r where the end tag of a belongs.
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(WsReadStartElement(reader, error), WS_E_INVALID_FORMAT)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            WS_E_OBJECT_FAULTED,
        )?;

        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 4),
            S_OK,
        )?;
        // Windows left three strings here, one per layer that failed.
        // How many a failure records is the library's own business and
        // the text is localized, so what is checked is that the failure
        // is described at all: a string, with characters in it.
        c.ok(count >= 1, u64::from(count))?;
        c.hr(WsGetErrorString(error, 0, &raw mut string), S_OK)?;
        c.ok(string.length > 0, u64::from(string.length))?;
        c.ok(!string.chars.is_null(), 0)?;
        // Past the last string is a bad argument, as asking a fresh
        // error object was.
        c.hr(WsGetErrorString(error, count + 4, &raw mut string), E_INVALIDARG)?;

        WsFreeReader(reader);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    section(b"objects");
    let (heap, error) = objects(&mut c)?;
    section(b"errors");
    errors(&mut c, error)?;
    section(b"reading");
    reading(&mut c, heap, error)?;
    section(b"faulted");
    faulted(&mut c, heap, error)?;

    // SAFETY: both objects were created above and nothing holds them.
    unsafe {
        WsFreeError(error);
        WsFreeHeap(heap);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok").s(CRLF).print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: the process ends here.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
