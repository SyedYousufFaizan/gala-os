//! The tick source and the counter's rate: the generic timer's virtual
//! channel (CNTV, PPI 27 at the GIC), its counter CNTVCT_EL0 at the rate
//! the firmware put in CNTFRQ_EL0, which is architected: nothing to
//! calibrate. The tick is periodic by compare value (CNTV_CVAL_EL0 moved
//! on by one period at each tick, so the period does not drift with the
//! handler's latency).
//!
//! Limits: one CPU (`start_periodic_here`, the other CPUs' ticks, comes
//! with SMP); entered at EL2 with VHE the CNTV registers would be EL2's
//! virtual timer (PPI 28), not handled; no RTC is read yet (the PL031).

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{interrupts, unported};
use crate::boot::BootInfo;
use crate::sync::IrqMutex;

/// The virtual timer's interrupt (a PPI).
const VIRTUAL_TIMER_PPI: u32 = 27;
/// CNTV_CTL_EL0: ENABLE (IMASK clear).
const CTL_ENABLE: u64 = 1;

/// Counter ticks per timer tick (0: not started).
static PERIOD: AtomicU64 = AtomicU64::new(0);
/// The compare value of the next tick.
static NEXT: AtomicU64 = AtomicU64::new(0);
static HANDLER: IrqMutex<Option<interrupts::Handler>> = IrqMutex::new(None);

fn counter_frequency() -> u64 {
    let f: u64;
    // SAFETY: reads CNTFRQ_EL0.
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags))
    };
    f
}

fn set_compare(value: u64) {
    // SAFETY: programs this CPU's virtual timer (compare value, enabled).
    unsafe {
        core::arch::asm!(
            "msr cntv_cval_el0, {cval}",
            "msr cntv_ctl_el0, {ctl}",
            "isb",
            cval = in(reg) value,
            ctl = in(reg) CTL_ENABLE,
            options(nomem, nostack, preserves_flags)
        )
    };
}

/// The timer's IRQ: the next compare value one period on (from the last,
/// or from now if the handler fell more than a period behind), then the
/// tick's handler.
fn on_interrupt(frame: &mut interrupts::TrapFrame) {
    let period = PERIOD.load(Ordering::Relaxed);
    let now = super::cpu::cycles();
    let mut next = NEXT.load(Ordering::Relaxed) + period;
    if next <= now {
        next = now + period;
    }
    NEXT.store(next, Ordering::Relaxed);
    set_compare(next);
    let handler = *HANDLER.lock();
    if let Some(h) = handler {
        h(frame);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    counter_hz: u64,
    timer_hz: u64,
}

/// The counter's and the timer's rate: CNTFRQ_EL0's (one counter drives
/// both). Nothing is measured, so no stall applies; a kernel handover's
/// carried rates are the same counter's.
pub fn calibrate(_info: &BootInfo, _stall: bool, carried: Option<(u64, u64)>) -> Calibration {
    let hz = counter_frequency();
    match carried {
        Some((counter_hz, timer_hz)) => Calibration { counter_hz, timer_hz },
        None => Calibration { counter_hz: hz, timer_hz: hz },
    }
}

pub fn recalibrate(_old: &Calibration) -> Option<Calibration> {
    None
}

/// Starts the boot CPU's tick at `hz`, calling `handler` at each (the GIC
/// is brought up first: the timer is its first source).
pub fn start_periodic(c: &Calibration, hz: u64, handler: interrupts::Handler) {
    if let Err(e) = super::gic::init() {
        panic!("timer: no interrupt controller: {e:?}");
    }
    *HANDLER.lock() = Some(handler);
    let period = (c.timer_hz() / hz.max(1)).max(1);
    PERIOD.store(period, Ordering::Relaxed);
    super::gic::enable_private(VIRTUAL_TIMER_PPI, on_interrupt);
    let first = super::cpu::cycles() + period;
    NEXT.store(first, Ordering::Relaxed);
    set_compare(first);
}

pub fn start_periodic_here(_c: &Calibration, _hz: u64) {
    unported("timer::start_periodic_here")
}

pub fn eoi() {}

/// The wall clock's seconds (a PL031 RTC).
pub fn rtc_seconds() -> Option<u8> {
    None
}

impl Calibration {
    pub fn counter_hz(&self) -> u64 {
        self.counter_hz
    }

    pub fn timer_hz(&self) -> u64 {
        self.timer_hz
    }

    pub fn set_rates(&mut self, counter_hz: u64, timer_hz: u64) {
        (self.counter_hz, self.timer_hz) = (counter_hz, timer_hz);
    }

    pub fn rescale(&mut self, counter_hz: u64) {
        self.timer_hz = (u128::from(self.timer_hz) * u128::from(counter_hz)
            / u128::from(self.counter_hz.max(1))) as u64;
        self.counter_hz = counter_hz;
    }

    pub fn reliable(&self) -> bool {
        true
    }

    pub fn source_name(&self) -> &'static str {
        "CNTFRQ_EL0"
    }

    pub fn stalls(&self) -> u32 {
        0
    }

    pub fn describe(&self) -> impl fmt::Display + '_ {
        "the architected rate"
    }

    /// The boot log's line: the counter's rate and where it came from.
    pub fn log(&self) {
        crate::info!(
            "timer: generic timer (virtual channel, PPI {VIRTUAL_TIMER_PPI}), counter at {} kHz \
             ({}, architected: not measured)",
            self.counter_hz / 1000,
            self.source_name()
        );
    }

    pub fn log_started(&self) {}

    pub fn emit_event(&self) {}
}
