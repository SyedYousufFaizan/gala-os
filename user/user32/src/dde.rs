//! The DDE packing helpers (PackDDElParam, UnpackDDElParam,
//! ReuseDDElParam, FreeDDElParam).
//!
//! Four DDE messages carry two values in one lParam (WM_DDE_ACK,
//! WM_DDE_ADVISE, WM_DDE_DATA, WM_DDE_POKE): the sender packs them into
//! an object, and the receiver unpacks it and frees it (or reuses it for
//! its answer). gala keeps those objects in a table of its own, the
//! lParam being an entry's handle. WM_DDE_EXECUTE carries one handle,
//! which is the lParam itself (the high half of what was packed); the
//! remaining messages carry the two halves of the lParam, and nothing is
//! allocated for them. An lParam that is no object of gala's is read as
//! those halves, so a parameter a program packed itself unpacks too.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const WM_DDE_ADVISE: u32 = 0x03e2;
const WM_DDE_ACK: u32 = 0x03e4;
const WM_DDE_DATA: u32 = 0x03e5;
const WM_DDE_POKE: u32 = 0x03e7;
const WM_DDE_EXECUTE: u32 = 0x03e8;

/// The objects a process can have packed at one time. DDE conversations
/// pack one parameter per message and free it when it is received.
const MAX: usize = 256;
/// Where the handles start, and how far apart they are.
const BASE: usize = 0x000d_0000;
const STEP: usize = 0x10;

#[derive(Clone, Copy)]
struct Entry {
    used: bool,
    low: usize,
    high: usize,
}

struct Table {
    lock: AtomicBool,
    entries: UnsafeCell<[Entry; MAX]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    entries: UnsafeCell::new([Entry { used: false, low: 0, high: 0 }; MAX]),
};

fn with<R>(f: impl FnOnce(&mut [Entry; MAX]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe { f(&mut *TABLE.entries.get()) };
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// Whether `msg` carries a packed object.
fn packs(msg: u32) -> bool {
    matches!(msg, WM_DDE_ACK | WM_DDE_ADVISE | WM_DDE_DATA | WM_DDE_POKE)
}

/// The table entry a handle names, if it is one of gala's.
fn slot(lparam: isize) -> Option<usize> {
    let d = (lparam as usize).checked_sub(BASE)?;
    (d % STEP == 0 && d / STEP < MAX).then_some(d / STEP)
}

/// The halves of an lParam that carries no object.
fn halves(lparam: isize) -> (usize, usize) {
    let v = lparam as usize;
    (v & 0xffff, (v >> 16) & 0xffff)
}

/// PackDDElParam: an lParam for `msg` carrying both values. The messages
/// that have an object get one (0 when 256 are already packed);
/// WM_DDE_EXECUTE's lParam is `high` alone (the data handle), and the
/// rest take the two halves.
#[unsafe(no_mangle)]
pub extern "system" fn PackDDElParam(msg: u32, low: usize, high: usize) -> isize {
    if !packs(msg) {
        return if msg == WM_DDE_EXECUTE {
            high as isize
        } else {
            (((high & 0xffff) << 16) | (low & 0xffff)) as isize
        };
    }
    with(|e| match e.iter().position(|e| !e.used) {
        Some(i) => {
            e[i] = Entry { used: true, low, high };
            (BASE + i * STEP) as isize
        }
        None => {
            crate::record(b"user32.dll!PackDDElParam(table full)", MAX as u64);
            0
        }
    })
}

/// UnpackDDElParam: the two values an lParam carries. An lParam that is
/// no object of gala's gives its own halves (WM_DDE_EXECUTE: the whole
/// lParam as the high value, as PackDDElParam left it).
#[unsafe(no_mangle)]
pub extern "system" fn UnpackDDElParam(
    msg: u32,
    lparam: isize,
    low: *mut usize,
    high: *mut usize,
) -> i32 {
    let (lo, hi) = match slot(lparam).filter(|_| packs(msg)) {
        Some(i) => match with(|e| e[i].used.then_some((e[i].low, e[i].high))) {
            Some(v) => v,
            None => return 0,
        },
        None if msg == WM_DDE_EXECUTE => (0, lparam as usize),
        None => halves(lparam),
    };
    if !low.is_null() {
        // SAFETY: the caller's output.
        unsafe { *low = lo };
    }
    if !high.is_null() {
        // SAFETY: the caller's output.
        unsafe { *high = hi };
    }
    1
}

/// FreeDDElParam: frees the object an lParam names. A message that
/// carries none has nothing to free and succeeds, as does an lParam that
/// is no object of gala's.
#[unsafe(no_mangle)]
pub extern "system" fn FreeDDElParam(msg: u32, lparam: isize) -> i32 {
    if let Some(i) = slot(lparam).filter(|_| packs(msg)) {
        with(|e| e[i] = Entry { used: false, low: 0, high: 0 });
    }
    1
}

/// ReuseDDElParam: the parameter of `msg_in` made into one for
/// `msg_out`. When both messages carry an object, the one that is there
/// takes the new values and is returned; otherwise it is freed and the
/// answer packed afresh.
#[unsafe(no_mangle)]
pub extern "system" fn ReuseDDElParam(
    lparam: isize,
    msg_in: u32,
    msg_out: u32,
    low: usize,
    high: usize,
) -> isize {
    if packs(msg_out)
        && let Some(i) = slot(lparam).filter(|_| packs(msg_in))
        && with(|e| {
            let reuse = e[i].used;
            if reuse {
                e[i] = Entry { used: true, low, high };
            }
            reuse
        })
    {
        return lparam;
    }
    FreeDDElParam(msg_in, lparam);
    PackDDElParam(msg_out, low, high)
}
