//! Device pages mapped into the bootloader's page tables, for what the
//! kernel needs before it has its own (the console's UART).
//!
//! Limine's higher half (TTBR1_EL1, 4 KiB granule, 4 levels: T1SZ 16)
//! maps the kernel, the HHDM of memory and nothing else: no device. Each
//! device page here goes at its own virtual address in `WINDOW` (below the
//! kernel's image), with the attribute index of Device-nGnRE, which this
//! module puts in MAIR_EL1's slot 2 (Limine guarantees slots 2-7 unused).
//! The tables the walk needs are taken from a small pool in the kernel's
//! own .bss (its physical address from the kernel's load address); the
//! bootloader's tables are reached through the HHDM. The port's own page
//! tables (`paging`) map these devices again when they take over.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::boot::KernelAddress;

/// Where device pages go: the 2 MiB below the kernel's image
/// (0xffff_ffff_8000_0000), one 4 KiB page each.
const WINDOW: u64 = 0xffff_ffff_7fe0_0000;
const WINDOW_PAGES: usize = 512;
const PAGE: u64 = 4096;

/// MAIR_EL1 slot this module sets to Device-nGnRE (0x04).
const DEVICE_ATTR: u64 = 2;

/// Descriptor bits (VMSAv8-64, 4 KiB granule).
const VALID_TABLE: u64 = 0b11;
const VALID_PAGE: u64 = 0b11;
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ADDRESS: u64 = 0x0000_ffff_ffff_f000;

/// Page tables for the walk (an L2 and an L3 cover the whole window; one
/// spare in case the bootloader's L1 is shared differently).
#[repr(C, align(4096))]
struct Pool([[u64; 512]; 3]);
static mut POOL: Pool = Pool([[0; 512]; 3]);
static POOL_USED: AtomicUsize = AtomicUsize::new(0);
static NEXT_PAGE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// No more pages in `WINDOW` or tables in the pool.
    Full,
    /// The bootloader's tables map a block where a table was expected.
    Block,
}

/// Maps the device page at physical `phys` (4 KiB aligned) and returns its
/// virtual address. `hhdm` and `kernel` are the bootloader's
/// (`boot::early_addresses`).
pub fn map_device(phys: u64, hhdm: u64, kernel: KernelAddress) -> Result<u64, MapError> {
    let n = NEXT_PAGE.fetch_add(1, Ordering::Relaxed);
    if n >= WINDOW_PAGES {
        return Err(MapError::Full);
    }
    let va = WINDOW + n as u64 * PAGE;
    set_device_attr();
    let root: u64;
    // SAFETY: reads the higher half's translation table base.
    unsafe {
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) root, options(nomem, nostack, preserves_flags))
    };
    let mut table = root & ADDRESS;
    for level in 0..3 {
        let index = (va >> (39 - 9 * level)) & 511;
        // SAFETY: `table` is a translation table (the bootloader's, in
        // memory the HHDM maps, or one of the pool's); `index` < 512.
        let entry = unsafe { ((table + hhdm) as *mut u64).add(index as usize) };
        // SAFETY: as above; the walk runs before any other CPU starts.
        let desc = unsafe { entry.read_volatile() };
        table = if desc & 1 == 0 {
            let fresh = take_table(kernel)?;
            // SAFETY: as above; the new table is zeroed (pool in .bss).
            unsafe { entry.write_volatile(fresh | VALID_TABLE) };
            fresh
        } else if desc & 0b11 == VALID_TABLE {
            desc & ADDRESS
        } else {
            return Err(MapError::Block);
        };
    }
    let index = (va >> 12) & 511;
    let page = (phys & ADDRESS) | VALID_PAGE | (DEVICE_ATTR << 2) | AF | PXN | UXN;
    // SAFETY: the level-3 table found or made above; `index` < 512.
    unsafe { ((table + hhdm) as *mut u64).add(index as usize).write_volatile(page) };
    // The entry was invalid before: no TLB entry to remove, only the
    // write to make visible to the walker before the first access.
    // SAFETY: barriers only.
    unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
    Ok(va)
}

/// A zeroed table from the pool: its physical address.
fn take_table(kernel: KernelAddress) -> Result<u64, MapError> {
    let i = POOL_USED.fetch_add(1, Ordering::Relaxed);
    if i >= 3 {
        return Err(MapError::Full);
    }
    // SAFETY: only the address of the i-th table, handed out once.
    let virt = unsafe { (&raw mut POOL.0[i]) as u64 };
    Ok(virt - kernel.virtual_base + kernel.physical_base)
}

/// MAIR_EL1's slot `DEVICE_ATTR` := Device-nGnRE.
fn set_device_attr() {
    let mut mair: u64;
    // SAFETY: reads and writes MAIR_EL1's slot this module owns.
    unsafe {
        core::arch::asm!("mrs {}, mair_el1", out(reg) mair, options(nomem, nostack, preserves_flags));
        mair = (mair & !(0xff << (8 * DEVICE_ATTR))) | (0x04 << (8 * DEVICE_ATTR));
        core::arch::asm!("msr mair_el1, {}", "isb", in(reg) mair, options(nomem, nostack, preserves_flags));
    }
}
