//! The other CPUs. Limine has started them (its MP response): each waits
//! at EL1, on the bootloader's tables, until its `goto_address` is
//! written. `start_aps` gives each a kernel stack and its CPU index (the
//! `extra_argument`) and sends it to `gala_aarch64_ap_entry`, which turns
//! FP/SIMD on, takes the stack as SP_EL1 and the kernel's tables (MAIR,
//! TTBR1, its TLB invalidated), then `ap_main`: its per-CPU block, vectors
//! and GIC part, online, the scheduler (`sched::run_ap`, which starts its
//! tick). CPUs the bootloader did not start (PSCI CPU_ON) are not started
//! here yet.
//!
//! IPIs are SGIs: SGI 0 reschedules (its IRQ's return is the point).
//! A TLB flush everywhere needs none: `tlbi vmalle1is` reaches every CPU
//! of the inner shareable domain.

/// What a processor's hardware id is (`boot::MpInfo::hw_id`): MPIDR_EL1's affinity fields.
pub const HW_ID_NAME: &str = "MPIDR";

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{KernelStack, StackError};
use super::unported;
use crate::boot::{BootInfo, KernelAddress};
use crate::mm::heap::AllocError;
use crate::sync::IrqMutex;

/// The reschedule IPI.
const RESCHEDULE_SGI: u32 = 0;
/// How long an AP has to come online.
const AP_START_NS: u64 = 1_000_000_000;

/// Online CPUs, one bit per index.
static ONLINE: AtomicU64 = AtomicU64::new(0);
/// Each CPU's hardware id (MPIDR affinity), by index.
static HW_IDS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Each AP's kernel stack's top, read by its entry, by index.
static AP_STACK_TOPS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// The kernel's TTBR1 root, for the APs' entry.
static AP_ROOT: AtomicU64 = AtomicU64::new(0);
/// The APs' kernel stacks, kept for as long as they run.
static AP_STACKS: IrqMutex<[Option<KernelStack>; MAX_CPUS]> =
    IrqMutex::new([const { None }; MAX_CPUS]);
/// What an AP runs once set up (`start_aps`' `main`).
static AP_MAIN: IrqMutex<Option<fn(u32) -> !>> = IrqMutex::new(None);

// x0: the AP's `limine_mp_info`, its CPU index in `extra_argument`.
core::arch::global_asm!(
    ".section .text.ap_entry, \"ax\"",
    ".global gala_aarch64_ap_entry",
    "gala_aarch64_ap_entry:",
    "mov x9, #(3 << 20)",
    "msr cpacr_el1, x9",
    "isb",
    "ldr x19, [x0, #{extra}]",
    "adrp x9, {tops}",
    "add x9, x9, :lo12:{tops}",
    "ldr x10, [x9, x19, lsl #3]",
    "msr spsel, #1",
    "mov sp, x10",
    "adrp x9, {root}",
    "add x9, x9, :lo12:{root}",
    "ldr x12, [x9]",
    "ldr x11, ={mair}",
    "msr mair_el1, x11",
    "dsb ishst",
    "msr ttbr1_el1, x12",
    "isb",
    "tlbi vmalle1",
    "dsb nsh",
    "isb",
    "mov x29, xzr",
    "mov x0, x19",
    "bl {main}",
    "udf #0",
    extra = const crate::boot::MP_INFO_EXTRA_ARGUMENT,
    tops = sym AP_STACK_TOPS,
    root = sym AP_ROOT,
    mair = const super::paging::MAIR,
    main = sym ap_main,
);

unsafe extern "C" {
    fn gala_aarch64_ap_entry();
}

/// An AP on its own stack and the kernel's tables: its per-CPU block,
/// vectors and GIC part, then online and the scheduler.
extern "C" fn ap_main(index: u64) -> ! {
    let index = index as u32;
    super::percpu::install(index);
    super::entry::install_vectors();
    if let Err(e) = super::gic::init_cpu() {
        crate::warn!("smp: CPU {index}: no interrupt controller ({e:?}): it stays offline");
        super::cpu::halt_forever();
    }
    ONLINE.fetch_or(1 << index, Ordering::AcqRel);
    let main = *AP_MAIN.lock();
    match main {
        Some(main) => main(index),
        None => super::cpu::halt_forever(),
    }
}

pub const MAX_CPUS: usize = 64;

/// What became of the bootloader's AP tables (x86-64's late-AP shelter;
/// none here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelter {
    Added,
    NoTrampoline,
    AlreadyMapped,
    Unsupported,
    UnknownTables,
}

pub fn shelter_state() -> Option<Shelter> {
    None
}

pub fn shelter_bootloader_aps(
    _hhdm_offset: u64,
    _kernel: KernelAddress,
    _reclaimable: impl Iterator<Item = (u64, u64)>,
) {
}

/// (TLB shootdowns, IPIs sent for them).
pub fn flush_stats() -> (u64, u64) {
    (0, 0)
}

/// Every CPU's TLB, by broadcast (`tlbi vmalle1is`: no IPI).
pub fn flush_tlb_everywhere() {
    // SAFETY: TLB maintenance only.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        )
    };
}

pub fn set_tlb_paranoid(_on: bool) {}

pub fn tlb_paranoid() -> bool {
    false
}

pub fn moved_flushes() -> u64 {
    0
}

pub fn flush_user_space(_root: u64, _changed_on: Option<u32>) {
    unported("smp::flush_user_space")
}

pub fn online_count() -> u32 {
    online_mask().count_ones()
}

pub fn online_mask() -> u64 {
    ONLINE.load(Ordering::Acquire).max(1)
}

#[derive(Debug)]
pub enum SmpError {
    OutOfMemory,
    Stack(StackError),
}

impl From<AllocError> for SmpError {
    fn from(_: AllocError) -> Self {
        SmpError::OutOfMemory
    }
}

impl From<StackError> for SmpError {
    fn from(e: StackError) -> Self {
        SmpError::Stack(e)
    }
}

/// (CPU index, hardware id: MPIDR's affinity) of every online CPU.
pub fn online_cpus() -> impl Iterator<Item = (u32, u32)> {
    let mask = online_mask();
    (0..MAX_CPUS as u32)
        .filter(move |i| mask & (1 << i) != 0)
        .map(|i| (i, HW_IDS[i as usize].load(Ordering::Relaxed) as u32))
}

/// Wakes CPU `index` to reschedule (SGI 0).
pub fn send_reschedule(index: u32) {
    if index < MAX_CPUS as u32 && online_mask() & (1 << index) != 0 {
        super::gic::send_sgi(RESCHEDULE_SGI, HW_IDS[index as usize].load(Ordering::Relaxed));
    }
}

pub fn send_nmi(_hw_id: u32) {
    unported("smp::send_nmi")
}

/// The boot CPU online (index 0), and the reschedule SGI's handler: its
/// IRQ's return is the point (the scheduler runs then).
pub fn init_bsp() {
    HW_IDS[0].store(super::gic::my_affinity(), Ordering::Relaxed);
    ONLINE.fetch_or(1, Ordering::AcqRel);
    super::gic::enable_private(RESCHEDULE_SGI, |_| {});
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpTest {
    None,
    Hide,
    Late,
}

#[derive(Debug, Default)]
pub struct SmpReport {
    pub online: u32,
    pub reported: usize,
    pub firmware: Option<usize>,
    pub started_here: u32,
    pub failed: u32,
    pub first_failed: Option<u32>,
    pub late_replay: Option<&'static str>,
    pub slow: bool,
}

/// Starts the CPUs the bootloader reported (all but this one), each
/// running `main(index)` once set up; waits up to 1 s each to see it
/// online. `test` is x86-64's (hiding or delaying an AP from the
/// bootloader): not offered here.
pub fn start_aps(
    info: &BootInfo,
    test: SmpTest,
    main: fn(u32) -> !,
) -> Result<SmpReport, SmpError> {
    if test != SmpTest::None {
        crate::warn!("smp: smptest {test:?} is x86-64's: not done on aarch64");
    }
    *AP_MAIN.lock() = Some(main);
    AP_ROOT.store(super::paging::kernel_root(), Ordering::Release);
    let mut report = SmpReport {
        reported: info.processors.len(),
        firmware: info.firmware_cpus.as_ref().and_then(|r| r.as_ref().ok()).map(|f| f.len()),
        ..SmpReport::default()
    };
    let mut next = 1u32;
    for cpu in info.processors.iter() {
        if cpu.hw_id() == info.processors.bsp_id {
            continue;
        }
        if next as usize >= MAX_CPUS {
            break;
        }
        let index = next;
        next += 1;
        let stack = KernelStack::new()?;
        AP_STACK_TOPS[index as usize].store(stack.top(), Ordering::Release);
        HW_IDS[index as usize].store(cpu.hw_id(), Ordering::Relaxed);
        AP_STACKS.lock()[index as usize] = Some(stack);
        cpu.extra_argument.store(u64::from(index), Ordering::Release);
        cpu.goto_address.store(gala_aarch64_ap_entry as *const () as u64, Ordering::Release);
        let until = crate::time::raw_monotonic_ns() + AP_START_NS;
        while ONLINE.load(Ordering::Acquire) & (1 << index) == 0
            && crate::time::raw_monotonic_ns() < until
        {
            core::hint::spin_loop();
        }
        if ONLINE.load(Ordering::Acquire) & (1 << index) == 0 {
            report.failed += 1;
            report.first_failed.get_or_insert(cpu.hw_id() as u32);
            crate::warn!(
                "smp: CPU {index} (MPIDR {:#x}) did not come online within 1 s",
                cpu.hw_id()
            );
        }
    }
    report.online = online_count();
    Ok(report)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotStopped(pub u64);

pub fn stop_others<R>(
    _expired: impl Fn() -> bool,
    _f: impl FnOnce(&[u64; MAX_CPUS]) -> R,
) -> Result<R, NotStopped> {
    unported("smp::stop_others")
}

pub fn interrupt_stacks(_cpu: usize) -> [(u64, u64); 3] {
    [(0, 0); 3]
}

pub fn ap_stack_top(_cpu: usize) -> u64 {
    0
}
