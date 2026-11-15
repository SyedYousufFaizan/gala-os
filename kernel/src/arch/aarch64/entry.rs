//! The kernel's entry from Limine and its exception vectors.
//!
//! Limine (base revision 6) enters at EL1, or at EL2 with VHE, with the
//! MMU on, interrupts masked, a 64 KiB stack and FP/SIMD trapped
//! (`CPACR_EL1` = 0): Rust code may use the SIMD registers at any
//! instruction, so the entry turns FP/SIMD on before the first one, then
//! calls `kmain` (its arguments zero: not a kernel handover), on the boot
//! stack moved to SP_EL1 (the protocol does not say which SP is in use,
//! and an exception at EL1 runs on SP_EL1: a kernel on SP_EL0 would take
//! its first exception on a stale stack). With VHE the `_EL1` names reach
//! the EL2 registers, so the same code serves both.
//!
//! The vectors (`VBAR_EL1`, 16 entries of 128 bytes): every exception is
//! reported and stops the kernel for now (`trap`); the port gives them
//! their handlers (IRQs, system calls, user faults).

core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global gala_aarch64_entry",
    "gala_aarch64_entry:",
    // CPACR_EL1.FPEN = 0b11: FP/SIMD not trapped at EL0 or EL1.
    "mov x9, #(3 << 20)",
    "msr cpacr_el1, x9",
    "isb",
    // The stack Limine gave, as SP_EL1: Limine may enter with SPSel 0
    // (on SP_EL0), and an exception taken at EL1 switches to SP_EL1,
    // which would hold whatever the firmware left there.
    "mov x9, sp",
    "msr spsel, #1",
    "mov sp, x9",
    "mov x0, xzr",
    "mov x1, xzr",
    "mov x2, xzr",
    "b kmain",
    "",
    ".section .text.vectors, \"ax\"",
    ".balign 2048",
    ".global gala_aarch64_vectors",
    "gala_aarch64_vectors:",
    ".irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
    ".balign 128",
    "mov x0, #\\n",
    "b gala_aarch64_trap_entry",
    ".endr",
    "",
    "gala_aarch64_trap_entry:",
    "mrs x1, esr_el1",
    "mrs x2, elr_el1",
    "mrs x3, far_el1",
    "mrs x4, spsr_el1",
    "bl {trap}",
    trap = sym trap,
);

unsafe extern "C" {
    static gala_aarch64_vectors: u8;
}

/// Installs the exception vectors on this CPU.
pub fn install_vectors() {
    let base = (&raw const gala_aarch64_vectors) as u64;
    // SAFETY: the table above, 2 KiB aligned, holds the 16 entries.
    unsafe {
        core::arch::asm!("msr vbar_el1, {}", "isb", in(reg) base, options(nostack, preserves_flags))
    };
}

/// The 16 vectors in table order: the exception's kind (synchronous, IRQ,
/// FIQ, SError) for each origin (the current EL on SP_EL0, on SP_ELx, a
/// lower EL in AArch64, in AArch32).
const VECTORS: [&str; 16] = [
    "synchronous from EL1 with SP_EL0",
    "IRQ from EL1 with SP_EL0",
    "FIQ from EL1 with SP_EL0",
    "SError from EL1 with SP_EL0",
    "synchronous from EL1 with SP_EL1",
    "IRQ from EL1 with SP_EL1",
    "FIQ from EL1 with SP_EL1",
    "SError from EL1 with SP_EL1",
    "synchronous from EL0 (AArch64)",
    "IRQ from EL0 (AArch64)",
    "FIQ from EL0 (AArch64)",
    "SError from EL0 (AArch64)",
    "synchronous from EL0 (AArch32)",
    "IRQ from EL0 (AArch32)",
    "FIQ from EL0 (AArch32)",
    "SError from EL0 (AArch32)",
];

/// Every exception for now: reported with its syndrome (ESR: the class in
/// bits 26-31), the address it was taken at (ELR), the faulting address
/// (FAR, for aborts) and the saved PSTATE, then the kernel stops (the
/// panic's report and exit).
extern "C" fn trap(vector: u64, esr: u64, elr: u64, far: u64, spsr: u64) -> ! {
    panic!(
        "aarch64: exception: {} (vector {vector}): {} (ESR {esr:#x}, class {:#x}) at {elr:#x}, \
         FAR {far:#x}, SPSR {spsr:#x}: not handled yet",
        VECTORS.get(vector as usize).copied().unwrap_or("?"),
        super::interrupts::exception_name((esr >> 26) as u8),
        esr >> 26
    )
}
