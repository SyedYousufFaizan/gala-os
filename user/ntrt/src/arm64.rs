//! The ARM64 runtime: what ARM64 test programs use (ROADMAP AR's user
//! mode; it grows with the programs built for ARM64, as x86's did).
//!
//! System call stubs are shaped like Windows ARM64's ntdll: the service
//! number in the SVC's immediate (`svc #N`), the arguments in x0-x7 and
//! on the stack, the result in x0. The numbers are the pinned build's,
//! the same as x64's (`kernel/src/nt/syscalls.rs`). The TEB is x18's
//! (Microsoft's ARM64 ABI), the PEB at TEB+0x60 as on x64.

use core::arch::{asm, global_asm};
use core::ffi::c_void;

/// A handle value. Opaque to user mode, so an integer rather than a pointer.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub usize);
pub type NtStatus = u32;

pub const STATUS_SUCCESS: NtStatus = 0;

/// `NtCurrentProcess()`.
pub const CURRENT_PROCESS: Handle = Handle(usize::MAX);

/// System call stubs: `svc #N; ret`.
macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        global_asm!(
            $(
                concat!(".globl ", stringify!($name)),
                ".p2align 2",
                concat!(stringify!($name), ":"),
                concat!("svc #", stringify!($nr)),
                "ret",
            )*
        );
    };
}

stubs! {
    ntrt_nt_write_file = 0x0008;
    ntrt_nt_terminate_process = 0x002c;
}

unsafe extern "C" {
    fn ntrt_nt_write_file(
        file: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buffer: *const u8,
        length: u32,
        byte_offset: *const i64,
        key: *const u32,
    ) -> NtStatus;
    fn ntrt_nt_terminate_process(process: Handle, status: NtStatus) -> NtStatus;
}

#[repr(C)]
pub struct IoStatusBlock {
    pub status: usize,
    pub information: usize,
}

/// The current thread's TEB (x18).
pub fn teb() -> *const u8 {
    let teb: *const u8;
    // SAFETY: on ARM64 Windows, x18 holds the current TEB.
    unsafe { asm!("mov {}, x18", out(reg) teb, options(nomem, nostack, preserves_flags)) };
    teb
}

/// PEB of the current process (TEB+0x60).
pub fn peb() -> *const u8 {
    // SAFETY: the TEB is mapped for the thread's life; +0x60 is its PEB
    // pointer.
    unsafe { *(teb().add(0x60) as *const *const u8) }
}

/// `PEB->ProcessParameters->StandardOutput` (the 64-bit layout's offsets).
pub fn stdout() -> Handle {
    // SAFETY: the loader always provides ProcessParameters.
    unsafe {
        let params = *(peb().add(0x20) as *const *const u8);
        *(params.add(0x28) as *const Handle)
    }
}

pub fn write_file(handle: Handle, data: &[u8]) -> NtStatus {
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: valid buffer and IO_STATUS_BLOCK, optional arguments null.
    unsafe {
        ntrt_nt_write_file(
            handle,
            Handle(0),
            null,
            null,
            &mut iosb,
            data.as_ptr(),
            data.len() as u32,
            core::ptr::null(),
            core::ptr::null(),
        )
    }
}

pub fn exit(status: NtStatus) -> ! {
    // SAFETY: terminating the current process does not return.
    unsafe { ntrt_nt_terminate_process(CURRENT_PROCESS, status) };
    loop {
        core::hint::spin_loop();
    }
}

#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn mainCRTStartup() -> ! {
            $crate::exit($main())
        }

        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            $crate::exit(0xc000_0409)
        }
    };
}

// Memory intrinsics the compiler may call (no CRT): byte loops in
// assembly, so they cannot be compiled back into calls to themselves.
global_asm!(
    ".globl memcpy",
    ".p2align 2",
    "memcpy:",
    "mov x3, x0",
    "cbz x2, 2f",
    "1:",
    "ldrb w4, [x1], #1",
    "strb w4, [x3], #1",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
    ".globl memmove",
    ".p2align 2",
    "memmove:",
    "cmp x0, x1",
    "b.ls memcpy",
    "add x3, x0, x2",
    "add x1, x1, x2",
    "cbz x2, 2f",
    "1:",
    "ldrb w4, [x1, #-1]!",
    "strb w4, [x3, #-1]!",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
    ".globl memset",
    ".p2align 2",
    "memset:",
    "mov x3, x0",
    "cbz x2, 2f",
    "1:",
    "strb w1, [x3], #1",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
    ".globl memcmp",
    ".p2align 2",
    "memcmp:",
    "mov x3, #0",
    "cbz x2, 2f",
    "1:",
    "ldrb w4, [x0], #1",
    "ldrb w5, [x1], #1",
    "subs w3, w4, w5",
    "b.ne 2f",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "mov w0, w3",
    "ret",
);
