//! The ARM64 parts of kernel32: a stub that compiles (ROADMAP PT; the
//! port is item AR), so code outside the per-architecture modules that
//! assumes x86 fails the ARM64 check (CI's Clippy step,
//! `--target aarch64-pc-windows-msvc`). The names are `x64.rs`'s.
//!
//! What the public documents say plainly is filled in: the TEB through
//! x18 (Microsoft's "Overview of ARM64 ABI conventions": in user mode x18
//! points to the TEB), the 64-bit layouts (`win64.rs`), ARM64's CONTEXT
//! (winnt.h), PROCESSOR_ARCHITECTURE_ARM64, and what is the processor's
//! own (the virtual counter, an exclusive pair for SLIST_HEADER). What
//! the kernel has not defined for ARM64 yet records the gap in the ledger
//! and ends the process (`unported`).

pub use super::win64::*;

/// PROCESSOR_ARCHITECTURE_ARM64: what GetSystemInfo reports. The
/// processor type GetSystemInfo gives on ARM64 is to be recorded on ARM64
/// Windows with the port; 0 until then.
pub const PROCESSOR_ARCHITECTURE: u16 = 12;
pub const PROCESSOR_TYPE: u32 = 0;
/// What GetNativeSystemInfo reports: the same.
pub const NATIVE_PROCESSOR_ARCHITECTURE: u16 = PROCESSOR_ARCHITECTURE;
pub const NATIVE_PROCESSOR_TYPE: u32 = PROCESSOR_TYPE;

/// The native system directory: System32.
pub const NATIVE_SYSTEM_DIRECTORY: &[u8] = b"\\system32";

/// Where the port has not been written yet: recorded, then the process
/// ends (the panic handler's ExitProcess).
#[track_caller]
fn unported(what: &[u8]) -> ! {
    super::unsupported(what, 0);
    panic!("kernel32 on ARM64: not ported yet")
}

/// Reads the pointer-sized field of the current TEB at `offset`.
pub fn teb_read(offset: usize) -> usize {
    let v: usize;
    // SAFETY: x18 holds the TEB; [x18 + offset] is a field of it.
    unsafe {
        core::arch::asm!("ldr {}, [x18, {}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

/// Writes the pointer-sized field of the current TEB at `offset`.
pub fn teb_write(offset: usize, value: usize) {
    // SAFETY: as teb_read, a field the caller names.
    unsafe {
        core::arch::asm!("str {}, [x18, {}]", in(reg) value, in(reg) offset, options(nostack, preserves_flags))
    };
}

/// Writes the 32-bit field of the current TEB at `offset`.
pub fn teb_write_u32(offset: usize, value: u32) {
    // SAFETY: as teb_write.
    unsafe {
        core::arch::asm!("str {:w}, [x18, {}]", in(reg) value, in(reg) offset, options(nostack, preserves_flags))
    };
}

/// A free-running counter: the virtual counter (CNTVCT_EL0).
pub fn cycles() -> u64 {
    let v: u64;
    // SAFETY: reads the virtual counter; no memory, no flags.
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

/// The processor running this thread: where the kernel tells it on ARM64
/// is the port's.
pub fn current_processor() -> u32 {
    unported(b"kernel32.dll!GetCurrentProcessorNumber(ARM64)")
}

/// CONTEXT (ARM64 layout, winnt.h's ARM64_NT_CONTEXT: 0x390 bytes,
/// 16-byte aligned).
#[repr(C, align(16))]
pub struct Context {
    context_flags: u32,
    cpsr: u32,
    /// X0..X28, Fp (x29), Lr (x30).
    x: [u64; 31],
    sp: u64,
    pc: u64,
    v: [u128; 32],
    fpcr: u32,
    fpsr: u32,
    bcr: [u32; 8],
    bvr: [u64; 8],
    wcr: [u32; 2],
    wvr: [u64; 2],
}

const _: () = assert!(size_of::<Context>() == 0x390);

/// CONTEXT_ARM64 | CONTEXT_CONTROL | CONTEXT_INTEGER |
/// CONTEXT_FLOATING_POINT.
const CONTEXT_FULL: u32 = 0x40_0007;

/// The CONTEXT a new thread starts with: at `entry(args[0], args[1])`
/// (x0, x1) with its stack ending at `top` (16-byte aligned), no return
/// address (lr 0). Where NtCreateThread takes the Win32 start address on
/// ARM64 (x64's rax) is the kernel's port.
pub fn thread_context(entry: usize, args: [usize; 2], top: usize) -> Context {
    // SAFETY: an all-zero CONTEXT is a valid bit pattern.
    let mut context: Context = unsafe { core::mem::zeroed() };
    context.context_flags = CONTEXT_FULL;
    context.pc = entry as u64;
    context.x[0] = args[0] as u64;
    context.x[1] = args[1] as u64;
    context.sp = (top & !0xf) as u64;
    context
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER (16 bytes): x64's encoding (depth and sequence in the low
// quadword, the first entry and HeaderType 1 in the high one), changed
// together by an exclusive pair (ldaxp/stlxp). Whether ARM64 Windows
// encodes its header bits the same is to be checked with the port.

#[repr(C, align(16))]
pub struct SListHeader {
    low: u64,
    high: u64,
}

#[repr(C)]
pub struct SListEntry {
    pub next: *mut SListEntry,
}

/// A header's value: (depth and sequence, first entry and type).
pub type SListValue = (u64, u64);

/// An empty header's value.
pub const SLIST_EMPTY: SListValue = (0, 1);

/// Compares the header with `old` and, if equal, stores `new`; returns
/// the value seen.
pub fn slist_cas(head: *mut SListHeader, old: SListValue, new: SListValue) -> SListValue {
    let (low, high): (u64, u64);
    // SAFETY: a 16-byte aligned SLIST_HEADER. The pair is loaded
    // exclusively; if it equals `old`, `new` is stored, else the value
    // seen is stored back (so the read is single-copy atomic as well);
    // either store failing (another writer) retries.
    unsafe {
        core::arch::asm!(
            "2:",
            "ldaxp {low}, {high}, [{head}]",
            "cmp {low}, {old_low}",
            "ccmp {high}, {old_high}, #0, eq",
            "b.ne 3f",
            "stlxp {status:w}, {new_low}, {new_high}, [{head}]",
            "cbnz {status:w}, 2b",
            "b 4f",
            "3:",
            "stlxp {status:w}, {low}, {high}, [{head}]",
            "cbnz {status:w}, 2b",
            "4:",
            head = in(reg) head,
            old_low = in(reg) old.0,
            old_high = in(reg) old.1,
            new_low = in(reg) new.0,
            new_high = in(reg) new.1,
            low = out(reg) low,
            high = out(reg) high,
            status = out(reg) _,
            options(nostack),
        );
    }
    (low, high)
}

/// The header's value, read atomically (a compare-exchange that fails,
/// or rewrites the same value).
pub fn slist_read(head: *mut SListHeader) -> SListValue {
    slist_cas(head, (0, 0), (0, 0))
}

pub fn slist_first(v: SListValue) -> *mut SListEntry {
    (v.1 & !0xf) as *mut SListEntry
}

pub fn slist_depth(v: SListValue) -> u16 {
    v.0 as u16
}

/// The value with `first` on top, `depth` entries and the next sequence
/// number.
pub fn slist_value(old: SListValue, first: *mut SListEntry, depth: u16) -> SListValue {
    let sequence = (old.0 >> 16).wrapping_add(1) & 0xffff_ffff_ffff;
    (u64::from(depth) | sequence << 16, first as u64 | 1)
}

pub fn slist_init(head: *mut SListHeader) {
    // SAFETY: the caller's aligned header.
    unsafe { head.write(SListHeader { low: SLIST_EMPTY.0, high: SLIST_EMPTY.1 }) };
}
