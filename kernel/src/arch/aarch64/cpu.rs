//! The processor: interrupt masking (PSTATE's DAIF.I), halting (WFI), the
//! free-running counter (CNTVCT_EL0). The random generator (RNDR) is not
//! read yet.

/// Halts this CPU for good: interrupts masked, waiting forever.
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: WFI only waits; with IRQs masked nothing is handled.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}

/// Whether IRQs are unmasked (DAIF.I clear).
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: reads PSTATE's interrupt masks.
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags))
    };
    daif & (1 << 7) == 0
}

/// Masks interrupts; whether they were enabled.
pub fn disable_interrupts() -> bool {
    let was = interrupts_enabled();
    // SAFETY: sets DAIF.I (IRQs masked); no memory is touched.
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
    was
}

/// Unmasks IRQs (DAIF.I clear).
pub fn enable_interrupts() {
    // SAFETY: clears DAIF.I; the vectors are installed (init_boot_cpu).
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
}

/// Unmasks IRQs and waits for one (WFI wakes on a pending IRQ even
/// while it is masked, so none is missed between the two).
pub fn enable_interrupts_and_halt() {
    // SAFETY: as enable_interrupts, then WFI.
    unsafe { core::arch::asm!("msr daifclr, #2", "wfi", options(nomem, nostack, preserves_flags)) };
}

/// Called by `IrqMutex` while it spins for a held lock.
pub fn lock_contended() {
    core::hint::spin_loop();
}

/// The free-running counter the clock reads (CNTVCT_EL0).
pub fn cycles() -> u64 {
    let v: u64;
    // SAFETY: reads the virtual counter (always readable at EL1).
    unsafe {
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

/// One random word from the processor's generator (RNDR): not read yet
/// (FEAT_RNG is optional; the port checks ID_AA64ISAR0_EL1 first).
pub fn hw_random() -> Option<u64> {
    None
}
