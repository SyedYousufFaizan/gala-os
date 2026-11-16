//! ARM64's part of exception dispatch (the shared part is `dispatch.rs`),
//! written from the ARM64 CONTEXT's public layout (winnt.h) and the Rtl*
//! functions' documented contracts:
//!
//! - `KiUserExceptionDispatcher`: the kernel resumes a thread here after
//!   a user-mode exception with sp at the CONTEXT of the fault and the
//!   EXCEPTION_RECORD right above it (`kernel/src/nt/abi/arm64`'s
//!   frame), and it hands both to `dispatch_exception`.
//! - `RtlCaptureContext(ContextRecord)`: the caller's registers
//!   (CONTEXT_FULL), Pc = its return address, Sp = its sp.
//! - `RtlRaiseException(ExceptionRecord)`: captures its caller's context,
//!   sets ExceptionAddress to the return address, dispatches (`raise`).
//!
//! Not yet (ROADMAP AR): frame-based handlers. ARM64's unwind data
//! (.pdata and .xdata, "ARM64 exception handling") is not read, so
//! `dispatch_frames` searches no frame and says so in the ledger; with it
//! come RtlLookupFunctionEntry, RtlVirtualUnwind, RtlUnwindEx and the
//! dispatcher's own unwind data.

use core::arch::global_asm;

/// ARM64 CONTEXT: ContextFlags.
pub(crate) const CTX_FLAGS: usize = 0;
/// RECORD's place in the kernel's dispatcher frame: right above the
/// CONTEXT (sizeof(ARM64_NT_CONTEXT)).
const RECORD_OFFSET: usize = 0x390;

/// The frames' handlers: none searched yet (ARM64's unwind data is not
/// read). Reported, so an exception that ends a process for want of them
/// says why in the ledger.
pub(crate) fn dispatch_frames(_record: *mut u8, _context: *mut u8) -> bool {
    let what = b"ntdll.dll!frame-based exception handlers (ARM64 unwinding)";
    crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, 0);
    false
}

// RtlCaptureContext(x0 = ContextRecord): X0-X28, Fp, Lr, Sp (the
// caller's), Pc = Lr (the return address), Cpsr's condition flags, V0-V31,
// Fpcr and Fpsr; ContextFlags CONTEXT_FULL. Only x16 and x17 (the
// intra-procedure scratch registers) are used, after being stored.
//
// RtlRaiseException(x0 = ExceptionRecord): a frame record and room for a
// CONTEXT; the captured context is then made the caller's (X0 the record,
// Fp and Lr the caller's, Sp above this frame, Pc the return address),
// ExceptionAddress set to the return address, and `raise` dispatches.
//
// KiUserExceptionDispatcher: sp is at the CONTEXT, the record above it.
global_asm!(
    ".text",
    ".globl RtlCaptureContext",
    ".p2align 2",
    "RtlCaptureContext:",
    "stp x0, x1, [x0, #0x08]",
    "stp x2, x3, [x0, #0x18]",
    "stp x4, x5, [x0, #0x28]",
    "stp x6, x7, [x0, #0x38]",
    "stp x8, x9, [x0, #0x48]",
    "stp x10, x11, [x0, #0x58]",
    "stp x12, x13, [x0, #0x68]",
    "stp x14, x15, [x0, #0x78]",
    "stp x16, x17, [x0, #0x88]",
    "stp x18, x19, [x0, #0x98]",
    "stp x20, x21, [x0, #0xa8]",
    "stp x22, x23, [x0, #0xb8]",
    "stp x24, x25, [x0, #0xc8]",
    "stp x26, x27, [x0, #0xd8]",
    "str x28, [x0, #0xe8]",
    "stp x29, x30, [x0, #0xf0]",
    "mov x16, sp",
    "stp x16, x30, [x0, #0x100]",
    "mrs x16, nzcv",
    "str w16, [x0, #0x4]",
    "add x16, x0, #0x110",
    "stp q0, q1, [x16, #0x00]",
    "stp q2, q3, [x16, #0x20]",
    "stp q4, q5, [x16, #0x40]",
    "stp q6, q7, [x16, #0x60]",
    "stp q8, q9, [x16, #0x80]",
    "stp q10, q11, [x16, #0xa0]",
    "stp q12, q13, [x16, #0xc0]",
    "stp q14, q15, [x16, #0xe0]",
    "stp q16, q17, [x16, #0x100]",
    "stp q18, q19, [x16, #0x120]",
    "stp q20, q21, [x16, #0x140]",
    "stp q22, q23, [x16, #0x160]",
    "stp q24, q25, [x16, #0x180]",
    "stp q26, q27, [x16, #0x1a0]",
    "stp q28, q29, [x16, #0x1c0]",
    "stp q30, q31, [x16, #0x1e0]",
    "mrs x17, fpcr",
    "str w17, [x0, #0x310]",
    "mrs x17, fpsr",
    "str w17, [x0, #0x314]",
    // CONTEXT_ARM64 | CONTROL | INTEGER | FLOATING_POINT
    "mov w16, #0x7",
    "movk w16, #0x40, lsl #16",
    "str w16, [x0]",
    "ret",
    "",
    ".globl RtlRaiseException",
    ".p2align 2",
    "RtlRaiseException:",
    "stp x29, x30, [sp, #-16]!",
    "mov x29, sp",
    "sub sp, sp, #0x3a0",
    "str x0, [sp, #0x390]",
    "mov x0, sp",
    "bl RtlCaptureContext",
    "ldr x1, [sp, #0x390]",
    "str x1, [sp, #0x08]",
    "ldp x2, x3, [x29]",
    "stp x2, x3, [sp, #0xf0]",
    "add x4, x29, #16",
    "stp x4, x3, [sp, #0x100]",
    "str x3, [x1, #0x10]",
    "mov x0, x1",
    "mov x1, sp",
    "bl {raise}",
    "brk #0xf000",
    "",
    ".globl KiUserExceptionDispatcher",
    ".p2align 2",
    "KiUserExceptionDispatcher:",
    "add x0, sp, #{record}",
    "mov x1, sp",
    "bl {dispatch}",
    "brk #0xf000",
    raise = sym crate::dispatch::raise,
    dispatch = sym crate::dispatch::dispatch_exception,
    record = const RECORD_OFFSET,
);
