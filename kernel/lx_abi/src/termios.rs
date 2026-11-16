//! Terminals as a Linux x86_64 program sees them: the kernel's `struct
//! termios` (TCGETS/TCSETS; 36 bytes, not libc's larger one), its flag
//! bits, the `c_cc` indexes, what a fresh pty starts with, cfmakeraw,
//! `struct winsize`, and the tty ioctl numbers.
//!
//! Sources: termios(3), ioctl_tty(2), pty(7), musl 1.2.4's
//! `<bits/termios.h>` and `<bits/ioctl.h>` (read as data); the defaults
//! are what user/lx/pty_test read from a fresh pty on Linux 6.18, and the
//! tests check them against that transcript.

use core::mem::size_of;

// c_iflag
pub const IGNBRK: u32 = 0o1;
pub const BRKINT: u32 = 0o2;
pub const IGNPAR: u32 = 0o4;
pub const PARMRK: u32 = 0o10;
pub const INPCK: u32 = 0o20;
pub const ISTRIP: u32 = 0o40;
pub const INLCR: u32 = 0o100;
pub const IGNCR: u32 = 0o200;
pub const ICRNL: u32 = 0o400;
pub const IUCLC: u32 = 0o1000;
pub const IXON: u32 = 0o2000;
pub const IXANY: u32 = 0o4000;
pub const IXOFF: u32 = 0o10000;
pub const IMAXBEL: u32 = 0o20000;
pub const IUTF8: u32 = 0o40000;

// c_oflag
pub const OPOST: u32 = 0o1;
pub const OLCUC: u32 = 0o2;
pub const ONLCR: u32 = 0o4;
pub const OCRNL: u32 = 0o10;
pub const ONOCR: u32 = 0o20;
pub const ONLRET: u32 = 0o40;

// c_cflag
pub const CBAUD: u32 = 0o10017;
pub const B38400: u32 = 0o17;
pub const CSIZE: u32 = 0o60;
pub const CS5: u32 = 0o0;
pub const CS6: u32 = 0o20;
pub const CS7: u32 = 0o40;
pub const CS8: u32 = 0o60;
pub const CSTOPB: u32 = 0o100;
pub const CREAD: u32 = 0o200;
pub const PARENB: u32 = 0o400;
pub const PARODD: u32 = 0o1000;
pub const HUPCL: u32 = 0o2000;
pub const CLOCAL: u32 = 0o4000;

// c_lflag
pub const ISIG: u32 = 0o1;
pub const ICANON: u32 = 0o2;
pub const ECHO: u32 = 0o10;
pub const ECHOE: u32 = 0o20;
pub const ECHOK: u32 = 0o40;
pub const ECHONL: u32 = 0o100;
pub const NOFLSH: u32 = 0o200;
pub const TOSTOP: u32 = 0o400;
pub const ECHOCTL: u32 = 0o1000;
pub const ECHOPRT: u32 = 0o2000;
pub const ECHOKE: u32 = 0o4000;
pub const FLUSHO: u32 = 0o10000;
pub const PENDIN: u32 = 0o40000;
pub const IEXTEN: u32 = 0o100000;
pub const EXTPROC: u32 = 0o200000;

// c_cc indexes.
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSWTC: usize = 7;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;
/// The kernel's c_cc has 19 entries.
pub const NCCS: usize = 19;

// ioctl numbers (ioctl_tty(2)).
pub const TCGETS: u32 = 0x5401;
pub const TCSETS: u32 = 0x5402;
pub const TCSETSW: u32 = 0x5403;
pub const TCSETSF: u32 = 0x5404;
/// termios2 (`struct termios2`: the termios, then c_ispeed and c_ospeed;
/// 44 bytes): glibc 2.43's isatty and tcgetattr ask with TCGETS2.
pub const TCGETS2: u32 = 0x802c_542a;
pub const TCSETS2: u32 = 0x402c_542b;
pub const TCSETSW2: u32 = 0x402c_542c;
pub const TCSETSF2: u32 = 0x402c_542d;
/// The input speed's bits in c_cflag (0: the output speed's).
pub const CIBAUD: u32 = CBAUD << 16;

/// The speed in bits per second of baud code `code` (c_cflag's CBAUD
/// bits: B0 .. B38400, then B57600 .. B4000000 with CBAUDEX).
pub fn speed(code: u32) -> u32 {
    const LOW: [u32; 16] =
        [0, 50, 75, 110, 134, 150, 200, 300, 600, 1200, 1800, 2400, 4800, 9600, 19200, 38400];
    const HIGH: [u32; 15] = [
        57600, 115200, 230400, 460800, 500000, 576000, 921600, 1000000, 1152000, 1500000, 2000000,
        2500000, 3000000, 3500000, 4000000,
    ];
    let code = code & CBAUD;
    if code & 0o10000 == 0 {
        LOW[code as usize]
    } else {
        // 0o10000 alone is BOTHER (a speed given in c_ispeed/c_ospeed,
        // which gala's terminals do not keep): 0.
        ((code & 0o17) as usize).checked_sub(1).and_then(|i| HIGH.get(i)).copied().unwrap_or(0)
    }
}

/// The baud code of `bps` bits per second, if it has one.
pub fn code_of(bps: u32) -> Option<u32> {
    (0..16).chain(0o10001..0o10020).find(|&c| speed(c) == bps && (c != 0 || bps == 0))
}

/// TCSETS2's c_cflag: a BOTHER speed (c_ospeed, c_ispeed) that has a baud
/// code becomes that code, as Linux encodes it; None for one without.
pub fn encode_speeds(cflag: u32, ispeed: u32, ospeed: u32) -> Option<u32> {
    const BOTHER: u32 = 0o10000;
    let mut c = cflag;
    if c & CBAUD == BOTHER {
        c = (c & !CBAUD) | code_of(ospeed)?;
    }
    if (c & CIBAUD) >> 16 == BOTHER {
        c = (c & !CIBAUD) | (code_of(ispeed)? << 16);
    }
    Some(c)
}

/// termios2's c_ispeed and c_ospeed for c_cflag `cflag` (TCGETS2).
pub fn speeds(cflag: u32) -> (u32, u32) {
    let out = speed(cflag);
    let input = (cflag & CIBAUD) >> 16;
    (if input == 0 { out } else { speed(input) }, out)
}
pub const TIOCSCTTY: u32 = 0x540e;
pub const TIOCGPGRP: u32 = 0x540f;
pub const TIOCSPGRP: u32 = 0x5410;
pub const TIOCGWINSZ: u32 = 0x5413;
pub const TIOCSWINSZ: u32 = 0x5414;
pub const FIONREAD: u32 = 0x541b;
pub const TIOCNOTTY: u32 = 0x5422;
pub const TIOCGSID: u32 = 0x5429;
pub const TIOCGPTN: u32 = 0x8004_5430;
pub const TIOCSPTLCK: u32 = 0x4004_5431;

/// The kernel's `struct termios`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Termios {
    pub iflag: u32,
    pub oflag: u32,
    pub cflag: u32,
    pub lflag: u32,
    pub line: u8,
    pub cc: [u8; NCCS],
}

/// A control character: ^X is X - 64.
const fn ctrl(c: u8) -> u8 {
    c & 0x1f
}

impl Termios {
    /// What a new pty (both ends) starts with: canonical mode with echo
    /// and signals, CR to NL on input, NL to CR NL on output, 8 bits at
    /// 38400 baud; ^C ^\ DEL ^U ^D, VTIME 0, VMIN 1, ^Q ^S ^Z, ^R ^O ^W ^V.
    pub const fn fresh_pty() -> Termios {
        let mut cc = [0u8; NCCS];
        cc[VINTR] = ctrl(b'C');
        cc[VQUIT] = ctrl(b'\\');
        cc[VERASE] = 0x7f;
        cc[VKILL] = ctrl(b'U');
        cc[VEOF] = ctrl(b'D');
        cc[VTIME] = 0;
        cc[VMIN] = 1;
        cc[VSTART] = ctrl(b'Q');
        cc[VSTOP] = ctrl(b'S');
        cc[VSUSP] = ctrl(b'Z');
        cc[VREPRINT] = ctrl(b'R');
        cc[VDISCARD] = ctrl(b'O');
        cc[VWERASE] = ctrl(b'W');
        cc[VLNEXT] = ctrl(b'V');
        Termios {
            iflag: ICRNL | IXON,
            oflag: OPOST | ONLCR,
            cflag: B38400 | CS8 | CREAD,
            lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
            line: 0,
            cc,
        }
    }

    /// cfmakeraw(3): no input processing, no output processing, 8 bits,
    /// no echo, no canonical mode, no signals; VMIN 1, VTIME 0.
    pub fn make_raw(&mut self) {
        self.iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
        self.oflag &= !OPOST;
        self.lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
        self.cflag &= !(CSIZE | PARENB);
        self.cflag |= CS8;
        self.cc[VMIN] = 1;
        self.cc[VTIME] = 0;
    }

    pub fn canonical(&self) -> bool {
        self.lflag & ICANON != 0
    }
}

/// `struct winsize` (TIOCGWINSZ/TIOCSWINSZ); a new pty's is all zero.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Winsize {
    pub row: u16,
    pub col: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

const _: () = {
    assert!(size_of::<Termios>() == 36);
    assert!(size_of::<Winsize>() == 8);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{text, value};

    const PTY: &str = include_str!("../../../user/lx/pty_test/expected.txt");

    /// "iflag 0x500, oflag 0x5, cflag 0xbf, lflag 0x8a3b, line 0" and
    /// "c_cc 3 28 ..." as pty_test prints a termios.
    fn parse(name: &str) -> Termios {
        let flags = text(PTY, name);
        let mut vals = [0u32; 5];
        for (i, part) in flags.split(", ").enumerate() {
            let v = part.split(' ').nth(1).unwrap();
            vals[i] = crate::transcript::number(v).unwrap() as u32;
        }
        let mut cc = [0u8; NCCS];
        let mut n = 0;
        let ccs = PTY
            .lines()
            .find_map(|l| l.strip_prefix(name).and_then(|r| r.strip_prefix(": c_cc ")))
            .unwrap();
        for (i, v) in ccs.split(' ').enumerate() {
            cc[i] = v.parse().unwrap();
            n += 1;
        }
        assert_eq!(n, NCCS);
        Termios {
            iflag: vals[0],
            oflag: vals[1],
            cflag: vals[2],
            lflag: vals[3],
            line: vals[4] as u8,
            cc,
        }
    }

    #[test]
    fn fresh_pty_matches_linux() {
        assert_eq!(parse("slave"), Termios::fresh_pty());
        assert_eq!(parse("master"), Termios::fresh_pty());
        assert_eq!(value(PTY, "TCGETS writes bytes") as usize, size_of::<Termios>());
    }

    #[test]
    fn speeds_of_the_baud_codes() {
        assert_eq!(speed(B38400), 38400);
        assert_eq!(speed(0), 0);
        assert_eq!(speed(0o10002), 115200);
        assert_eq!(speed(0o10017), 4000000);
        assert_eq!(speed(0o10000), 0);
        assert_eq!(speeds(B38400), (38400, 38400));
        assert_eq!(speeds(B38400 | (0o15 << 16)), (9600, 38400));
        assert_eq!(encode_speeds(0o10000 | CS8, 0, 9600), Some(0o15 | CS8));
        assert_eq!(encode_speeds(B38400, 0, 0), Some(B38400));
        assert_eq!(encode_speeds(0o10000, 0, 12345), None);
        assert!(Termios::fresh_pty().canonical());
    }

    #[test]
    fn make_raw_matches_linux() {
        let mut t = Termios::fresh_pty();
        t.make_raw();
        assert_eq!(parse("raw"), t);
        assert!(!t.canonical());
    }

    #[test]
    fn winsize() {
        assert_eq!(value(PTY, "sizeof(struct winsize)") as usize, size_of::<Winsize>());
        assert_eq!(value(PTY, "  rows"), 0);
        assert_eq!(value(PTY, "  cols"), 0);
        assert_eq!(Winsize::default(), Winsize { row: 0, col: 0, xpixel: 0, ypixel: 0 });
    }
}
