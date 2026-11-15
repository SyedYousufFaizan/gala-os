//! The interrupt controller: a GICv3 (Arm's GIC Architecture
//! Specification, v3 and v4). QEMU `virt`'s (with `gic-version=3`): the
//! distributor at 0x0800_0000 and the redistributors from 0x080a_0000, the
//! board's fixed layout like the UART's (the firmware's MADT names them
//! on other machines: the port reads it). One security state (QEMU's
//! `virt` without `secure=on`): GICD_CTLR's DS is set, group 1 is the
//! kernel's.
//!
//! Brought up with its first interrupt source (`timer::start_periodic`):
//! the distributor (affinity routing, group 1 on), this CPU's
//! redistributor (awake; its private interrupts, SGIs and PPIs, enabled
//! one by one) and the CPU interface through its system registers
//! (`ICC_*`: enabled, every priority let through). `dispatch` acknowledges
//! an IRQ (ICC_IAR1_EL1), runs its handler and ends it (ICC_EOIR1_EL1).

use core::sync::atomic::{AtomicU64, Ordering};

use super::interrupts::{Handler, TrapFrame};
use crate::sync::IrqMutex;

const GICD_PHYS: u64 = 0x0800_0000;
const GICR_PHYS: u64 = 0x080a_0000;
/// How much of the redistributor region is searched for this CPU's (one
/// redistributor is 128 KiB: its RD_base frame, then its SGI_base frame).
const GICR_SPAN: u64 = 0x00f6_0000;
const GICR_STRIDE: u64 = 0x2_0000;

const GICD_CTLR: u64 = 0x0000;
const GICD_CTLR_RWP: u32 = 1 << 31;
/// EnableGrp0, EnableGrp1, ARE (single security state: DS).
const GICD_CTLR_ENABLE: u32 = (1 << 0) | (1 << 1) | (1 << 4);

const GICR_TYPER: u64 = 0x0008;
const GICR_TYPER_LAST: u64 = 1 << 4;
const GICR_WAKER: u64 = 0x0014;
const WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;
/// In the SGI_base frame (64 KiB after RD_base).
const SGI_BASE: u64 = 0x1_0000;
const GICR_IGROUPR0: u64 = SGI_BASE + 0x0080;
const GICR_ISENABLER0: u64 = SGI_BASE + 0x0100;
const GICR_IPRIORITYR: u64 = SGI_BASE + 0x0400;

/// INTIDs 1020-1023 are special: nothing to handle (spurious).
const SPURIOUS: u64 = 1020;
/// The private interrupts (SGIs 0-15, PPIs 16-31) handled here.
const PRIVATE: usize = 32;

/// The redistributor's RD_base for this CPU, virtual (0: not up).
static RD_BASE: AtomicU64 = AtomicU64::new(0);
static HANDLERS: IrqMutex<[Option<Handler>; PRIVATE]> = IrqMutex::new([None; PRIVATE]);

/// GIC bring-up failed: the boot cannot have interrupts.
#[derive(Debug)]
pub enum GicError {
    Map(super::paging::PagingError),
    /// No redistributor has this CPU's affinity.
    NoRedistributor(u64),
}

fn read32(addr: u64) -> u32 {
    // SAFETY: a GIC register in a mapping `init` made (Device memory).
    unsafe { (addr as *const u32).read_volatile() }
}

fn write32(addr: u64, value: u32) {
    // SAFETY: as read32.
    unsafe { (addr as *mut u32).write_volatile(value) }
}

fn read64(addr: u64) -> u64 {
    // SAFETY: as read32.
    unsafe { (addr as *const u64).read_volatile() }
}

/// This CPU's affinity in GICR_TYPER's form: Aff3.Aff2.Aff1.Aff0 in 32 bits.
fn affinity() -> u64 {
    let mpidr: u64;
    // SAFETY: reads MPIDR_EL1.
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags))
    };
    (mpidr & 0xff_ffff) | ((mpidr >> 8) & 0xff00_0000)
}

/// Brings the GIC up for this CPU (once).
pub fn init() -> Result<(), GicError> {
    if RD_BASE.load(Ordering::Acquire) != 0 {
        return Ok(());
    }
    let gicd = super::paging::map_mmio(GICD_PHYS, 0x1_0000).map_err(GicError::Map)?;
    write32(gicd + GICD_CTLR, GICD_CTLR_ENABLE);
    for _ in 0..1_000_000 {
        if read32(gicd + GICD_CTLR) & GICD_CTLR_RWP == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    let gicr = super::paging::map_mmio(GICR_PHYS, GICR_SPAN).map_err(GicError::Map)?;
    let me = affinity();
    let mut rd = None;
    let mut at = gicr;
    while at < gicr + GICR_SPAN {
        let typer = read64(at + GICR_TYPER);
        if typer >> 32 == me {
            rd = Some(at);
            break;
        }
        if typer & GICR_TYPER_LAST != 0 {
            break;
        }
        at += GICR_STRIDE;
    }
    let rd = rd.ok_or(GicError::NoRedistributor(me))?;
    // Awake: the redistributor forwards interrupts to the CPU interface.
    write32(rd + GICR_WAKER, read32(rd + GICR_WAKER) & !WAKER_PROCESSOR_SLEEP);
    for _ in 0..1_000_000 {
        if read32(rd + GICR_WAKER) & WAKER_CHILDREN_ASLEEP == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    // Every private interrupt in group 1 (none enabled yet).
    write32(rd + GICR_IGROUPR0, u32::MAX);
    // SAFETY: the CPU interface's system registers (ICC_SRE_EL1,
    // ICC_PMR_EL1, ICC_BPR1_EL1, ICC_IGRPEN1_EL1 by their encodings):
    // system register access on, every priority unmasked, no preemption
    // grouping, group 1 on.
    unsafe {
        core::arch::asm!(
            "mrs {t}, S3_0_C12_C12_5",
            "orr {t}, {t}, #1",
            "msr S3_0_C12_C12_5, {t}",
            "isb",
            "mov {t}, #0xff",
            "msr S3_0_C4_C6_0, {t}",
            "msr S3_0_C12_C12_3, xzr",
            "mov {t}, #1",
            "msr S3_0_C12_C12_7, {t}",
            "isb",
            t = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
    RD_BASE.store(rd, Ordering::Release);
    crate::info!(
        "gic: GICv3, distributor at {GICD_PHYS:#x}, this CPU's redistributor (affinity {me:#x}) at {:#x}",
        GICR_PHYS + (rd - gicr)
    );
    Ok(())
}

/// Enables private interrupt `intid` (an SGI or PPI) on this CPU, at the
/// middle priority, with `handler`.
pub fn enable_private(intid: u32, handler: Handler) {
    let rd = RD_BASE.load(Ordering::Acquire);
    let i = intid as usize;
    if rd == 0 || i >= PRIVATE {
        crate::warn!("gic: interrupt {intid} not enabled (GIC up: {})", rd != 0);
        return;
    }
    HANDLERS.lock()[i] = Some(handler);
    // SAFETY: one byte of GICR_IPRIORITYR per interrupt.
    unsafe { ((rd + GICR_IPRIORITYR + u64::from(intid)) as *mut u8).write_volatile(0x80) };
    write32(rd + GICR_ISENABLER0, 1 << intid);
}

/// Handles one IRQ (the IRQ vector's): acknowledged, its handler run with
/// the interrupted state, then ended. An interrupt without a handler is
/// logged once per INTID.
pub fn dispatch(frame: &mut TrapFrame) {
    let iar: u64;
    // SAFETY: ICC_IAR1_EL1: acknowledges the highest pending group 1 IRQ.
    unsafe {
        core::arch::asm!("mrs {}, S3_0_C12_C12_0", out(reg) iar, options(nomem, nostack, preserves_flags))
    };
    let intid = iar & 0xff_ffff;
    if intid >= SPURIOUS {
        return;
    }
    let handler = HANDLERS.lock().get(intid as usize).copied().flatten();
    match handler {
        Some(h) => h(frame),
        None => {
            static UNHANDLED: AtomicU64 = AtomicU64::new(0);
            if intid < 64 && UNHANDLED.fetch_or(1 << intid, Ordering::Relaxed) & (1 << intid) == 0 {
                crate::warn!("gic: interrupt {intid} has no handler");
            }
        }
    }
    // SAFETY: ICC_EOIR1_EL1: ends the interrupt acknowledged above.
    unsafe {
        core::arch::asm!("msr S3_0_C12_C12_1, {}", in(reg) iar, options(nomem, nostack, preserves_flags))
    };
}
