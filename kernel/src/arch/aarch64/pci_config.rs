//! PCI configuration space (stub: the host bridge's ECAM window, which the
//! MCFG names, is not mapped yet: ROADMAP AR). Every read sees no device,
//! so enumeration finds none; the first read says so in the log, so a
//! boot without disk or network says why.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::pci::Address;

static SAID: AtomicBool = AtomicBool::new(false);

pub fn read_u32(_at: Address, _offset: u8) -> u32 {
    if !SAID.swap(true, Ordering::Relaxed) {
        crate::warn!(
            "pci: no configuration space on aarch64 yet (the ECAM from the MCFG comes with the \
             port): no PCI device is seen"
        );
    }
    u32::MAX
}

pub fn write_u16(_at: Address, _offset: u8, _value: u16) {}
