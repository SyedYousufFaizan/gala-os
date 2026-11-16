//! Windows' ARM64 ABI: arguments in x0-x7, then the stack; a call's
//! argument in x0; ARM64's CONTEXT (`context`), which a process's first
//! thread starts LdrInitializeThunk with, NtContinue resumes and
//! NtCreateThread starts a thread from. Not yet (ROADMAP AR): the
//! exception and APC frames (exceptions are not dispatched to user mode,
//! NtContinue's TestAlert runs no APC), NtGetContextThread and
//! NtSetContextThread, the KUSER_SHARED_DATA processor features; their
//! services answer STATUS_NOT_IMPLEMENTED.

use super::ArgSlot;
use crate::nt::crashreport::Note;
use crate::nt::object::ObjectRef;
use crate::nt::platform::{Gpr, SharedPage, SyscallFrame, TrapFrame, UserRegs};
use crate::nt::process::Process;
use crate::nt::registry::Processor;
use crate::nt::status::NtStatus;
use crate::nt::syscalls::Call;

pub mod context;

use crate::nt::platform::{self, XState, xstate};

// The kernel's FP/SIMD save area is the one CONTEXT's floating-point part
// converts to and from.
const _: () = assert!(xstate::MAX_SIZE == context::FP_STATE_BYTES);

/// The kernel's user registers as CONTEXT's.
fn context_regs(regs: &UserRegs) -> context::Regs {
    let w = regs.to_words();
    let mut x = [0u64; 31];
    x.copy_from_slice(&w[..31]);
    context::Regs { x, sp: w[31], pc: w[32], cpsr: w[33] as u32 }
}

/// CONTEXT's registers as the kernel's user registers.
fn user_regs(regs: &context::Regs) -> UserRegs {
    let mut w = [0u64; UserRegs::WORDS];
    w[..31].copy_from_slice(&regs.x);
    (w[31], w[32], w[33]) = (regs.sp, regs.pc, u64::from(regs.cpsr));
    UserRegs::from_words(&w)
}

/// A CONTEXT's floating-point part as the kernel's FP/SIMD state.
fn xstate_of(bytes: &[u8; context::FP_STATE_BYTES]) -> XState {
    let mut state = XState::initial();
    state.bytes_mut().copy_from_slice(bytes);
    state
}

const REGISTER_ARGS: usize = 8;

/// System call argument `i`: x0-x7, then the user stack.
pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> ArgSlot {
    saved_arg(&frame.return_regs(), i)
}

pub fn saved_arg(regs: &UserRegs, i: usize) -> ArgSlot {
    const X: [Gpr; REGISTER_ARGS] =
        [Gpr::X0, Gpr::X1, Gpr::X2, Gpr::X3, Gpr::X4, Gpr::X5, Gpr::X6, Gpr::X7];
    match X.get(i) {
        Some(&r) => ArgSlot::Value(regs.gpr(r)),
        None => ArgSlot::Stack(regs.sp() + 8 * (i - REGISTER_ARGS) as u64),
    }
}

/// The stub's caller: the link register.
pub fn stub_return_address(_process: &Process, regs: &UserRegs) -> Option<u64> {
    Some(regs.gpr(Gpr::X30))
}

/// The registers that call `entry(arg)` on a stack whose top is
/// `stack_top`: x0 = `arg`, sp 16-aligned, lr 0.
pub fn call_regs(entry: u64, arg: u64, stack_top: u64) -> UserRegs {
    let mut regs = UserRegs::default();
    regs.set_gpr(Gpr::X0, arg);
    regs.set_sp(stack_top & !0xf);
    regs.set_ip(entry);
    regs
}

/// The start through ntdll: LdrInitializeThunk (`thunk`) with x0 = the
/// CONTEXT of `start` (CONTEXT_FULL, the floating-point part the reset
/// state), written at the top of its stack, and x1 = ntdll's base
/// `ntdll`; `start`'s sp is moved below the CONTEXT, and the thunk runs
/// on the stack below that.
pub fn ldr_thunk_regs(
    process: &Process,
    start: &UserRegs,
    stack_top: u64,
    thunk: u64,
    ntdll: u64,
) -> Result<UserRegs, NtStatus> {
    let mut entry = *start;
    let context_addr = (stack_top - context::SIZE as u64) & !0xf;
    entry.set_sp(context_addr);
    let reset = [0u8; context::FP_STATE_BYTES];
    process
        .write(context_addr, &context::to_bytes(&context_regs(&entry), Some(&reset)))
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut regs = call_regs(thunk, context_addr, context_addr);
    regs.set_gpr(Gpr::X1, ntdll);
    Ok(regs)
}

/// What a new thread's CONTEXT says (NtCreateThread): its registers and
/// the start address ThreadQuerySetWin32StartAddress reports (its Pc).
/// Its floating-point part is ignored: a new thread starts with the reset
/// FP/SIMD state, as on x64.
pub fn thread_start(ctx: &[u8]) -> (UserRegs, u64) {
    let regs = user_regs(&context::from_bytes(ctx));
    (regs, regs.ip())
}

pub fn deliver_to_user(
    _process: &Process,
    _frame: &mut TrapFrame,
    _status: NtStatus,
    _cr2: Option<u64>,
) -> bool {
    false
}

pub fn keep_note(
    process: &Process,
    frame: &TrapFrame,
    status: NtStatus,
    ip: u64,
    _cr2: Option<u64>,
) {
    let mut regs = frame.user_regs();
    regs.set_ip(ip);
    *process.last_exception.lock() = Some(Note::new(process, status, None, regs, false));
}

pub fn report_registers<T: core::fmt::Display>(_note: &Note, _stack: &[Option<T>]) {}

/// The page's machine fields (ARM64); its processor features are not
/// written yet.
pub fn write_kuser_processor(page: &SharedPage) {
    use crate::nt::kuser::{IMAGE_NUMBER_HIGH, IMAGE_NUMBER_LOW, NATIVE_PROCESSOR_ARCHITECTURE};
    const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
    const PROCESSOR_ARCHITECTURE_ARM64: u16 = 12;
    page.write(IMAGE_NUMBER_LOW, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(IMAGE_NUMBER_HIGH, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(NATIVE_PROCESSOR_ARCHITECTURE, &PROCESSOR_ARCHITECTURE_ARM64.to_le_bytes());
}

pub fn processor() -> Processor {
    Processor { name: [0; 48], vendor: [0; 12], family: 0, model: 0, stepping: 0, mhz: 0 }
}

pub(in crate::nt) fn nt_get_context_thread(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

pub(in crate::nt) fn nt_set_context_thread(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

/// NtContinue(Context, TestAlert): the thread resumes with the parts of
/// `Context` its flags name over the registers it called with (the
/// floating-point part loaded now: the kernel does not use those
/// registers, so they are what the thread resumes with). x18 stays the
/// TEB whatever the CONTEXT's X18 says (`syscall::for_user`). TestAlert
/// runs no APC yet: ARM64's APC frame is ROADMAP AR's.
pub(in crate::nt) fn nt_continue(call: &mut Call<'_>) -> NtStatus {
    let ctx_ptr = match call.arg(0) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let mut ctx = [0u8; context::SIZE];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let flags = context::flags(&ctx);
    if !context::has(flags, context::CONTEXT_ARM64) {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut regs = context_regs(&call.frame.return_regs());
    context::apply(&mut regs, &ctx, flags);
    let regs = user_regs(&regs);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    if let Some(fp) = context::float_state(&ctx) {
        platform::xrstor(&xstate_of(&fp));
    }
    platform::continue_with(regs);
    NtStatus::SUCCESS
}

pub(in crate::nt) fn gala_unhandled_exception(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

/// No APC frame yet: the wait's status as it is.
pub(in crate::nt) fn deliver_apc_after(_call: &Call<'_>, status: NtStatus) -> NtStatus {
    status
}

/// No WOW64 on aarch64.
pub(in crate::nt) fn create_wow64_thread(
    _call: &Call<'_>,
    _ctx: &[u8],
    _stack: (u64, u64, u64),
    _suspended: bool,
) -> Result<ObjectRef, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}
