/* fdioctl_test: the ioctls Linux answers for every descriptor before
 * any device's own (fs/ioctl.c): FIOCLEX and FIONCLEX (close-on-exec,
 * the descriptor's), FIONBIO (O_NONBLOCK, the open file description's,
 * so a dup sees it), and a pipe's FIONREAD (ROADMAP LD: Rust's std and
 * GLib set O_NONBLOCK with FIONBIO; glycin's loader spawn on Ubuntu 26.04
 * panicked on gala's ENOTTY).
 *
 * The results are the same on any machine: the flags as fcntl reads
 * them back, a non-blocking read's EAGAIN, the bytes a pipe holds, and
 * the errors. */
#include "../common/lx.h"
#include "../common/more.h"

#define FIONREAD 0x541B
#define FIONBIO 0x5421
#define FIONCLEX 0x5450
#define FIOCLEX 0x5451
#define F_GETFD 1
#define F_GETFL 3
#define O_NONBLOCK 04000

static void flags(const char *what, int fd)
{
	put(what);
	put(": cloexec ");
	put_dec(sys2(SYS_fcntl, fd, F_GETFD));
	put(", nonblock ");
	put_dec((sys2(SYS_fcntl, fd, F_GETFL) & O_NONBLOCK) != 0);
	end_line();
}

static void each(const char *what, int fd)
{
	int on = 1, off = 0;
	say_text(what);
	flags("  at first", fd);
	say_ret("  FIONBIO 1", sys3(SYS_ioctl, fd, FIONBIO, &on));
	flags("  then", fd);
	say_ret("  FIOCLEX", sys2(SYS_ioctl, fd, FIOCLEX));
	flags("  then", fd);
	int copy = (int)sys1(SYS_dup, fd);
	flags("  a dup (its description, not its descriptor)", copy);
	say_ret("  FIONBIO 0 through the dup", sys3(SYS_ioctl, copy, FIONBIO, &off));
	flags("  the first", fd);
	say_ret("  FIONCLEX", sys2(SYS_ioctl, fd, FIONCLEX));
	flags("  then", fd);
	say_ret("  FIONBIO(bad pointer)", sys3(SYS_ioctl, fd, FIONBIO, 1));
	sys1(SYS_close, copy);
}

int main(void)
{
	lx_init();
	int p[2];
	sys2(SYS_pipe2, p, 0);
	each("-- a pipe's read end", p[0]);
	int on = 1, n = -1;
	sys3(SYS_ioctl, p[0], FIONBIO, &on);
	char c;
	say_ret("a non-blocking read of the empty pipe", sys3(SYS_read, p[0], &c, 1));
	say_ret("FIONREAD of the empty pipe", sys3(SYS_ioctl, p[0], FIONREAD, &n));
	say("  bytes", n);
	sys3(SYS_write, p[1], "hello", 5);
	say_ret("FIONREAD after 5 bytes", sys3(SYS_ioctl, p[0], FIONREAD, &n));
	say("  bytes", n);
	say_ret("FIONREAD(bad pointer)", sys3(SYS_ioctl, p[0], FIONREAD, 1));
	say_ret("an unknown ioctl on a pipe", sys3(SYS_ioctl, p[0], 0x54ff, 0));
	each("-- a pipe's write end", p[1]);

	int s[2];
	sys4(SYS_socketpair, 1 /* AF_UNIX */, 1 /* SOCK_STREAM */, 0, s);
	each("-- a socket", s[0]);
	say_ret("a non-blocking recv of nothing (FIONBIO 1)",
		(sys3(SYS_ioctl, s[0], FIONBIO, &on), sys3(SYS_read, s[0], &c, 1)));

	i64 f = sys3(SYS_open, "fdioctl.tmp", 0102 /* O_RDWR|O_CREAT */, 0600);
	each("-- a regular file", (int)f);
	sys1(SYS_close, f);
	sys1(SYS_unlink, "fdioctl.tmp");

	each("-- an eventfd", (int)sys2(SYS_eventfd2, 0, 0));
	each("-- /dev/null", (int)sys2(SYS_open, "/dev/null", 2));

	say_text("-- errors");
	say_ret("FIONBIO on a closed descriptor", sys3(SYS_ioctl, 99, FIONBIO, &on));
	say_ret("FIOCLEX on -1", sys2(SYS_ioctl, -1, FIOCLEX));
	say_text("fdioctl_test: ok");
	return 0;
}
