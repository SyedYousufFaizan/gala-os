//! PCI configuration space through the host bridge's ECAM window (PCI
//! Express's enhanced configuration access mechanism), which the
//! firmware's MCFG names (`boot::acpi::parse_mcfg`): each function's 4 KiB
//! of configuration space at `base + (bus << 20 | device << 15 | function
//! << 12)`, plain loads and stores of Device memory. `init` maps segment
//! 0's buses once, before the drivers enumerate; without an MCFG (or
//! before `init`) every read sees no device, and the first one says why in
//! the log, so a boot without disk or network says so.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::pci::Address;
use crate::boot::BootInfo;
use crate::boot::acpi::Ecam;

/// The window, mapped (0: none), and its bus range.
static BASE: AtomicU64 = AtomicU64::new(0);
static BUSES: AtomicU64 = AtomicU64::new(0);
static SAID: AtomicBool = AtomicBool::new(false);

/// Maps the MCFG's window (segment 0's buses).
pub fn init(info: &BootInfo) {
    let Some(ecam) = info.pci_ecam else {
        crate::warn!(
            "pci: no MCFG from the firmware: no configuration space, no PCI device is seen"
        );
        return;
    };
    let len = (u64::from(ecam.end_bus - ecam.start_bus) + 1) << 20;
    match super::paging::map_mmio(ecam.base, len) {
        Ok(virt) => {
            BUSES
                .store(u64::from(ecam.start_bus) | u64::from(ecam.end_bus) << 8, Ordering::Relaxed);
            BASE.store(virt, Ordering::Release);
            crate::info!(
                "pci: configuration space through the MCFG's ECAM at {:#x} (segment {}, buses {}-{})",
                ecam.base,
                ecam.segment,
                ecam.start_bus,
                ecam.end_bus
            );
        }
        Err(e) => {
            crate::warn!("pci: the ECAM at {:#x} ({len:#x} bytes) not mapped ({e:?})", ecam.base)
        }
    }
}

/// The mapped register's virtual address, or None (no window, or a bus
/// outside it).
fn register(at: Address, offset: u8) -> Option<u64> {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        if !SAID.swap(true, Ordering::Relaxed) {
            crate::warn!(
                "pci: configuration space read with no ECAM window mapped: no device seen"
            );
        }
        return None;
    }
    let buses = BUSES.load(Ordering::Relaxed);
    let ecam = Ecam { base, segment: 0, start_bus: buses as u8, end_bus: (buses >> 8) as u8 };
    crate::arch::pci::ecam_offset(ecam, at, offset).map(|o| base + o)
}

pub fn read_u32(at: Address, offset: u8) -> u32 {
    match register(at, offset) {
        // SAFETY: a register of the window `init` mapped (Device memory);
        // configuration reads have no side effect on memory.
        Some(addr) => unsafe { (addr as *const u32).read_volatile() },
        None => u32::MAX,
    }
}

/// Writes the 16-bit register at `offset` (the command register: `arch::pci`
/// writes nothing else).
pub fn write_u16(at: Address, offset: u8, value: u16) {
    if let Some(addr) = register(at, offset) {
        // SAFETY: as `read_u32`; the halfword at `offset` of the dword
        // `register` found (the command register: decoding and bus
        // mastering for a device whose driver owns it).
        unsafe { ((addr + u64::from(offset & 2)) as *mut u16).write_volatile(value) }
    }
}
