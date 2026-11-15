//! ARM64's `guard` (a stub that compiles, ROADMAP PT): catching an
//! exception needs ARM64's unwind information and calling a server
//! routine its argument registers, both the port's (item AR). Each records
//! the gap and ends the call's thread here (the panic handler).

use core::ffi::c_void;

/// `f(arg)`: 0, or the code of the exception that ended it.
pub fn guarded(_f: unsafe extern "system" fn(*mut c_void), _arg: *mut c_void) -> u32 {
    crate::sys::unsupported(b"rpcrt4.dll!guarded call(ARM64)", 0);
    panic!("rpcrt4 on ARM64: not ported yet")
}

/// Calls server routine `f` with `args`.
///
/// # Safety
/// `f` takes `args.len()` pointer-sized arguments.
pub unsafe fn call_routine(_f: *const c_void, args: &[u64]) -> u64 {
    crate::sys::unsupported(b"rpcrt4.dll!server routine call(ARM64)", args.len() as u64);
    panic!("rpcrt4 on ARM64: not ported yet")
}
