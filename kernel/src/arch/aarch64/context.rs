//! Kernel stacks and the low-level thread context switch.
//!
//! A kernel stack is a slot of a window as x86-64's: an unmapped guard
//! area, then the stack's pages (`paging::map_kernel_page`); its unmap
//! invalidates the pages on every CPU (`tlbi vaae1is`).
//!
//! The switch saves the AAPCS64's callee-saved registers (x19-x28, the
//! frame pointer x29, the link register x30) on the outgoing stack, its SP
//! in the context, and the FP/SIMD registers (V0-V31, FPCR, FPSR: user
//! mode's, the kernel being soft-float) in the context's save area; then
//! the reverse for the incoming one.

use core::arch::global_asm;
use core::sync::atomic::AtomicU64;

use super::paging;
use super::xstate::{NoMemory, XState};
use crate::mm::paging::PageFlags;
use crate::mm::{self, PAGE_SIZE};
use crate::sync::IrqMutex;

/// Kernel stacks' window: fixed-size slots, each a guard area (never
/// mapped) followed by the stack pages.
const STACK_WINDOW_START: u64 = 0xffff_d000_0000_0000;
const SLOT_SIZE: u64 = 128 * 1024;
const MAX_SLOTS: usize = 1024;
/// Default kernel stack size; the rest of the slot below it is guard.
pub const STACK_PAGES: usize = 16;

static SLOTS: IrqMutex<[u64; MAX_SLOTS / 64]> = IrqMutex::new([0; MAX_SLOTS / 64]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    TooManyStacks,
    OutOfMemory,
}

/// A mapped kernel stack with an unmapped guard region below it. Unmapped
/// and freed on drop.
#[derive(Debug)]
pub struct KernelStack {
    slot: usize,
    pages: usize,
}

impl KernelStack {
    pub fn new() -> Result<Self, StackError> {
        let slot = {
            let mut slots = SLOTS.lock();
            let (word, bit) = slots
                .iter()
                .enumerate()
                .find(|(_, w)| **w != u64::MAX)
                .map(|(i, w)| (i, w.trailing_ones() as usize))
                .ok_or(StackError::TooManyStacks)?;
            slots[word] |= 1 << bit;
            word * 64 + bit
        };
        let mut stack = KernelStack { slot, pages: 0 };
        for i in 0..STACK_PAGES {
            let frame = mm::allocate_frame(mm::Use::KernelStack).ok_or(StackError::OutOfMemory)?;
            let virt = stack.bottom() + i as u64 * PAGE_SIZE;
            if paging::map_kernel_page(virt, frame, PageFlags::KERNEL_RW).is_err() {
                let _ = mm::free_frame(frame);
                return Err(StackError::OutOfMemory); // `stack`'s drop frees the rest
            }
            stack.pages += 1;
        }
        Ok(stack)
    }

    /// Lowest mapped address.
    fn bottom(&self) -> u64 {
        STACK_WINDOW_START + (self.slot as u64 + 1) * SLOT_SIZE - STACK_PAGES as u64 * PAGE_SIZE
    }

    /// One past the highest stack address (16-byte aligned).
    pub fn top(&self) -> u64 {
        self.bottom() + STACK_PAGES as u64 * PAGE_SIZE
    }

    /// The guard region `[start, end)` below the stack.
    pub fn guard(&self) -> core::ops::Range<u64> {
        STACK_WINDOW_START + self.slot as u64 * SLOT_SIZE..self.bottom()
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        // Each unmap invalidates its page on every CPU before the frame
        // is freed (`paging::unmap_kernel_page`).
        for i in 0..self.pages {
            if let Some(frame) = paging::unmap_kernel_page(self.bottom() + i as u64 * PAGE_SIZE) {
                let _ = mm::free_frame(frame);
            }
        }
        SLOTS.lock()[self.slot / 64] &= !(1 << (self.slot % 64));
    }
}

/// A switched-out thread's state: its SP (at 0, for the switch) and its
/// FP/SIMD save area's address (at 8: a `Box` is one pointer).
#[repr(C)]
pub struct Context {
    sp: AtomicU64,
    area: alloc::boxed::Box<XState>,
}

impl core::fmt::Debug for Context {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Context").field("sp", &self.sp).finish_non_exhaustive()
    }
}

/// The callee-saved registers the switch pushes: x19-x28, x29, x30.
const SAVED_REGS: usize = 12;
const _: () = assert!(core::mem::offset_of!(Context, area) == 8);

impl Context {
    /// The context of the thread already running (saved by the first
    /// switch away from it).
    pub fn current() -> Result<Self, NoMemory> {
        let area = crate::mm::heap::try_box(XState::initial()).map_err(|_| NoMemory)?;
        Ok(Context { sp: AtomicU64::new(0), area })
    }

    /// A context that starts `entry` on `stack` when switched to (with
    /// interrupts masked, as the switch runs); `entry` must not return.
    pub fn new_thread(stack: &KernelStack, entry: extern "C" fn() -> !) -> Result<Self, NoMemory> {
        let area = crate::mm::heap::try_box(XState::initial()).map_err(|_| NoMemory)?;
        // The frame the switch pops: x19 (`entry`), x20-x28, x29 (0: the
        // backtrace's end), x30 (the trampoline the `ret` goes to).
        let mut frame = [0u64; SAVED_REGS];
        frame[0] = entry as *const () as u64;
        frame[11] = gala_thread_trampoline as *const () as u64;
        let sp = stack.top() - (frame.len() * 8) as u64;
        // SAFETY: `[sp, top)` lies in the mapped stack, unused before the
        // thread first runs.
        unsafe {
            let p = core::ptr::with_exposed_provenance_mut::<u64>(sp as usize);
            p.copy_from_nonoverlapping(frame.as_ptr(), frame.len());
        }
        Ok(Context { sp: AtomicU64::new(sp), area })
    }

    /// The SP the last switch away from this context saved (0 before the
    /// first).
    pub fn saved_sp(&self) -> u64 {
        self.sp.load(core::sync::atomic::Ordering::Acquire)
    }

    /// The FP/SIMD state the last switch away from this context saved.
    ///
    /// # Safety
    /// The thread is not running on any CPU (the switch writes the area).
    pub unsafe fn saved_xstate(&self) -> XState {
        (*self.area).clone()
    }
}

// x0 = from, x1 = to. The FP/SIMD instructions are assembled although the
// kernel is soft-float (it never runs them elsewhere).
global_asm!(
    r#"
    .arch_extension fp
    .arch_extension simd
    .section .text.context, "ax"
    .global gala_context_switch
gala_context_switch:
    ldr x8, [x0, #8]
    stp q0, q1, [x8, #0]
    stp q2, q3, [x8, #32]
    stp q4, q5, [x8, #64]
    stp q6, q7, [x8, #96]
    stp q8, q9, [x8, #128]
    stp q10, q11, [x8, #160]
    stp q12, q13, [x8, #192]
    stp q14, q15, [x8, #224]
    stp q16, q17, [x8, #256]
    stp q18, q19, [x8, #288]
    stp q20, q21, [x8, #320]
    stp q22, q23, [x8, #352]
    stp q24, q25, [x8, #384]
    stp q26, q27, [x8, #416]
    stp q28, q29, [x8, #448]
    stp q30, q31, [x8, #480]
    mrs x9, fpcr
    mrs x10, fpsr
    add x8, x8, #512
    stp x9, x10, [x8]
    sub sp, sp, #96
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    mov x9, sp
    str x9, [x0]
    ldr x9, [x1]
    mov sp, x9
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    add sp, sp, #96
    ldr x8, [x1, #8]
    ldp q0, q1, [x8, #0]
    ldp q2, q3, [x8, #32]
    ldp q4, q5, [x8, #64]
    ldp q6, q7, [x8, #96]
    ldp q8, q9, [x8, #128]
    ldp q10, q11, [x8, #160]
    ldp q12, q13, [x8, #192]
    ldp q14, q15, [x8, #224]
    ldp q16, q17, [x8, #256]
    ldp q18, q19, [x8, #288]
    ldp q20, q21, [x8, #320]
    ldp q22, q23, [x8, #352]
    ldp q24, q25, [x8, #384]
    ldp q26, q27, [x8, #416]
    ldp q28, q29, [x8, #448]
    ldp q30, q31, [x8, #480]
    add x8, x8, #512
    ldp x9, x10, [x8]
    msr fpcr, x9
    msr fpsr, x10
    ret

    .global gala_thread_trampoline
gala_thread_trampoline:
    blr x19
    udf #0
    "#
);

unsafe extern "C" {
    /// `from` and `to` are `Context`s (the layout above: SP at 0, the
    /// save area's address at 8).
    fn gala_context_switch(from: *const core::ffi::c_void, to: *const core::ffi::c_void);
    fn gala_thread_trampoline();
}

/// Saves the current thread's state into `from` and resumes `to`; returns
/// when something switches back to `from`.
///
/// # Safety
/// Interrupts are masked. `to` holds a context saved by `switch` or made
/// by `Context::new_thread`, its stack still mapped, running nowhere
/// else; `from` stays alive until it is switched back to (or never runs
/// again).
pub unsafe fn switch(from: &Context, to: &Context) {
    // SAFETY: forwarded to the caller; the assembly writes `from`'s SP and
    // save area and reads `to`'s.
    unsafe { gala_context_switch((from as *const Context).cast(), (to as *const Context).cast()) }
}
