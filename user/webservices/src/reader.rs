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
const WS_ELEMENT_CONTENT_TYPE_MAPPING: u32 = 3;
const WS_UINT32_TYPE: u32 = 7;
const WS_WSZ_TYPE: u32 = 17;
const WS_READ_REQUIRED_VALUE: u32 = 1;
const WS_READ_REQUIRED_POINTER: u32 = 2;

const TRUE: i32 = 1;
const FALSE: i32 = 0;

/// WebServices.h's WS_XML_READER_PROPERTY_ID, the ids a reader of
/// gala's answers for. The rest of the enumeration (the MIME and
/// stream settings, ids 7, 8 and 10 to 13) is accepted when a reader is
/// created, as Windows does, and reported in the missing-features
/// ledger; WsGetReaderProperty does not answer for it, which is also
/// what Windows does ("the property ID was not recognized").
const WS_XML_READER_PROPERTY_MAX_DEPTH: u32 = 0;
const WS_XML_READER_PROPERTY_ALLOW_FRAGMENT: u32 = 1;
const WS_XML_READER_PROPERTY_MAX_ATTRIBUTES: u32 = 2;
const WS_XML_READER_PROPERTY_READ_DECLARATION: u32 = 3;
const WS_XML_READER_PROPERTY_CHARSET: u32 = 4;
const WS_XML_READER_PROPERTY_ROW: u32 = 5;
const WS_XML_READER_PROPERTY_COLUMN: u32 = 6;
const WS_XML_READER_PROPERTY_IN_ATTRIBUTE: u32 = 9;
const WS_XML_READER_PROPERTY_MAX_NAMESPACES: u32 = 14;
/// One past the last id WebServices.h names, which Windows refuses as
/// read-only rather than unrecognized.
const WS_XML_READER_PROPERTY_LAST: u32 = 15;

/// What a reader property is: how many bytes its value takes, and
/// whether a reader may be created with it (the positions, the
/// character set and "in an attribute" are what the reader has reached,
/// so Windows refuses them as "not valid for the current operation").
/// Measured on Windows 11 26200 in both widths (webservices_test's
/// `properties`).
struct Property {
    size: u32,
    settable: bool,
    /// Answered by WsGetReaderProperty.
    readable: bool,
}

/// The property `id` is, or None when the id is not one WebServices.h
/// names.
fn property(id: u32) -> Option<Property> {
    let (size, settable, readable) = match id {
        WS_XML_READER_PROPERTY_MAX_DEPTH
        | WS_XML_READER_PROPERTY_ALLOW_FRAGMENT
        | WS_XML_READER_PROPERTY_MAX_ATTRIBUTES
        | WS_XML_READER_PROPERTY_READ_DECLARATION
        | WS_XML_READER_PROPERTY_MAX_NAMESPACES => (4, true, true),
        WS_XML_READER_PROPERTY_CHARSET | WS_XML_READER_PROPERTY_IN_ATTRIBUTE => (4, false, true),
        WS_XML_READER_PROPERTY_ROW | WS_XML_READER_PROPERTY_COLUMN => (8, false, true),
        // The settings gala does not honour: a reader may be created
        // with them and they cannot be read back.
        7 | 8 | 10..=13 => (4, true, false),
        WS_XML_READER_PROPERTY_LAST => (4, false, false),
        _ => return None,
    };
    Some(Property { size, settable, readable })
}

/// The settings a reader was created with, as WsGetReaderProperty
/// reports them back. The defaults are Windows' own, measured on a
/// reader created with no properties at all.
struct Settings {
    /// How deep a start element may be. A start element below it is
    /// WS_E_QUOTA_EXCEEDED and faults the reader.
    max_depth: u32,
    /// More than one root element (gala's XML reader scans a fragment).
    allow_fragment: bool,
    /// Accepted and reported back, but not enforced: gala's reader has
    /// no limit of its own on attributes or namespace bindings, and it
    /// never hands out the XML declaration as a node, so reading it is
    /// the same either way. Each is recorded in the ledger when a
    /// reader is created with it.
    max_attributes: u32,
    max_namespaces: u32,
    read_declaration: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_depth: 32,
            allow_fragment: false,
            max_attributes: 128,
            max_namespaces: 32,
            read_declaration: TRUE as u32,
        }
    }
}

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

/// WebServices.h's WS_XML_READER_PROPERTY: which setting, a pointer to
/// its value and the value's size. Properties are fixed when the reader
/// is created -- WebServices.h has no WsSetReaderProperty -- so this is
/// the only shape that carries them.
#[repr(C)]
struct WsXmlReaderProperty {
    id: u32,
    value: *const c_void,
    value_size: u32,
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
    /// What the reader was created with, which WsSetInput does not
    /// change (WebServices.h has no WsSetReaderProperty, so these are
    /// fixed for the reader's life).
    settings: Settings,
    /// The character set the input was decoded as, which
    /// WS_XML_READER_PROPERTY_CHARSET reports.
    char_set: u32,
    /// The document as text. The input's bytes are the caller's, so the
    /// reader decodes them into its own copy when the input is set
    /// rather than reading from memory it does not own.
    text: String,
    scan: Scan,
    /// The node the reader is on. None before the first read and
    /// while a read is between nodes; a read that consumes one reads
    /// ahead to the next, as Windows' reader does.
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
                    // A start element deeper than the reader allows:
                    // the read that reaches its tag is refused, which
                    // is where Windows refuses it (the row and column
                    // in its error are the tag's, not the one the call
                    // started from).
                    if self.scan.depth() as u32 > self.settings.max_depth {
                        self.faulted = true;
                        let max = self.settings.max_depth;
                        fail(
                            error,
                            function,
                            &alloc::format!(
                                "The maximum allowed depth of {max} for the document was exceeded."
                            ),
                        );
                        return WS_E_QUOTA_EXCEEDED;
                    }
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

    /// A read met something the document cannot mean: described in the
    /// error object, and the reader is faulted, so every later call
    /// answers WS_E_OBJECT_FAULTED. Windows faults its reader on every
    /// read of a value that the document does not hold -- a number that
    /// is not one, a number too big for its type, an element where a
    /// value was expected -- not only on text that is not well-formed.
    fn malformed(&mut self, error: *mut c_void, function: &[u8], text: &str) {
        self.faulted = true;
        fail(error, function, text);
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
    let mut settings = Settings::default();
    if property_count != 0 {
        // SAFETY: the caller's array of `property_count` properties,
        // which it says is this long and which is not null (above).
        let properties = unsafe {
            core::slice::from_raw_parts(
                properties.cast::<WsXmlReaderProperty>(),
                property_count as usize,
            )
        };
        let mut seen = 0u32;
        for p in properties {
            // An id that is not a property, one that is the reader's to
            // report and not the caller's to set, a value of the wrong
            // size, no value at all, or the same property twice: each
            // is E_INVALIDARG and no reader, as Windows answers. The id
            // is recorded either way, so a refusal gala gets wrong is
            // in the ledger rather than only in the caller's HRESULT.
            let Some(it) = property(p.id) else {
                record(b"WsCreateReader", b"an unknown reader property", OTHER, u64::from(p.id));
                return E_INVALIDARG;
            };
            if !it.settable || p.value_size != it.size || p.value.is_null() {
                record(b"WsCreateReader", b"a reader property refused", OTHER, u64::from(p.id));
                return E_INVALIDARG;
            }
            // Every id `property` answers for is under 32, so one bit
            // each says which have been seen.
            if seen & (1 << p.id) != 0 {
                record(b"WsCreateReader", b"a reader property twice", OTHER, u64::from(p.id));
                return E_INVALIDARG;
            }
            seen |= 1 << p.id;
            // SAFETY: the four bytes the property's size says are
            // there, read unaligned because nothing promises the
            // alignment of a value inside the caller's own structure.
            let value = unsafe { p.value.cast::<u32>().read_unaligned() };
            match p.id {
                WS_XML_READER_PROPERTY_MAX_DEPTH => {
                    settings.max_depth = value;
                    // gala's XML reader has a limit of its own, which a
                    // deeper one does not lift.
                    if value as usize > gala_xml::MAX_DEPTH {
                        record(
                            b"WsCreateReader",
                            b"a depth over the scanner's own",
                            OTHER,
                            u64::from(value),
                        );
                    }
                }
                WS_XML_READER_PROPERTY_ALLOW_FRAGMENT => settings.allow_fragment = value != 0,
                WS_XML_READER_PROPERTY_MAX_ATTRIBUTES => {
                    settings.max_attributes = value;
                    record(b"WsCreateReader", b"an attribute limit", OTHER, u64::from(value));
                }
                WS_XML_READER_PROPERTY_MAX_NAMESPACES => {
                    settings.max_namespaces = value;
                    record(b"WsCreateReader", b"a namespace limit", OTHER, u64::from(value));
                }
                WS_XML_READER_PROPERTY_READ_DECLARATION => {
                    settings.read_declaration = value;
                    record(b"WsCreateReader", b"the declaration setting", OTHER, u64::from(value));
                }
                // The MIME and stream settings: accepted, as Windows
                // accepts them, and named in the ledger because nothing
                // honours them.
                _ => record(b"WsCreateReader", b"a stream property", OTHER, u64::from(p.id)),
            }
        }
    }
    let object = new_object(
        READER,
        Reader {
            settings,
            char_set: WS_CHARSET_UTF8,
            text: String::new(),
            scan: Scan::new(),
            node: None,
            ready: false,
            faulted: false,
        },
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
    reader.scan =
        if reader.settings.allow_fragment { Scan::new().allow_fragment() } else { Scan::new() };
    reader.char_set = match char_set {
        WS_CHARSET_AUTO => WS_CHARSET_UTF8,
        other => other,
    };
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

/// WsGetReaderProperty: one setting of the reader, or where it has
/// reached. The buffer must be exactly the property's own size.
///
/// The order of the refusals is Windows' own, measured: a faulted
/// reader answers WS_E_OBJECT_FAULTED and one with no input
/// WS_E_INVALID_OPERATION whatever is asked of them; then an id that is
/// no property a reader reports is E_INVALIDARG; and only then the
/// size, a wrong one being E_INVALIDARG that leaves the buffer alone
/// and faults the reader.
#[unsafe(no_mangle)]
pub extern "system" fn WsGetReaderProperty(
    reader: *mut c_void,
    id: u32,
    buffer: *mut c_void,
    buffer_size: u32,
    error: *mut c_void,
) -> HResult {
    if buffer.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsGetReaderProperty") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsGetReaderProperty");
    if hr != S_OK {
        return hr;
    }
    let Some(it) = property(id).filter(|it| it.readable) else {
        record(b"WsGetReaderProperty", b"reader property", OTHER, u64::from(id));
        fail(error, b"WsGetReaderProperty", "The property ID was not recognized or supported.");
        return E_INVALIDARG;
    };
    if buffer_size != it.size {
        reader.faulted = true;
        fail(error, b"WsGetReaderProperty", "The buffer specified for the property is invalid.");
        return E_INVALIDARG;
    }

    // The two wide properties are where the reader has got to: the line
    // and how far into it, both counted from 0, which is what Windows
    // reports (its error strings count them from 1).
    if id == WS_XML_READER_PROPERTY_ROW || id == WS_XML_READER_PROPERTY_COLUMN {
        let (line, column) = reader.scan.position();
        let value = if id == WS_XML_READER_PROPERTY_ROW {
            u64::from(line.saturating_sub(1))
        } else {
            column as u64
        };
        // SAFETY: the caller's buffer, of the eight bytes it says.
        unsafe { buffer.cast::<u64>().write_unaligned(value) };
        return S_OK;
    }

    let value = match id {
        WS_XML_READER_PROPERTY_MAX_DEPTH => reader.settings.max_depth,
        WS_XML_READER_PROPERTY_ALLOW_FRAGMENT => u32::from(reader.settings.allow_fragment),
        WS_XML_READER_PROPERTY_MAX_ATTRIBUTES => reader.settings.max_attributes,
        WS_XML_READER_PROPERTY_MAX_NAMESPACES => reader.settings.max_namespaces,
        WS_XML_READER_PROPERTY_READ_DECLARATION => reader.settings.read_declaration,
        WS_XML_READER_PROPERTY_CHARSET => reader.char_set,
        // WS_XML_READER_PROPERTY_IN_ATTRIBUTE: gala's reader has no
        // attribute cursor, so it is never in one.
        _ => FALSE as u32,
    };
    // SAFETY: the caller's buffer, of the four bytes it says.
    unsafe { buffer.cast::<u32>().write_unaligned(value) };
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

/// WsReadEndElement: moves past the end tag the reader is on, out of
/// the element it was in, and on to the node after it -- where a
/// document that has more than one root element fails, since what
/// follows the first root's end tag is content after the root element.
#[unsafe(no_mangle)]
pub extern "system" fn WsReadEndElement(reader: *mut c_void, error: *mut c_void) -> HResult {
    // SAFETY: a handle this DLL gave out.
    let Some(reader) = (unsafe { body::<Reader>(reader, READER, b"WsReadEndElement") }) else {
        return E_INVALIDARG;
    };
    let hr = reader.usable(error, b"WsReadEndElement");
    if hr != S_OK {
        return hr;
    }
    let hr = reader.settle(error, b"WsReadEndElement");
    if hr != S_OK {
        return hr;
    }
    if !matches!(reader.node, Some(Node::End)) {
        fail(error, b"WsReadEndElement", "The reader is not on an end element.");
        return WS_E_INVALID_FORMAT;
    }
    reader.node = None;
    reader.pull(error, b"WsReadEndElement")
}

/// WsReadType: the element the reader is on, read whole as a value of
/// `ws_type` (WS_ELEMENT_TYPE_MAPPING: its start tag, its text and its
/// end tag), leaving the reader after its end tag.
///
/// WS_ELEMENT_CONTENT_TYPE_MAPPING reads from inside an element
/// instead (the caller has read its start tag) and leaves the reader on
/// the end tag, for the caller's own WsReadEndElement -- which is how
/// setup.exe reads the configuration service's answer. It refuses
/// nothing on where the reader stands: it takes the text it finds
/// there, which is none at all when the reader is on a start tag, and
/// leaves the reader where it was.
///
/// A read that the document does not hold a value for faults the
/// reader, as Windows' does.
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
    let content = match type_mapping {
        WS_ELEMENT_TYPE_MAPPING => false,
        WS_ELEMENT_CONTENT_TYPE_MAPPING => true,
        _ => {
            record(b"WsReadType", b"type mapping", NOT_IMPLEMENTED, u64::from(type_mapping));
            return WS_E_NOT_SUPPORTED;
        }
    };
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
    let mut text = String::new();
    if content {
        // The text from where the reader stands up to the first node
        // that is not text: the element's end tag, or a child
        // element's start tag, which no string or number is made of.
        // Neither is consumed, so the caller's own WsReadEndElement
        // still has the end tag to read -- and an element that holds
        // elements holds the empty string, which is what Windows reads
        // there and the reason this mapping refuses nothing on where
        // the reader stands.
        while let Some(Node::Text(run)) = &reader.node {
            text.push_str(run);
            reader.node = None;
            let hr = reader.pull(error, b"WsReadType");
            if hr != S_OK {
                return hr;
            }
        }
    } else {
        if !matches!(reader.node, Some(Node::Start { .. })) {
            reader.malformed(error, b"WsReadType", "A start element was expected.");
            return WS_E_INVALID_FORMAT;
        }
        // Past the start tag, then the text to the end tag. A child
        // element is not part of a string or a number, so an element
        // that has one does not hold a value of these types.
        loop {
            reader.node = None;
            let hr = reader.pull(error, b"WsReadType");
            if hr != S_OK {
                return hr;
            }
            match &reader.node {
                Some(Node::Text(run)) => text.push_str(run),
                Some(Node::End) => break,
                _ => {
                    reader.malformed(
                        error,
                        b"WsReadType",
                        "The element does not hold a value of this type.",
                    );
                    return WS_E_INVALID_FORMAT;
                }
            }
        }
        // Past the element's end tag and on to the node after it, as
        // Windows' reader leaves it (ROW and COLUMN stand there). The
        // content mapping leaves the reader on the end tag instead,
        // for the caller's WsReadEndElement.
        reader.node = None;
        let hr = reader.pull(error, b"WsReadType");
        if hr != S_OK {
            return hr;
        }
    }

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
                reader.malformed(error, b"WsReadType", "The element does not hold a number.");
                return WS_E_INVALID_FORMAT;
            }
            let Ok(number) = digits.parse::<u32>() else {
                reader.malformed(error, b"WsReadType", "The number does not fit in 32 bits.");
                return WS_E_NUMERIC_OVERFLOW;
            };
            // SAFETY: the caller's buffer, of the four bytes it said.
            unsafe { value.cast::<u32>().write_unaligned(number) };
            S_OK
        }
    }
}
