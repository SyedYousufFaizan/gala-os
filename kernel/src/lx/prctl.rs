//! prctl(2) and the process's ids beyond getuid (ROADMAP LD step 3: GLib,
//! libcap and libselinux call them when a GTK program starts;
//! user/lx/prctl_test, recorded on Linux, is the spec).
//!
//! - PR_SET_NAME and PR_GET_NAME: the calling thread's name
//!   (`state::Comm`, Linux's "comm"): set from up to 15 bytes of a string
//!   (cut, as Linux cuts it), read back as 16 bytes ending in a NUL;
//!   EFAULT for a bad pointer. A new thread starts with its creator's
//!   name, a program with its file name (execve).
//! - PR_CAPBSET_READ: the capability bounding set, full as for any
//!   process Linux starts without dropping capabilities: 1 for each
//!   capability up to `CAP_LAST_CAP` (40, CAP_CHECKPOINT_RESTORE in the
//!   Linux release gala's ABI follows), EINVAL beyond (libcap finds the
//!   last capability by that EINVAL). The process holds none of them
//!   (it is the one unprivileged user), which is what its capability
//!   sets, not the bounding set, say.
//! - PR_SET_NO_NEW_PRIVS (only 1, the other arguments 0) and
//!   PR_GET_NO_NEW_PRIVS: the process's flag (`LxProcess::no_new_privs`),
//!   kept by its children and across execve, never cleared; gala has no
//!   set-user-id programs for it to hold back (bubblewrap sets it first).
//! - PR_SET_PDEATHSIG and PR_GET_PDEATHSIG: the signal (up to 64) the
//!   process gets when its parent process ends (`state::ended`; Linux's
//!   is the parent thread's end: the same for a single-threaded parent),
//!   none in a fork's child, kept across execve.
//! - PR_GET_AUXV: the image's auxiliary vector as its initial stack has it
//!   (`LxProcess::auxv`) in Linux's saved_auxv, 448 bytes with zeros
//!   after AT_NULL: as much as fits copied, 448 returned (rustix reads it
//!   so: uutils' coreutils; /proc/self/auxv has the pairs only).
//! - Any other option is EINVAL, as Linux answers an unknown one, and is
//!   counted in the ledger (`"what":"linux option missing"`, service
//!   `prctl`, detail the option) so the next ones are chosen from data.
//! - getresuid and getresgid: real, effective and saved ids, all the one
//!   user's (`lx::identity`); EFAULT for a bad pointer.

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL};
use super::state::Comm;
use crate::nt::ledger;

const PR_SET_PDEATHSIG: u64 = 1;
const PR_GET_PDEATHSIG: u64 = 2;
const PR_SET_NAME: u64 = 15;
const PR_GET_NAME: u64 = 16;
const PR_CAPBSET_READ: u64 = 23;
const PR_SET_NO_NEW_PRIVS: u64 = 38;
const PR_GET_SECCOMP: u64 = 21;
const PR_SET_SECCOMP: u64 = 22;
const PR_GET_NO_NEW_PRIVS: u64 = 39;
const PR_GET_AUXV: u64 = 0x4155_5856;
/// The size of Linux's saved auxiliary vector (AT_VECTOR_SIZE u64 words
/// on x86-64, Linux 6.18), which PR_GET_AUXV reports.
const SAVED_AUXV: usize = 448;
/// The last capability (capability.h's CAP_LAST_CAP: CAP_CHECKPOINT_RESTORE).
const CAP_LAST_CAP: u64 = 40;

pub fn prctl(call: &Call<'_>, option: u64, args: [u64; 4]) -> Result {
    let [arg2, arg3, arg4, arg5] = args;
    use core::sync::atomic::Ordering::Relaxed;
    match option {
        // A signal for the parent's end (`state::ended` sends it).
        PR_SET_PDEATHSIG => {
            if arg2 > u64::from(lx_abi::signal::NSIG) {
                return Err(EINVAL);
            }
            call.lx.pdeathsig.store(arg2 as u32, Relaxed);
            Ok(0)
        }
        PR_GET_PDEATHSIG => {
            let sig = call.lx.pdeathsig.load(Relaxed) as i32;
            call.process.write(arg2, &sig.to_le_bytes()).map_err(|_| EFAULT)?;
            Ok(0)
        }
        // Once set, kept (no way back), by children too.
        PR_GET_SECCOMP => super::seccomp::get_mode(call),
        PR_SET_SECCOMP => super::seccomp::set_mode(call, arg2, arg3),
        PR_SET_NO_NEW_PRIVS => {
            if arg2 != 1 || arg3 | arg4 | arg5 != 0 {
                return Err(EINVAL);
            }
            call.lx.no_new_privs.store(true, Relaxed);
            Ok(0)
        }
        PR_GET_NO_NEW_PRIVS => {
            if arg2 | arg3 | arg4 | arg5 != 0 {
                return Err(EINVAL);
            }
            Ok(u64::from(call.lx.no_new_privs.load(Relaxed)))
        }
        // The auxiliary vector's size; as much of it as fits copied.
        PR_GET_AUXV => {
            if arg4 | arg5 != 0 {
                return Err(EINVAL);
            }
            // Linux's saved_auxv: AT_VECTOR_SIZE words (Linux 6.18: 448
            // bytes), the pairs and zeros after AT_NULL's.
            let mut saved = [0u8; SAVED_AUXV];
            let auxv = call.lx.auxv.lock();
            let k = auxv.len().min(SAVED_AUXV);
            saved[..k].copy_from_slice(&auxv[..k]);
            drop(auxv);
            let n = (arg3 as usize).min(SAVED_AUXV);
            if n > 0 {
                call.process.write(arg2, &saved[..n]).map_err(|_| EFAULT)?;
            }
            Ok(SAVED_AUXV as u64)
        }
        PR_SET_NAME => {
            // Up to 15 bytes or a NUL, a byte at a time: the string may end
            // just before an unmapped page.
            let mut bytes = [0u8; 15];
            let mut n = 0;
            while n < bytes.len() {
                let mut b = [0u8; 1];
                call.process.read(arg2 + n as u64, &mut b).map_err(|_| EFAULT)?;
                if b[0] == 0 {
                    break;
                }
                bytes[n] = b[0];
                n += 1;
            }
            call.lx.set_comm(call.tid, Comm::of_bytes(&bytes[..n]));
            Ok(0)
        }
        PR_GET_NAME => {
            let comm = call.lx.comm(call.tid);
            call.process.write(arg2, &comm.0).map_err(|_| EFAULT)?;
            Ok(0)
        }
        PR_CAPBSET_READ => {
            if arg2 > CAP_LAST_CAP {
                return Err(EINVAL);
            }
            Ok(1)
        }
        _ => {
            ledger::record("prctl", "linux option missing", option);
            Err(EINVAL)
        }
    }
}

/// getresuid(ruid, euid, suid) and getresgid: each id an unsigned int
/// (real, effective and saved are the same: `lx::identity`).
pub fn getres(call: &Call<'_>, ptrs: [u64; 3], gid: bool) -> Result {
    let ids = super::identity::of_process(&call.process);
    let id = if gid { ids.gid } else { ids.uid };
    for p in ptrs {
        call.process.write(p, &id.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// getgroups(size, list): the supplementary groups (`lx::identity`); a
/// size of 0 asks only how many, a smaller one than that is EINVAL.
pub fn getgroups(call: &Call<'_>, size: u64, list: u64) -> Result {
    let ids = super::identity::of_process(&call.process);
    let groups = ids.groups();
    let size = size as u32 as i32;
    if size < 0 {
        return Err(EINVAL);
    }
    if size == 0 {
        return Ok(groups.len() as u64);
    }
    if (size as usize) < groups.len() {
        return Err(EINVAL);
    }
    let mut bytes = [0u8; 4 * super::identity::MAX_GROUPS];
    for (i, g) in groups.iter().enumerate() {
        bytes[4 * i..4 * i + 4].copy_from_slice(&g.to_le_bytes());
    }
    call.process.write(list, &bytes[..4 * groups.len()]).map_err(|_| EFAULT)?;
    Ok(groups.len() as u64)
}
