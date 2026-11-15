//! The ARM64 parts of msvcrt: a stub that compiles (ROADMAP PT; the port
//! is item AR), with `x64.rs`'s names. The C-variadic entry points,
//! `_setjmp`/`longjmp` and the floating-point control word are ARM64's
//! own (its variadic convention, its _JUMP_BUFFER, FPCR) and come with the
//! port: build.rs exports none of them for ARM64, and `_controlfp`
//! records the gap and ends the process.

/// Where the port has not been written yet: recorded, then the process
/// ends (the panic handler's abort).
#[track_caller]
pub fn unported(what: &[u8]) -> ! {
    super::missing(what, 0);
    panic!("msvcrt on ARM64: not ported yet")
}

/// _controlfp(new, mask): the floating-point control word (ARM64: FPCR,
/// the port's).
#[unsafe(no_mangle)]
pub extern "C" fn _controlfp(_new: u32, _mask: u32) -> u32 {
    unported(b"msvcrt.dll!_controlfp(ARM64)")
}

/// The raised-exception flags (FPSR's on ARM64, the port's): the UCRT's.
#[cfg(ucrt)]
pub fn fp_status(_clear: bool) -> u32 {
    unported(b"msvcrt.dll!floating-point status(ARM64)")
}

/// _fpreset (FPCR and FPSR on ARM64, the port's).
pub fn fp_reset() {
    unported(b"msvcrt.dll!_fpreset(ARM64)")
}
