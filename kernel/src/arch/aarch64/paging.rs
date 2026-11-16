//! The kernel's address space on aarch64: its own translation tables for
//! the higher half (TTBR1_EL1, VMSAv8-64 with the 4 KiB granule:
//! `mm::paging::Mapper` with the `Vmsa` encoding), built and switched to
//! once the frame allocator is up, as x86-64's. User address spaces
//! (TTBR0_EL1) come with the port's user mode (ROADMAP AR); their
//! functions below are still the stub's.

use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

use super::unported;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::memmap::{MemoryRegion, RegionKind};
use crate::mm::paging::{
    CacheMode, MapError, Mapper, PageFlags, PageSize, PageTable, TableMemory, Translation, Vmsa,
};
use crate::sync::IrqMutex;

/// MAIR_EL1: one attribute per `CacheMode`, at its value (the `Vmsa`
/// encoding's AttrIndx): 0 write-back (0xff), 1 write-through (0xbb), 2
/// UC- as Device-nGnRE (0x04: `early_map`'s slot), 3 UC as Device-nGnRnE
/// (0x00), 4 write-protect as write-through (0xbb: VMSA has no such type,
/// the leaf is read-only anyway), 5 write-combining as Normal
/// non-cacheable (0x44). Slot 0 is Limine's write-back, as before.
pub const MAIR: u64 = 0x0000_44bb_0004_bbff;

unsafe extern "C" {
    static __kernel_start: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __kernel_end: u8;
}

fn sym(s: &u8) -> u64 {
    s as *const u8 as u64
}

/// Translation tables accessed through the higher-half direct map.
pub struct HhdmTables {
    offset: u64,
}

impl TableMemory for HhdmTables {
    fn table(&mut self, frame: Frame) -> &mut PageTable {
        let virt = self.offset + frame.start();
        // SAFETY: table frames come from the frame allocator (usable RAM,
        // HHDM-mapped, owned by this address space) and are only reached
        // through the `Mapper` that owns this `HhdmTables`, one at a time.
        unsafe { &mut *core::ptr::with_exposed_provenance_mut::<PageTable>(virt as usize) }
    }
}

static KERNEL_SPACE: IrqMutex<Option<Mapper<HhdmTables, Vmsa>>> = IrqMutex::new(None);

/// Device mappings (bump-allocated, never freed).
const MMIO_WINDOW: Range<u64> = 0xffff_e000_0000_0000..0xffff_e100_0000_0000;
static MMIO_NEXT: AtomicU64 = AtomicU64::new(MMIO_WINDOW.start);

/// The kernel's TTBR1 root (0 before `init`).
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum PagingError {
    NotInitialized,
    MmioWindowFull,
    Map(MapError),
}

impl From<MapError> for PagingError {
    fn from(e: MapError) -> Self {
        PagingError::Map(e)
    }
}

/// Inputs from the bootloader.
pub struct KernelLayout<'a> {
    pub hhdm_offset: u64,
    pub kernel_phys_base: u64,
    pub kernel_virt_base: u64,
    pub memory_map: &'a mut dyn Iterator<Item = MemoryRegion>,
}

/// Statistics for the boot log.
#[derive(Debug, Default)]
pub struct BuildStats {
    pub hhdm_pages: [u64; 3],
    pub kernel_pages: u64,
}

fn alloc_table() -> Option<Frame> {
    crate::mm::allocate_frame(crate::mm::Use::PageTable)
}

/// HHDM flags for a memory map region (what Limine maps there), or None.
fn hhdm_flags(kind: RegionKind) -> Option<PageFlags> {
    match kind {
        RegionKind::Usable
        | RegionKind::BootloaderReclaimable
        | RegionKind::ExecutableAndModules
        | RegionKind::AcpiReclaimable
        | RegionKind::AcpiNvs
        | RegionKind::ReservedMapped => Some(PageFlags::KERNEL_RW),
        RegionKind::Framebuffer => {
            Some(PageFlags { cache: CacheMode::WriteCombining, ..PageFlags::KERNEL_RW })
        }
        RegionKind::Reserved | RegionKind::BadMemory | RegionKind::Unknown(_) => None,
    }
}

/// Builds the kernel's higher half (the HHDM for every region Limine maps
/// there, the image with per-segment permissions, the device pages mapped
/// so far by `early_map`, at the same addresses) and switches TTBR1 to it.
/// Once, after the frame allocator is up.
pub fn init(layout: KernelLayout<'_>) -> Result<BuildStats, PagingError> {
    let mut stats = BuildStats::default();
    let mut mapper: Mapper<HhdmTables, Vmsa> =
        Mapper::new(HhdmTables { offset: layout.hhdm_offset }, &mut alloc_table)?;
    // Regions are sorted; round outward to pages, never a page twice.
    let mut mapped_until = 0;
    for region in layout.memory_map {
        let Some(flags) = hhdm_flags(region.kind) else { continue };
        let start = (region.base / PAGE_SIZE * PAGE_SIZE).max(mapped_until);
        let end = region.end().next_multiple_of(PAGE_SIZE);
        if start >= end {
            continue;
        }
        let virt = layout.hhdm_offset + start;
        let counts = mapper.map_range(virt, start, end - start, flags, &mut alloc_table)?;
        for (total, n) in stats.hhdm_pages.iter_mut().zip(counts) {
            *total += n;
        }
        mapped_until = end;
    }
    // SAFETY: linker-script symbols; only their addresses are used.
    let segments: [(Range<u64>, PageFlags); 3] = unsafe {
        [
            (sym(&__rodata_start)..sym(&__rodata_end), PageFlags::KERNEL_RO),
            (sym(&__text_start)..sym(&__text_end), PageFlags::KERNEL_RX),
            (sym(&__data_start)..sym(&__data_end), PageFlags::KERNEL_RW),
        ]
    };
    for (range, flags) in segments {
        let phys = range.start - layout.kernel_virt_base + layout.kernel_phys_base;
        let len = range.end - range.start;
        mapper.map_range(range.start, phys, len, flags, &mut alloc_table)?;
        stats.kernel_pages += len / PAGE_SIZE;
    }
    for (virt, phys) in super::early_map::mapped() {
        let device = PageFlags { cache: CacheMode::UncachedMinus, ..PageFlags::MMIO };
        mapper.map(virt, phys, PageSize::Size4K, device, &mut alloc_table)?;
    }
    let root = mapper.root().start();
    // SAFETY: the new tables map everything the kernel uses now at the same
    // addresses with the same attributes (MAIR's slot 0 stays write-back,
    // slot 2 Device-nGnRE): its image, through the HHDM the boot stack,
    // the bootloader's data, the frame bitmap and the tables, and the
    // early device pages. The old entries are invalidated (all of EL1's:
    // the kernel's are global) before any further access.
    unsafe {
        core::arch::asm!(
            "msr mair_el1, {mair}",
            "dsb ishst",
            "msr ttbr1_el1, {root}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            mair = in(reg) MAIR,
            root = in(reg) root,
            options(nostack, preserves_flags)
        )
    };
    KERNEL_ROOT.store(root, Ordering::Release);
    *KERNEL_SPACE.lock() = Some(mapper);
    Ok(stats)
}

fn with_space<R>(f: impl FnOnce(&mut Mapper<HhdmTables, Vmsa>) -> R) -> Option<R> {
    KERNEL_SPACE.lock().as_mut().map(f)
}

pub fn translate(virt: u64) -> Option<Translation> {
    with_space(|m| m.translate(virt)).flatten()
}

/// The higher half's root (TTBR1_EL1's table address).
pub fn active_root() -> u64 {
    let ttbr1: u64;
    // SAFETY: reads TTBR1_EL1.
    unsafe {
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) ttbr1, options(nomem, nostack, preserves_flags))
    };
    ttbr1 & 0x0000_ffff_ffff_f000
}

/// Kernel image bounds `[start, end)`.
pub fn kernel_image() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__kernel_start)..sym(&__kernel_end) }
}

/// The kernel's code `[start, end)`.
pub fn kernel_text() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__text_start)..sym(&__text_end) }
}

/// Maps `len` bytes of device memory at physical `phys` (Device-nGnRnE)
/// and returns the virtual address corresponding to `phys`.
pub fn map_mmio(phys: u64, len: u64) -> Result<u64, PagingError> {
    let start = phys / PAGE_SIZE * PAGE_SIZE;
    let size = (phys + len).next_multiple_of(PAGE_SIZE) - start;
    let virt = MMIO_NEXT.fetch_add(size, Ordering::Relaxed);
    if virt + size > MMIO_WINDOW.end {
        return Err(PagingError::MmioWindowFull);
    }
    with_space(|m| m.map_range(virt, start, size, PageFlags::MMIO, &mut alloc_table))
        .ok_or(PagingError::NotInitialized)??;
    Ok(virt + (phys - start))
}

/// Maps one 4 KiB kernel page. The page must not be mapped already.
pub fn map_kernel_page(virt: u64, frame: Frame, flags: PageFlags) -> Result<(), PagingError> {
    with_space(|m| m.map(virt, frame.start(), PageSize::Size4K, flags, &mut alloc_table))
        .ok_or(PagingError::NotInitialized)??;
    // The entry was invalid: only the store must reach the walker.
    // SAFETY: barriers only.
    unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
    Ok(())
}

/// Unmaps one 4 KiB kernel page and invalidates it on every CPU of the
/// inner shareable domain (`tlbi vaae1is`). Returns the frame it mapped.
pub fn unmap_kernel_page(virt: u64) -> Option<Frame> {
    let frame = with_space(|m| m.unmap_4k(virt)).flatten()?;
    // SAFETY: invalidating TLB entries cannot break memory safety.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) (virt >> 12) & 0x0000_0fff_ffff_ffff,
            options(nostack, preserves_flags)
        )
    };
    Some(frame)
}

/// Frames kept forever because their deferred free could not be recorded
/// (none yet here: no user spaces).
pub fn leaked_frames() -> u64 {
    0
}

/// The kernel's TTBR1 root (0 before `init`).
pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Acquire)
}

/// Loads an address space: on aarch64 the kernel half is TTBR1's, always
/// loaded; a user space's (TTBR0) comes with the port's user mode.
///
/// # Safety
/// `root` is a complete address space's (as x86-64's).
pub unsafe fn activate(root: u64) {
    if root != kernel_root() {
        unported("paging::activate (a user address space)")
    }
}

pub type SharedUserSpace = crate::mm::heap::KArc<SpaceLock>;

/// A user address space and its lock.
pub struct SpaceLock(IrqMutex<UserSpace>);

impl SpaceLock {
    pub fn new(space: UserSpace) -> Self {
        SpaceLock(IrqMutex::new(space))
    }

    pub fn lock(&self) -> crate::sync::IrqGuard<'_, UserSpace> {
        self.0.lock()
    }
}

/// A user address space.
pub struct UserSpace {
    root: u64,
}

fn hhdm_ptr(phys: u64) -> *mut u8 {
    core::ptr::with_exposed_provenance_mut((crate::mm::hhdm_offset() + phys) as usize)
}

/// A kernel-owned page also mapped into user address spaces
/// (KUSER_SHARED_DATA), written through the HHDM with volatile stores
/// since user threads read it concurrently (as x86-64's).
pub struct SharedPage {
    frame: Frame,
}

impl SharedPage {
    pub fn new() -> Result<Self, PagingError> {
        let frame = crate::mm::allocate_frame(crate::mm::Use::SectionPage)
            .ok_or(PagingError::Map(MapError::OutOfMemory))?;
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by us.
        unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
        Ok(Self { frame })
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }

    /// Writes `bytes` at `offset`; out-of-range writes are dropped.
    pub fn write(&self, offset: usize, bytes: &[u8]) {
        if offset.checked_add(bytes.len()).is_none_or(|end| end > PAGE_SIZE as usize) {
            return;
        }
        let base = hhdm_ptr(self.frame.start());
        for (i, &b) in bytes.iter().enumerate() {
            // SAFETY: in bounds of the owned frame; volatile because user
            // mode reads the page concurrently through its own mapping.
            unsafe { base.add(offset + i).write_volatile(b) };
        }
    }

    pub fn write_u32(&self, offset: usize, value: u32) {
        if !offset.is_multiple_of(4) || offset + 4 > PAGE_SIZE as usize {
            return;
        }
        // SAFETY: aligned and in bounds of the owned frame; one volatile
        // store, so readers never see a torn value.
        unsafe { hhdm_ptr(self.frame.start()).add(offset).cast::<u32>().write_volatile(value) };
    }
}

/// A user access that faulted, at `address`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserFault {
    pub address: u64,
}

impl UserSpace {
    pub fn new() -> Result<Self, PagingError> {
        Err(PagingError::NotInitialized)
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn map_zero_page(&mut self, _virt: u64, _flags: PageFlags) -> Result<(), PagingError> {
        unported("UserSpace::map_zero_page")
    }

    pub fn is_mapped(&mut self, _virt: u64) -> bool {
        false
    }

    pub fn next_mapped(&mut self, _virt: u64, _end: u64) -> Option<u64> {
        None
    }

    pub fn set_flags(&mut self, _virt: u64, _flags: PageFlags) -> Result<(), PagingError> {
        unported("UserSpace::set_flags")
    }

    pub fn unmap_page(&mut self, _virt: u64) -> bool {
        false
    }

    pub fn map_zeroed(
        &mut self,
        _virt: u64,
        _pages: u64,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_zeroed")
    }

    pub fn read(&mut self, virt: u64, _buf: &mut [u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn write(&mut self, virt: u64, _data: &[u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn write_initial(&mut self, virt: u64, _data: &[u8]) -> Result<(), UserFault> {
        Err(UserFault { address: virt })
    }

    pub fn map_shared(
        &mut self,
        _virt: u64,
        _page: &SharedPage,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_shared")
    }

    pub fn map_borrowed(
        &mut self,
        _virt: u64,
        _frame: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_borrowed")
    }

    pub fn map_adopted(
        &mut self,
        _virt: u64,
        _frame: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_adopted")
    }

    pub fn for_each_page(&mut self, _f: impl FnMut(u64, Frame, PageFlags)) {}

    /// The page file (`nt::pagefile`) needs a disk: not ported yet. Nothing ages.
    pub fn age_page(&mut self, _virt: u64) -> crate::mm::paging::Age {
        crate::mm::paging::Age::Absent
    }

    pub fn copy_page(&mut self, _virt: u64, _frame: Frame, _buf: &mut [u8]) -> bool {
        false
    }

    pub fn reserve_frees(&mut self, _n: usize) -> bool {
        false
    }

    pub fn page_out(&mut self, _virt: u64, _frame: Frame, _token: u64) -> bool {
        false
    }

    pub fn swapped(&mut self, _virt: u64) -> Option<u64> {
        None
    }

    pub fn page_in(
        &mut self,
        _virt: u64,
        _token: u64,
        _data: &[u8],
        _flags: PageFlags,
    ) -> Result<bool, PagingError> {
        Ok(false)
    }

    pub fn mapping(&mut self, _virt: u64) -> Option<(Frame, bool)> {
        None
    }

    pub fn move_page(&mut self, _from: u64, _to: u64) -> Result<bool, PagingError> {
        unported("UserSpace::move_page")
    }

    pub fn make_writable(&mut self, _virt: u64, _flags: PageFlags) -> Result<bool, PagingError> {
        unported("UserSpace::make_writable")
    }

    pub fn share_into(
        &mut self,
        _child: &mut UserSpace,
        _virt: u64,
    ) -> Result<crate::mm::paging::ForkedPage, PagingError> {
        unported("UserSpace::share_into")
    }

    pub fn map_private_copy(
        &mut self,
        _virt: u64,
        _source: Frame,
        _flags: PageFlags,
    ) -> Result<(), PagingError> {
        unported("UserSpace::map_private_copy")
    }

    pub fn frame_with(_data: &[u8]) -> Option<Frame> {
        None
    }

    pub fn zeroed_frame() -> Option<Frame> {
        None
    }
}
