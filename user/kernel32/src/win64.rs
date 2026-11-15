//! The 64-bit Windows layouts kernel32 reads, shared by x64 and ARM64
//! (one TEB, PEB, RTL_USER_PROCESS_PARAMETERS and PE32+ optional header
//! for both: winnt.h, winternl.h). `x64.rs` and `aarch64.rs` re-export
//! them; the x86 build has its own in `x86.rs`.

/// TEB offsets.
pub const TEB_STACK_BASE: usize = 0x08;
pub const TEB_SELF: usize = 0x30;
pub const TEB_CLIENT_PROCESS: usize = 0x40;
pub const TEB_CLIENT_THREAD: usize = 0x48;
pub const TEB_PEB: usize = 0x60;
pub const TEB_LAST_ERROR: usize = 0x68;
pub const TEB_DEALLOCATION_STACK: usize = 0x1478;
pub const TEB_TLS_SLOTS: usize = 0x1480;
pub const TEB_HARD_ERROR_MODE: usize = 0x16b0;
pub const TEB_GUARANTEED_STACK_BYTES: usize = 0x1748;

/// PEB offsets.
pub const PEB_BEING_DEBUGGED: usize = 0x02;
pub const PEB_IMAGE_BASE: usize = 0x10;
pub const PEB_PROCESS_PARAMETERS: usize = 0x20;
pub const PEB_PROCESS_HEAP: usize = 0x30;
pub const PEB_SESSION_ID: usize = 0x2c0;

/// RTL_USER_PROCESS_PARAMETERS offsets: the standard handles
/// (input, output, error: pointer-sized, in that order), CurrentDirectory
/// (DosPath, a UNICODE_STRING, then Handle), ImagePathName, CommandLine
/// and RuntimeData (UNICODE_STRINGs: Length, MaximumLength, Buffer),
/// Environment, EnvironmentSize.
pub const PARAMS_STANDARD_INPUT: usize = 0x20;
pub const PARAMS_CURRENT_DIRECTORY: usize = 0x38;
pub const PARAMS_CURRENT_DIRECTORY_HANDLE: usize = 0x48;
pub const PARAMS_IMAGE_PATH: usize = 0x60;
pub const PARAMS_COMMAND_LINE: usize = 0x70;
pub const PARAMS_ENVIRONMENT: usize = 0x80;
pub const PARAMS_RUNTIME_DATA: usize = 0xe0;
pub const PARAMS_ENVIRONMENT_SIZE: usize = 0x3f0;

/// The optional header's SizeOfStackReserve and SizeOfStackCommit (PE32+:
/// 8 bytes each), from the optional header's start.
pub const OPTIONAL_STACK_RESERVE: usize = 72;
pub const OPTIONAL_STACK_COMMIT: usize = 80;
