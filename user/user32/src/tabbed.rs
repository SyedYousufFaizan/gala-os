//! Tabbed text (GetTabbedTextExtentW, TabbedTextOutW) and GrayStringW,
//! over gdi32's text functions.
//!
//! Tab stops are measured from the tab origin: with none, tabs go to
//! every eighth average character width; with one stop, to its multiples;
//! with several, to the stops given (and, past the last of them, to the
//! default width again). Negative stops ask for right-aligned text,
//! which gala does not draw: it takes their distance. The text is drawn
//! from its left edge, whatever SetTextAlign asked for.
//!
//! GrayStringW draws the text in the grayed text colour (COLOR_GRAYTEXT)
//! instead of Windows' half-tone pattern over a bitmap of it, and
//! ignores the brush. An output function of the caller's is given the
//! device context itself, not a bitmap of its own, and draws where it
//! wants to; `x` and `y` are then left to it.

use crate::window::Handle;
use core::sync::atomic::AtomicUsize;

/// The characters of a string gala lays out at most.
const MAX: usize = 1024;
const TAB: u16 = 9;
/// COLOR_GRAYTEXT.
const GRAY_TEXT: i32 = 17;

type Extent = extern "system" fn(Handle, *const u16, i32, *mut [i32; 2]) -> i32;
type Metrics = extern "system" fn(Handle, *mut [i32; 15]) -> i32;
type TextOut = extern "system" fn(Handle, i32, i32, *const u16, i32) -> i32;
type SetColor = extern "system" fn(Handle, u32) -> u32;

/// What GrayStringW's caller draws with: the device context, the data it
/// was given and its length.
type Output = extern "system" fn(Handle, isize, i32) -> i32;

struct Gdi {
    extent: Extent,
    metrics: Metrics,
    text_out: TextOut,
    set_color: SetColor,
}

fn gdi() -> Option<Gdi> {
    static E: AtomicUsize = AtomicUsize::new(0);
    static M: AtomicUsize = AtomicUsize::new(0);
    static T: AtomicUsize = AtomicUsize::new(0);
    static C: AtomicUsize = AtomicUsize::new(0);
    let f = |c: &AtomicUsize, n: &[u8]| crate::icon::gdi_proc(c, n);
    let (e, m, t, c) = (
        f(&E, b"GetTextExtentPoint32W\0"),
        f(&M, b"GetTextMetricsW\0"),
        f(&T, b"TextOutW\0"),
        f(&C, b"SetTextColor\0"),
    );
    if e == 0 || m == 0 || t == 0 || c == 0 {
        return None;
    }
    // SAFETY: gdi32's functions of those names have these signatures.
    unsafe {
        Some(Gdi {
            extent: core::mem::transmute::<usize, Extent>(e),
            metrics: core::mem::transmute::<usize, Metrics>(m),
            text_out: core::mem::transmute::<usize, TextOut>(t),
            set_color: core::mem::transmute::<usize, SetColor>(c),
        })
    }
}

/// The string as a slice: `len` characters, or up to the NUL when `len`
/// is negative, and at most MAX.
///
/// # Safety
/// `text` points at that many characters (or at a NUL-terminated string).
unsafe fn units<'a>(text: *const u16, len: i32) -> &'a [u16] {
    if text.is_null() {
        return &[];
    }
    let n = if len >= 0 {
        (len as usize).min(MAX)
    } else {
        let mut k = 0;
        // SAFETY: the caller's NUL-terminated string.
        while k < MAX && unsafe { *text.add(k) } != 0 {
            k += 1;
        }
        k
    };
    // SAFETY: `n` characters of the caller's string.
    unsafe { core::slice::from_raw_parts(text, n) }
}

/// The tab stops as a slice.
///
/// # Safety
/// `stops` points at `tabs` positions.
unsafe fn positions<'a>(tabs: i32, stops: *const i32) -> &'a [i32] {
    if stops.is_null() || tabs <= 0 {
        return &[];
    }
    // SAFETY: the caller's `tabs` positions.
    unsafe { core::slice::from_raw_parts(stops, tabs as usize) }
}

/// The first tab stop after `at`: one of the stops given, else the next
/// multiple of `default` from `origin`.
fn next_stop(at: i32, origin: i32, stops: &[i32], default: i32) -> i32 {
    for &s in stops {
        let stop = origin + s.abs();
        if stop > at {
            return stop;
        }
    }
    origin + ((at - origin).div_euclid(default) + 1) * default
}

/// Lays `text` out from `x`, its tabs going to the stops measured from
/// `origin`, calling `run(x, piece)` for each piece between the tabs.
/// Answers the width of the whole and the height of its tallest piece.
fn layout(
    g: &Gdi,
    dc: Handle,
    text: &[u16],
    stops: &[i32],
    origin: i32,
    x: i32,
    mut run: impl FnMut(i32, &[u16]),
) -> (i32, i32) {
    let mut tm = [0i32; 15];
    (g.metrics)(dc, &mut tm);
    // TEXTMETRICW: tmHeight, tmAscent, tmDescent, tmInternalLeading,
    // tmExternalLeading, tmAveCharWidth.
    let (stops, default) = match *stops {
        [] => (&[][..], 8 * tm[5].max(1)),
        [one] => (&[][..], one.abs().max(1)),
        _ => (stops, 8 * tm[5].max(1)),
    };
    // An empty string measures the font's height.
    let probe = [0u16; 1];
    let mut size = [0i32; 2];
    (g.extent)(dc, probe.as_ptr(), 0, &mut size);
    let mut height = size[1];
    let mut at = x;
    let mut piece = 0;
    for k in 0..=text.len() {
        if k < text.len() && text[k] != TAB {
            continue;
        }
        let part = &text[piece..k];
        if !part.is_empty() {
            (g.extent)(dc, part.as_ptr(), part.len() as i32, &mut size);
            run(at, part);
            at += size[0];
            height = height.max(size[1]);
        }
        if k < text.len() {
            at = next_stop(at, origin, stops, default);
        }
        piece = k + 1;
    }
    (at - x, height)
}

/// A width and a height in one DWORD, as the tabbed text functions
/// answer them.
fn both(width: i32, height: i32) -> u32 {
    ((height as u32) << 16) | (width as u32 & 0xffff)
}

/// GetTabbedTextExtentW: how wide and high the text would be with its
/// tabs expanded (the width in the low word, the height in the high one).
#[unsafe(no_mangle)]
pub extern "system" fn GetTabbedTextExtentW(
    dc: Handle,
    text: *const u16,
    len: i32,
    tabs: i32,
    stops: *const i32,
) -> u32 {
    let Some(g) = gdi() else { return 0 };
    // SAFETY: the caller's string of `len` characters.
    let text = unsafe { units(text, len) };
    // SAFETY: the caller's `tabs` stops.
    let stops = unsafe { positions(tabs, stops) };
    let (w, h) = layout(&g, dc, text, stops, 0, 0, |_, _| {});
    both(w, h)
}

/// TabbedTextOutW: the text drawn from (x, y) with its tabs expanded;
/// answers its width and height as GetTabbedTextExtentW does.
#[unsafe(no_mangle)]
pub extern "system" fn TabbedTextOutW(
    dc: Handle,
    x: i32,
    y: i32,
    text: *const u16,
    len: i32,
    tabs: i32,
    stops: *const i32,
    origin: i32,
) -> i32 {
    let Some(g) = gdi() else { return 0 };
    // SAFETY: the caller's string of `len` characters.
    let text = unsafe { units(text, len) };
    // SAFETY: the caller's `tabs` stops.
    let stops = unsafe { positions(tabs, stops) };
    let (w, h) = layout(&g, dc, text, stops, origin, x, |at, part| {
        (g.text_out)(dc, at, y, part.as_ptr(), part.len() as i32);
    });
    both(w, h) as i32
}

/// GrayStringW: the text drawn grayed. Without an output function of the
/// caller's, TextOutW draws it at (x, y) in the grayed text colour; with
/// one, that function draws and gala only sets the colour. The brush, the
/// width and the height are not used: nothing is drawn into a bitmap of
/// its own (the module's note).
#[unsafe(no_mangle)]
pub extern "system" fn GrayStringW(
    dc: Handle,
    _brush: Handle,
    output: Option<Output>,
    data: isize,
    len: i32,
    x: i32,
    y: i32,
    _width: i32,
    _height: i32,
) -> i32 {
    let Some(g) = gdi() else { return 0 };
    if data == 0 {
        crate::set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    // SAFETY: with no length, the data is a NUL-terminated string.
    let count = if len > 0 { len } else { unsafe { units(data as *const u16, -1) }.len() as i32 };
    let old = (g.set_color)(dc, crate::window::GetSysColor(GRAY_TEXT));
    let drawn = match output {
        Some(f) => f(dc, data, count),
        None => {
            // SAFETY: the caller's string of `count` characters.
            let text = unsafe { units(data as *const u16, count) };
            (g.text_out)(dc, x, y, text.as_ptr(), text.len() as i32)
        }
    };
    (g.set_color)(dc, old);
    i32::from(drawn != 0)
}
