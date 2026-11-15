//! The WS_XML_READER: a document read node by node, which is how the
//! Office Deployment Tool's setup.exe reads the configuration service's
//! answer. The nodes come from gala's XML reader (`gala_xml`'s `Scan`),
//! so the reader hands out what it has read and fails at the node that
//! is malformed, as Windows' reader does.
//!
//! The writer is an object and nothing more: setup.exe creates one and
//! frees it, and the write functions are not among its imports, so
//! there is nothing yet for a writer to do.

use alloc::string::String;
use alloc::vec::Vec;
use core::char::decode_utf16;
use core::ffi::c_void;
use core::slice;

use gala_xml::{Event, Scan};

use crate::{
    E_INVALIDARG, ERROR, Error, HEAP, HResult, Heap, NOT_IMPLEMENTED, OTHER, READER, S_OK, WRITER,
    WS_E_INVALID_FORMAT, WS_E_INVALID_OPERATION, WS_E_NOT_SUPPORTED, WS_E_NUMERIC_OVERFLOW,
    WS_E_OBJECT_FAULTED, WS_E_QUOTA_EXCEEDED, body, new_object, record,
};

// WebServices.h's enumerations, the members the reader knows.
const WS_XML_READER_INPUT_TYPE_BUFFER: u32 = 1;
const WS_XML_READER_ENCODING_TYPE_TEXT: u32 = 1;
const WS_CHARSET_AUTO: u32 = 0;
const WS_CHARSET_UTF8: u32 = 1;
const WS_CHARSET_UTF16LE: u32 = 2;
const WS_CHARSET_UTF16BE: u32 = 3;
const WS_ELEMENT_TYPE_MAPPING: u32 = 1;
const WS_UINT32_TYPE: u32 = 7;
const WS_WSZ_TYPE: u32 = 17;
const WS_READ_REQUIRED_VALUE: u32 = 1;
const WS_READ_REQUIRED_POINTER: u32 = 2;

const TRUE: i32 = 1;
const FALSE: i32 = 0;

/// WebServices.h's WS_XML_STRING: UTF-8 bytes, with a dictionary and an
/// id gala's reader does not use (it compares the bytes).
#[repr(C)]
pub struct WsXmlString {
    length: u32,
    bytes: *const u8,
    dictionary: *const c_void,
    id: u32,
}

/// WebServices.h's WS_XML_READER_ENCODING and the text encoding that
/// extends it.
#[repr(C)]
struct WsXmlReaderTextEncoding {
    encoding_type: u32,
    char_set: u32,
}

/// WebServices.h's WS_XML_READER_INPUT and the buffer input that
/// extends it.
#[repr(C)]
struct WsXmlReaderBufferInput {
    input_type: u32,
    encoded_data: *const c_void,
    encoded_data_size: u32,
}

/// The node a reader is on.
enum Node {
    /// A start tag, its namespace and local name.
    Start { ns: String, name: String },
    /// Character data or a CDATA section.
    Text(String),
    /// The end of the element the reader was in.
    End,
    /// Past the root element.
    Done,
}

/// A WS_XML_READER: the document, where the scan has got to, and the
/// node the reader is on.
pub(crate) struct Reader {
    /// The document as text. The input's bytes are the caller's, so the
    /// reader decodes them into its own copy when the input is set
    /// rather than reading from memory it does not own.
    text: String,
    scan: Scan,
    /// None before the first read, and again after a read that
    /// consumed a node: the next call pulls.
    node: Option<Node>,
    /// Whether an input has been set at all.
    ready: bool,
    /// A read met a malformed document. Every later call on this reader
    /// answers WS_E_OBJECT_FAULTED, as Windows' reader does.
    faulted: bool,
}

/// A WS_XML_WRITER: an object with nothing to write yet.
pub(crate) struct Writer;

/// Describes a failure in the caller's error object, when it gave one.
fn fail(error: *mut c_void, function: &[u8], text: &str) {
    // SAFETY: a handle this DLL gave out, or null.
    if let Some(error) = unsafe { body::<Error>(error, ERROR, function) } {
        error.push(text);
    }
}

impl Reader {
    /// The next node, comments and processing instructions skipped. A
    /// malformed document faults the reader and is described in the
    /// error object.
    fn pull(&mut self, error: *mut c_void, function: &[u8]) -> HResult {
        loop {
            match self.scan.next_event(&self.text) {
                Ok(Event::Comment(_) | Event::Pi { .. }) => {}
                Ok(Event::StartElement { ns, name, .. }) => {
                    self.node = Some(Node::Start { ns, name });
                    return S_OK;
                }
                Ok(Event::EndElement) => {
                    self.node = Some(Node::End);
                    return S_OK;
                }
                Ok(Event::Text(text)) => {
                    self.node = Some(Node::Text(text));
                    return S_OK;
                }
                Ok(Event::End) => {
                    self.node = Some(Node::Done);
                    return S_OK;
                }
                Err(e) => {
                    self.faulted = true;
                    fail(error, function, &alloc::format!("The document is not well-formed: {e}"));
                    return WS_E_INVALID_FORMAT;
                }
            }
        }
    }

    /// The node the reader is on, pulling one when it is between nodes
    /// and skipping the white space between markup.
    fn settle(&mut self, error: *mut c_void, function: &[u8]) -> HResult {
        loop {
            match &self.node {
                None => {}
                Some(Node::Text(text)) if text.trim().is_empty() => {}
                Some(_) => return S_OK,
            }
            let hr = self.pull(error, function);
            if hr != S_OK {
                return hr;
            }
        }
    }

    /// What every call answers before doing anything: a reader with no
    /// input cannot read, and a faulted one cannot do anything more.
    fn usable(&self, error: *mut c_void, function: &[u8]) -> HResult {
        if self.faulted {
            fail(
                error,
                function,
                "The operation could not be completed because the reader is in a faulted state.",
            );
            return WS_E_OBJECT_FAULTED;
        }
        if !self.ready {
            fail(error, function, "The reader has no input set.");
            return WS_E_INVALID_OPERATION;
        }
        S_OK
    }
}

/// A WS_XML_STRING as text, or None when it is null or its bytes are
/// not UTF-8 (gala's reader compares names as text).
///
/// # Safety
/// `p` is null or the caller's WS_XML_STRING, whose bytes are its own
/// length long.
unsafe fn name_of<'a>(p: *const WsXmlString) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller's WS_XML_STRING.
    let s = unsafe { &*p };
    if s.bytes.is_null() {
        return None;
    }
    // SAFETY: as the contract says, `length` bytes at `bytes`.
    core::str::from_utf8(unsafe { slice::from_raw_parts(s.bytes, s.length as usize) }).ok()
}

/// The input's bytes as text: UTF-8 or UTF-16, by what the encoding
/// says, with `WS_CHARSET_AUTO` (and UTF-8, which may carry a byte
/// order mark) left to gala's XML reader to tell apart.
fn decode(bytes: &[u8], char_set: u32) -> Option<String> {
    match char_set {
        WS_CHARSET_AUTO | WS_CHARSET_UTF8 => gala_xml::decode(bytes).ok(),
        WS_CHARSET_UTF16LE | WS_CHARSET_UTF16BE => {
            if !bytes.len().is_multiple_of(2) {
                return None;
            }
            let units = bytes.chunks_exact(2).map(|p| {
                if char_set == WS_CHARSET_UTF16LE {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            });
            let text: Result<String, _> = decode_utf16(units).collect();
            // Line ends are normalized as gala's XML reader does for the
            // text it decodes itself (XML 1.0 section 2.11).
            text.ok().map(|t| t.replace("\r\n", "\n").replace('\r', "\n"))
        }
        _ => None,
    }
}

/// WsCreateReader: a reader with no input yet.
#[unsafe(no_mangle)]
pub extern "system" fn WsCreateReader(
    properties: *const c_void,
    property_count: u32,
    reader: *mut *mut c_void,
    _error: *mut c_void,
) -> HResult {
    if reader.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 && properties.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 {
        record(b"WsCreateReader", b"reader properties", NOT_IMPLEMENTED, u64::from(property_count));
    }
    let object = new_object(
        READER,
        Reader { text: String::new(), scan: Scan::new(), node: None, ready: false, faulted: false },
    );
    // SAFETY: the caller's handle, not null.
    unsafe { *reader = object };
    S_OK
}

/// WsFreeReader: the reader and its copy of the document.
#[unsafe(no_mangle)]
pub extern "system" fn WsFreeReader(reader: *mut c_void) {
    // SAFETY: a handle this DLL gave out, which the caller is done with.
    drop(unsafe { crate::take::<Reader>(reader, READER, b"WsFreeReader") });
}

/// WsCreateWriter: a writer object.
#[unsafe(no_mangle)]
pub extern "system" fn WsCreateWriter(
    properties: *const c_void,
    property_count: u32,
    writer: *mut *mut c_void,
    _error: *mut c_void,
) -> HResult {
    if writer.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 && properties.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 {
        record(b"WsCreateWriter", b"writer properties", NOT_IMPLEMENTED, u64::from(property_count));
    }
    // SAFETY: the caller's handle, not null.
    unsafe { *writer = new_object(WRITER, Writer) };
    S_OK
}

/// WsFreeWriter: the writer object.
#[unsafe(no_mangle)]
pub extern "system" fn WsFreeWriter(writer: *mut c_void) {
    // SAFETY: a handle this DLL gave out, which the caller is done with.
    drop(unsafe { crate::take::<Writer>(writer, WRITER, b"WsFreeWriter") });
}

/// WsSetInput: the document the reader reads, and how its bytes are
/// encoded. gala's reader copies the buffer as text here, so a buffer
/// whose encoding it cannot decode is refused now rather than at the
/// first read.
#[unsafe(no_mangle)]
pub extern "system" fn WsSetInput(
    reader: *mut c_void,
    encoding: *const c_void,
    input: *const c_void,
    properties: *const c_void,
    property_count: u32,
    error: *mut c_void,
) -> HResult {
    if encoding.is_null() || input.is_null() {
        return E_INVALIDARG;
    }
    if property_count != 0 && properties.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsSetInput") }) else {
        return E_INVALIDARG;
    };
    if property_count != 0 {
        record(b"WsSetInput", b"reader properties", NOT_IMPLEMENTED, u64::from(property_count));
    }

    // SAFETY: the caller's WS_XML_READER_ENCODING; its type says which
    // encoding extends it, and only the text one is read further.
    let encoding_type = unsafe { encoding.cast::<u32>().read_unaligned() };
    if encoding_type != WS_XML_READER_ENCODING_TYPE_TEXT {
        record(b"WsSetInput", b"reader encoding", NOT_IMPLEMENTED, u64::from(encoding_type));
        return WS_E_NOT_SUPPORTED;
    }
    // SAFETY: the type above says this is a WS_XML_READER_TEXT_ENCODING.
    let char_set = unsafe { &*encoding.cast::<WsXmlReaderTextEncoding>() }.char_set;

    // SAFETY: the caller's WS_XML_READER_INPUT, as above.
    let input_type = unsafe { input.cast::<u32>().read_unaligned() };
    if input_type != WS_XML_READER_INPUT_TYPE_BUFFER {
        record(b"WsSetInput", b"reader input", NOT_IMPLEMENTED, u64::from(input_type));
        return WS_E_NOT_SUPPORTED;
    }
    // SAFETY: the type above says this is a WS_XML_READER_BUFFER_INPUT.
    let buffer = unsafe { &*input.cast::<WsXmlReaderBufferInput>() };
    if buffer.encoded_data.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's buffer, of the size it gave.
    let bytes = unsafe {
        slice::from_raw_parts(buffer.encoded_data.cast::<u8>(), buffer.encoded_data_size as usize)
    };
    let Some(text) = decode(bytes, char_set) else {
        record(b"WsSetInput", b"character set", OTHER, u64::from(char_set));
        fail(error, b"WsSetInput", "The input could not be decoded.");
        return WS_E_INVALID_FORMAT;
    };

    reader.text = text;
    reader.scan = Scan::new();
    reader.node = None;
    reader.ready = true;
    reader.faulted = false;
    S_OK
}

/// WsFillReader: makes sure `min_size` bytes of the input are there to
/// read. A buffer input is the whole document, which gala's reader has
/// already copied, so there is never anything to wait for.
#[unsafe(no_mangle)]
pub extern "system" fn WsFillReader(
    reader: *mut c_void,
    _min_size: u32,
    async_context: *const c_void,
    error: *mut c_void,
) -> HResult {
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsFillReader") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsFillReader");
    if hr != S_OK {
        return hr;
    }
    if !async_context.is_null() {
        // The fill is already done, so there is nothing to run
        // asynchronously; the call still answers synchronously, which a
        // caller that passed a context must handle.
        record(b"WsFillReader", b"asynchronous fill", NOT_IMPLEMENTED, 0);
    }
    S_OK
}

/// WsReadToStartElement: moves to the next start element, skipping the
/// white space, comments and processing instructions before it. With
/// `found`, an element that is not there is FALSE and S_OK; without
/// one, it is WS_E_INVALID_FORMAT.
///
/// A null `ns` matches an element in any namespace, and is recorded:
/// which of the two readings Windows takes (any namespace, or no
/// namespace) is not measured, so the ledger names the guess if a
/// program depends on it.
#[unsafe(no_mangle)]
pub extern "system" fn WsReadToStartElement(
    reader: *mut c_void,
    local_name: *const WsXmlString,
    ns: *const WsXmlString,
    found: *mut i32,
    error: *mut c_void,
) -> HResult {
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsReadToStartElement") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsReadToStartElement");
    if hr != S_OK {
        return hr;
    }
    // SAFETY: the caller's strings, or null.
    let (want_name, want_ns) = unsafe { (name_of(local_name), name_of(ns)) };
    if !local_name.is_null() && want_name.is_none() {
        return E_INVALIDARG;
    }
    if ns.is_null() {
        record(b"WsReadToStartElement", b"a null namespace", OTHER, 0);
    }

    let hr = reader.settle(error, b"WsReadToStartElement");
    if hr != S_OK {
        return hr;
    }
    let matched = match &reader.node {
        Some(Node::Start { ns, name }) => {
            want_name.is_none_or(|want| want == name) && want_ns.is_none_or(|want| want == ns)
        }
        _ => false,
    };
    if found.is_null() {
        if !matched {
            fail(error, b"WsReadToStartElement", "The element was not found.");
            return WS_E_INVALID_FORMAT;
        }
        return S_OK;
    }
    // SAFETY: the caller's BOOL, not null.
    unsafe { *found = if matched { TRUE } else { FALSE } };
    S_OK
}

/// WsReadStartElement: moves past the start tag the reader is on, into
/// the element's content.
#[unsafe(no_mangle)]
pub extern "system" fn WsReadStartElement(reader: *mut c_void, error: *mut c_void) -> HResult {
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsReadStartElement") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsReadStartElement");
    if hr != S_OK {
        return hr;
    }
    let hr = reader.settle(error, b"WsReadStartElement");
    if hr != S_OK {
        return hr;
    }
    if !matches!(reader.node, Some(Node::Start { .. })) {
        fail(error, b"WsReadStartElement", "The reader is not on a start element.");
        return WS_E_INVALID_FORMAT;
    }
    reader.pull(error, b"WsReadStartElement")
}

/// WsReadType: the element the reader is on, read whole as a value of
/// `ws_type` (WS_ELEMENT_TYPE_MAPPING: its start tag, its text and its
/// end tag), leaving the reader after its end tag.
///
/// The types gala reads are the two the Office Deployment Tool's answer
/// is made of: WS_WSZ_TYPE as a NUL-terminated string on the heap, and
/// WS_UINT32_TYPE as a number. Anything else is recorded in the ledger
/// with the number it was handed and refused with WS_E_NOT_SUPPORTED,
/// so the ledger names the next type a program needs.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn WsReadType(
    reader: *mut c_void,
    type_mapping: u32,
    ws_type: u32,
    type_description: *const c_void,
    read_option: u32,
    heap: *mut c_void,
    value: *mut c_void,
    value_size: u32,
    error: *mut c_void,
) -> HResult {
    if value.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsReadType") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsReadType");
    if hr != S_OK {
        return hr;
    }
    if type_mapping != WS_ELEMENT_TYPE_MAPPING {
        record(b"WsReadType", b"type mapping", NOT_IMPLEMENTED, u64::from(type_mapping));
        return WS_E_NOT_SUPPORTED;
    }
    if !type_description.is_null() {
        record(b"WsReadType", b"a type description", NOT_IMPLEMENTED, u64::from(ws_type));
        return WS_E_NOT_SUPPORTED;
    }
    let wanted = match (ws_type, read_option) {
        (WS_WSZ_TYPE, WS_READ_REQUIRED_POINTER) => size_of::<*mut u16>() as u32,
        (WS_UINT32_TYPE, WS_READ_REQUIRED_VALUE) => 4,
        (WS_WSZ_TYPE | WS_UINT32_TYPE, _) => {
            record(b"WsReadType", b"read option", NOT_IMPLEMENTED, u64::from(read_option));
            return WS_E_NOT_SUPPORTED;
        }
        _ => {
            record(b"WsReadType", b"value type", NOT_IMPLEMENTED, u64::from(ws_type));
            return WS_E_NOT_SUPPORTED;
        }
    };
    if value_size != wanted {
        return E_INVALIDARG;
    }

    let hr = reader.settle(error, b"WsReadType");
    if hr != S_OK {
        return hr;
    }
    if !matches!(reader.node, Some(Node::Start { .. })) {
        fail(error, b"WsReadType", "The reader is not on a start element.");
        return WS_E_INVALID_FORMAT;
    }

    // The element's text, to its end tag. A child element is not part
    // of a string or a number, so an element that has one does not hold
    // a value of these types.
    let mut text = String::new();
    loop {
        let hr = reader.pull(error, b"WsReadType");
        if hr != S_OK {
            return hr;
        }
        match reader.node.take() {
            Some(Node::Text(run)) => text.push_str(&run),
            Some(Node::End) => break,
            _ => {
                fail(error, b"WsReadType", "The element does not hold a value of this type.");
                return WS_E_INVALID_FORMAT;
            }
        }
    }
    // Past the element's end tag: the next call pulls the node after it.
    reader.node = None;

    match ws_type {
        WS_WSZ_TYPE => {
            // SAFETY: a handle this DLL gave out.
            let Some(heap) = (unsafe { body::<Heap>(heap, HEAP, b"WsReadType") }) else {
                return E_INVALIDARG;
            };
            let mut units: Vec<u16> = text.encode_utf16().collect();
            units.push(0);
            let bytes = units.len() * size_of::<u16>();
            let Some(block) = heap.alloc(bytes) else {
                fail(error, b"WsReadType", "The heap has no room for the string.");
                return WS_E_QUOTA_EXCEEDED;
            };
            // SAFETY: `block` is `bytes` bytes of the heap, which is as
            // long as the string's units; the heap's blocks are blocks
            // of 8-byte words, so it is aligned for u16 as well.
            unsafe {
                core::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), block, bytes);
                value.cast::<*mut u16>().write_unaligned(block.cast::<u16>());
            }
            S_OK
        }
        _ => {
            let digits = text.trim();
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                fail(error, b"WsReadType", "The element does not hold a number.");
                return WS_E_INVALID_FORMAT;
            }
            let Ok(number) = digits.parse::<u32>() else {
                fail(error, b"WsReadType", "The number does not fit in 32 bits.");
                return WS_E_NUMERIC_OVERFLOW;
            };
            // SAFETY: the caller's buffer, of the four bytes it said.
            unsafe { value.cast::<u32>().write_unaligned(number) };
            S_OK
        }
    }
}
