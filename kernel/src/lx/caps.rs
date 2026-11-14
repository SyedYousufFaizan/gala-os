//! Capabilities for the Linux personality: capget(2) and capset(2)
//! (ROADMAP LD: Ubuntu 26.04's glycin reads them before it sandboxes an
//! image loader). user/lx/caps_test is the spec, recorded on Linux.
//!
//! gala keeps no capability sets of a process's own: they follow its uid,
//! as Linux's do for a program without file capabilities, uid 0 holding
//! every capability up to CAP_LAST_CAP (40) and any other uid none. The
//! version handshake is Linux's: v1 copies one structure, v2 and v3 two;
//! an unknown version is answered with v3 written back and EINVAL (0
//! when no data was asked for). capset accepts the sets the process has,
//! refuses an effective capability not permitted, more than it has, and
//! another process (EPERM); a root process dropping capabilities is not
//! kept (gala has no reduced sets): accepted and ledgered (`linux option
//! missing`), as the limits setrlimit lowers are.

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL, EPERM, ESRCH};
use crate::nt::ledger;

pub const V1: u32 = 0x1998_0330;
pub const V2: u32 = 0x2007_1026;
pub const V3: u32 = 0x2008_0522;
/// CAP_LAST_CAP (Linux 6.x: CAP_CHECKPOINT_RESTORE).
pub const LAST_CAP: u32 = 40;
/// Every capability, as v3's two 32-bit words make one mask.
pub const ALL: u64 = (1 << (LAST_CAP + 1)) - 1;

/// The structures a version copies (1 or 2), or None for an unknown one.
pub fn words(version: u32) -> Option<usize> {
    match version {
        V1 => Some(1),
        V2 | V3 => Some(2),
        _ => None,
    }
}

/// The sets (effective, permitted, inheritable) of a process of `uid`.
pub fn sets_of(uid: u32) -> (u64, u64, u64) {
    if uid == 0 { (ALL, ALL, 0) } else { (0, 0, 0) }
}

/// The sets laid out as `n` struct __user_cap_data_struct (low words
/// first).
pub fn layout((e, p, i): (u64, u64, u64), n: usize) -> [u8; 24] {
    let mut b = [0u8; 24];
    for w in 0..n.min(2) {
        let shift = 32 * w;
        for (k, v) in [e, p, i].iter().enumerate() {
            let at = 12 * w + 4 * k;
            b[at..at + 4].copy_from_slice(&((v >> shift) as u32).to_le_bytes());
        }
    }
    b
}

/// Reads the header: its version's structure count, or the errno after
/// writing v3 back for an unknown version (EINVAL; `None` asked: Ok(None)).
fn header(call: &Call<'_>, hdr: u64) -> core::result::Result<(Option<usize>, i32), u16> {
    let mut h = [0u8; 8];
    call.process.read(hdr, &mut h).map_err(|_| EFAULT)?;
    let version = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let pid = i32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    let n = words(version);
    if n.is_none() {
        call.process.write(hdr, &V3.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    Ok((n, pid))
}

/// The uid of the process `pid` names (0: the caller; a Linux pid or tid).
fn uid_of(call: &Call<'_>, pid: i32) -> core::result::Result<u32, u16> {
    if pid < 0 {
        return Err(EINVAL);
    }
    if pid == 0 || pid as u64 == call.process.id {
        return Ok(super::identity::of_process(&call.process).uid);
    }
    let (lx, _) = super::state::find_thread(pid as u64).ok_or(ESRCH)?;
    let process = crate::nt::process::running()
        .ok()
        .and_then(|list| list.into_iter().find(|p| p.id == lx.pid))
        .ok_or(ESRCH)?;
    Ok(super::identity::of_process(&process).uid)
}

/// capget(hdr, data).
pub fn capget(call: &Call<'_>, hdr: u64, data: u64) -> Result {
    let (n, pid) = header(call, hdr)?;
    let Some(n) = n else {
        return if data == 0 { Ok(0) } else { Err(EINVAL) };
    };
    if data == 0 {
        return Ok(0);
    }
    let uid = uid_of(call, pid)?;
    call.process.write(data, &layout(sets_of(uid), n)[..12 * n]).map_err(|_| EFAULT)?;
    Ok(0)
}

/// capset(hdr, data): only the caller's own sets.
pub fn capset(call: &Call<'_>, hdr: u64, data: u64) -> Result {
    let (n, pid) = header(call, hdr)?;
    let n = n.ok_or(EINVAL)?;
    if pid != 0 && pid as u64 != call.process.id && pid as u64 != call.tid {
        return Err(EPERM);
    }
    let mut b = [0u8; 24];
    call.process.read(data, &mut b[..12 * n]).map_err(|_| EFAULT)?;
    let word = |w: usize, k: usize| {
        let at = 12 * w + 4 * k;
        u64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])) << (32 * w)
    };
    let set = |k: usize| (0..n).map(|w| word(w, k)).fold(0, |a, v| a | v);
    let (e, p, i) = (set(0), set(1), set(2));
    let (_, has, _) = sets_of(super::identity::of_process(&call.process).uid);
    if e & !p != 0 || p & !has != 0 || i & !has != 0 {
        return Err(EPERM);
    }
    if (e, p, i) != sets_of(super::identity::of_process(&call.process).uid) {
        // Fewer than it has: not kept.
        ledger::record("capset", "linux option missing", p);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_has_every_capability_and_a_user_none() {
        assert_eq!(sets_of(0), (ALL, ALL, 0));
        assert_eq!(sets_of(1000), (0, 0, 0));
        let b = layout(sets_of(0), 2);
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), u32::MAX);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 0x1ff);
        assert_eq!(&layout(sets_of(0), 1)[12..], &[0; 12]);
    }

    #[test]
    fn versions_copy_one_or_two_structures() {
        assert_eq!(words(V1), Some(1));
        assert_eq!(words(V2), Some(2));
        assert_eq!(words(V3), Some(2));
        assert_eq!(words(0), None);
    }
}
