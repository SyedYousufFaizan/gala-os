//! timerfd_create(2), timerfd_settime(2) and timerfd_gettime(2) for the
//! Linux personality (ROADMAP LD: Ubuntu 26.04's glycin waits with one in
//! epoll, through Rust's async-io). user/lx/timerfd_test, recorded on
//! Linux, is the spec.
//!
//! A timerfd (`Object::TimerFd`) holds its next expiry on its clock (the
//! real-time one, or the monotonic one for CLOCK_MONOTONIC and
//! CLOCK_BOOTTIME: gala's monotonic clock counts suspend too), its interval
//! and the expirations not read yet. Expiries are counted from the clock
//! whenever it is looked at (`Setting::settle`), so none is lost and none
//! comes early; a read gives them (8 bytes, else EINVAL) and zeroes them,
//! or blocks until the next one (EAGAIN with O_NONBLOCK; a disarmed timer
//! blocks until a settime). Poll, select and epoll see it readable while
//! expiries wait (`readable`), waiting on its `Waiters`; a kernel thread
//! (`timerfd`) sleeps until the earliest armed expiry and wakes them then,
//! as nothing else would. settime zeroes the expirations, as Linux's does.
//!
//! The alarm clocks need CAP_WAKE_ALARM (EPERM for a user, as on Linux;
//! uid 0 has it and they behave as their base clocks). TFD_TIMER_CANCEL_ON_SET
//! is taken and not acted on (gala's clock is not set while programs run):
//! ledgered `linux option missing`. timerfd has no write (EINVAL).
//!
//! Observability: each timerfd is a `timerfd (clock C, next N ns, interval
//! I ns, M expirations)` line in the descriptor table's state dump.

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EBADF, EFAULT, EINVAL, ENOMEM, EPERM};
use super::state::{Fd, Object};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::{ledger, platform};
use crate::sync::IrqMutex;
use crate::unix::Waiters;

const CLOCK_REALTIME: u64 = 0;
const CLOCK_MONOTONIC: u64 = 1;
const CLOCK_BOOTTIME: u64 = 7;
const CLOCK_REALTIME_ALARM: u64 = 8;
const CLOCK_BOOTTIME_ALARM: u64 = 9;
const TFD_NONBLOCK: u64 = 0o4000;
const TFD_CLOEXEC: u64 = 0o2000000;
pub const TFD_TIMER_ABSTIME: u64 = 1;
pub const TFD_TIMER_CANCEL_ON_SET: u64 = 2;
const NS: u64 = 1_000_000_000;

/// The expiry state, in nanoseconds of the timer's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Setting {
    /// The next expiry (None: disarmed).
    pub next: Option<u64>,
    /// 0: one-shot.
    pub interval: u64,
    /// Expirations not read yet.
    pub ticks: u64,
    /// Changes so far (epoll's edge-triggered reports).
    pub changes: u64,
}

impl Setting {
    /// Counts the expiries up to `now`: a one-shot timer disarms, an
    /// interval timer moves on past `now`.
    pub fn settle(&mut self, now: u64) {
        let Some(next) = self.next else { return };
        if now < next {
            return;
        }
        match (now - next).checked_div(self.interval) {
            // One-shot: disarmed.
            None => {
                self.ticks = self.ticks.saturating_add(1);
                self.next = None;
            }
            Some(periods) => {
                let k = periods + 1;
                self.ticks = self.ticks.saturating_add(k);
                self.next = Some(next.saturating_add(k.saturating_mul(self.interval)));
            }
        }
        self.changes = self.changes.wrapping_add(1);
    }

    /// Arms it (`value` 0: disarms) at `now`, returning the setting it had
    /// as (time left, interval).
    pub fn arm(&mut self, now: u64, value: u64, interval: u64, absolute: bool) -> (u64, u64) {
        self.settle(now);
        let old = (self.next.map_or(0, |n| n.saturating_sub(now)), self.interval);
        self.ticks = 0;
        self.interval = interval;
        self.next = match (value, absolute) {
            (0, _) => None,
            (v, true) => Some(v),
            (v, false) => Some(now.saturating_add(v)),
        };
        self.changes = self.changes.wrapping_add(1);
        old
    }
}

pub struct TimerFd {
    /// The real-time clock's (true) or the monotonic one's.
    realtime: bool,
    clock: u64,
    state: IrqMutex<Setting>,
    pub waiters: KArc<Waiters>,
}

impl TimerFd {
    fn now(&self) -> u64 {
        if self.realtime { super::time::realtime_ns() } else { platform::monotonic_ns() }
    }

    /// The expiries counted up to now.
    fn settled(&self) -> Setting {
        let mut s = self.state.lock();
        s.settle(self.now());
        *s
    }

    /// Whether a read would not block.
    pub fn readable(&self) -> bool {
        self.settled().ticks > 0
    }

    pub fn changes(&self) -> u64 {
        self.settled().changes
    }

    /// The monotonic time of its next expiry (a real-time one converted
    /// at the clocks' offset now), if armed.
    fn monotonic_next(&self) -> Option<u64> {
        let next = self.state.lock().next?;
        let now = self.now();
        Some(platform::monotonic_ns().saturating_add(next.saturating_sub(now)))
    }

    /// The state dump's description: (clock, next, interval, expirations).
    pub fn describe(&self) -> (u64, Option<u64>, u64, u64) {
        let s = self.settled();
        (self.clock, s.next, s.interval, s.ticks)
    }
}

/// The armed timerfds the worker wakes (closed ones go: only the list
/// holds them).
static ARMED: IrqMutex<Vec<KArc<TimerFd>>> = IrqMutex::new(Vec::new());
static WORKER: IrqMutex<Option<platform::ThreadId>> = IrqMutex::new(None);
/// Set by `watch` (a timer armed) before it wakes the worker, cleared by
/// the worker before it looks at the list and tested again after its
/// prepare_wait: a timer armed while it was looking is not slept past.
static KICKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn worker(_: usize) {
    use core::sync::atomic::Ordering::SeqCst;
    loop {
        KICKED.store(false, SeqCst);
        let list: Vec<KArc<TimerFd>> = {
            let mut armed = ARMED.lock();
            armed.retain(|t| KArc::strong_count(t) > 1 && t.state.lock().next.is_some());
            let mut copy = Vec::new();
            if copy.try_reserve(armed.len()).is_ok() {
                for t in armed.iter() {
                    let _ = copy.try_push(t.clone());
                }
            }
            copy
        };
        let mut earliest: Option<u64> = None;
        for t in &list {
            let fired = {
                let mut s = t.state.lock();
                let before = s.ticks;
                s.settle(t.now());
                s.ticks != before
            };
            if fired {
                t.waiters.wake_all();
            }
            if let Some(at) = t.monotonic_next() {
                earliest = Some(earliest.map_or(at, |e| e.min(at)));
            }
        }
        drop(list);
        // A real-time timer's expiry is looked at again at least every
        // second (the clock may be moved).
        let at = earliest.map(|e| e.min(platform::monotonic_ns().saturating_add(NS)));
        platform::prepare_wait(at.map(super::wait::ticks_at));
        if KICKED.load(SeqCst) {
            // Armed while it looked: look again (the wait is abandoned).
            platform::wake(platform::current());
        }
        platform::wait();
    }
}

/// Lists `t` for the worker (started at the first) and wakes it to look.
fn watch(t: &KArc<TimerFd>) -> core::result::Result<(), u16> {
    {
        let mut armed = ARMED.lock();
        if !armed.iter().any(|a| KArc::ptr_eq(a, t)) {
            armed.try_push(t.clone()).map_err(|_| ENOMEM)?;
        }
    }
    KICKED.store(true, core::sync::atomic::Ordering::SeqCst);
    let mut w = WORKER.lock();
    match *w {
        Some(tid) => platform::wake(tid),
        None => *w = Some(platform::spawn("timerfd", worker, 0).map_err(|_| ENOMEM)?),
    }
    Ok(())
}

/// timerfd_create(clockid, flags).
pub fn timerfd_create(call: &Call<'_>, clock: u64, flags: u64) -> Result {
    let clock = u64::from(clock as u32);
    let realtime = match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_ALARM => true,
        CLOCK_MONOTONIC | CLOCK_BOOTTIME | CLOCK_BOOTTIME_ALARM => false,
        _ => return Err(EINVAL),
    };
    if flags & !(TFD_NONBLOCK | TFD_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let alarm = matches!(clock, CLOCK_REALTIME_ALARM | CLOCK_BOOTTIME_ALARM);
    if alarm && super::identity::of_process(&call.process).uid != 0 {
        return Err(EPERM);
    }
    let waiters = KArc::try_new(Waiters::default()).map_err(|_| ENOMEM)?;
    let t = KArc::try_new(TimerFd {
        realtime,
        clock,
        state: IrqMutex::new(Setting::default()),
        waiters,
    })
    .map_err(|_| ENOMEM)?;
    let f = KArc::try_new(Fd::new(Object::TimerFd(t), flags & TFD_NONBLOCK != 0))
        .map_err(|_| ENOMEM)?;
    call.lx.install_from(f, flags & TFD_CLOEXEC != 0, 0)
}

fn timer_of(call: &Call<'_>, fd: u64) -> core::result::Result<KArc<TimerFd>, u16> {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    match &f.object {
        Object::TimerFd(t) => Ok(t.clone()),
        _ => Err(EINVAL),
    }
}

fn itimerspec(interval: u64, value: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[..16].copy_from_slice(&super::time::timespec(interval));
    b[16..].copy_from_slice(&super::time::timespec(value));
    b
}

/// timerfd_settime(fd, flags, new, old).
pub fn timerfd_settime(call: &Call<'_>, fd: u64, flags: u64, new: u64, old: u64) -> Result {
    let t = timer_of(call, fd)?;
    if flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET) != 0 {
        return Err(EINVAL);
    }
    let interval = super::time::read_timespec(call, new)?;
    let value = super::time::read_timespec(call, new + 16)?;
    if flags & TFD_TIMER_CANCEL_ON_SET != 0 {
        ledger::record("timerfd_settime", "linux option missing", TFD_TIMER_CANCEL_ON_SET);
    }
    let ns = |s: lx_abi::layout::Timespec| {
        (s.sec as u64).saturating_mul(NS).saturating_add(s.nsec as u64)
    };
    let (left, was) = {
        let mut s = t.state.lock();
        s.arm(t.now(), ns(value), ns(interval), flags & TFD_TIMER_ABSTIME != 0)
    };
    if t.state.lock().next.is_some() {
        watch(&t)?;
    }
    t.waiters.wake_all();
    if old != 0 {
        call.process.write(old, &itimerspec(was, left)).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

/// timerfd_gettime(fd, cur).
pub fn timerfd_gettime(call: &Call<'_>, fd: u64, cur: u64) -> Result {
    let t = timer_of(call, fd)?;
    let s = t.settled();
    let left = s.next.map_or(0, |n| n.saturating_sub(t.now()));
    call.process.write(cur, &itimerspec(s.interval, left)).map_err(|_| EFAULT)?;
    Ok(0)
}

/// read(2) of a timerfd: the expirations since the last read.
pub fn read(call: &Call<'_>, f: &Fd, t: &TimerFd, buf: u64, count: u64) -> Result {
    if count < 8 {
        return Err(EINVAL);
    }
    loop {
        let taken = {
            let mut s = t.state.lock();
            s.settle(t.now());
            core::mem::take(&mut s.ticks)
        };
        if taken > 0 {
            if call.process.write(buf, &taken.to_le_bytes()).is_err() {
                let mut s = t.state.lock();
                s.ticks = s.ticks.saturating_add(taken);
                return Err(EFAULT);
            }
            return Ok(8);
        }
        if f.nonblock() {
            return Err(EAGAIN);
        }
        let me = platform::current();
        t.waiters.add(me).map_err(|_| ENOMEM)?;
        super::wait::prepare(call, t.monotonic_next());
        if t.readable() {
            t.waiters.remove(me);
            super::wait::abandon();
            continue;
        }
        let r = super::wait::block(call);
        t.waiters.remove(me);
        r?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_shot_timer_expires_once_and_never_early() {
        let mut s = Setting::default();
        s.arm(1000, 50, 0, false);
        s.settle(1049);
        assert_eq!((s.ticks, s.next), (0, Some(1050)));
        s.settle(5000);
        assert_eq!((s.ticks, s.next), (1, None));
        s.settle(9000);
        assert_eq!(s.ticks, 1);
    }

    #[test]
    fn an_interval_timer_counts_every_period_passed() {
        let mut s = Setting::default();
        s.arm(0, 10, 10, false);
        s.settle(35);
        // Expiries at 10, 20, 30; the next at 40.
        assert_eq!((s.ticks, s.next), (3, Some(40)));
    }

    #[test]
    fn settime_reports_the_old_setting_and_zeroes_the_expirations() {
        let mut s = Setting::default();
        s.arm(0, 100, 20, false);
        s.settle(130);
        assert_eq!(s.ticks, 2);
        let old = s.arm(130, 0, 0, false);
        assert_eq!(old, (10, 20));
        assert_eq!((s.ticks, s.next), (0, None));
        // Absolute and already past: due at once.
        s.arm(500, 400, 0, true);
        s.settle(500);
        assert_eq!(s.ticks, 1);
    }
}
