/* caps_test: capget(2) and capset(2) (ROADMAP LD: Ubuntu 26.04's glycin
 * reads the process's capabilities before it sandboxes an image loader;
 * ENOSYS in gala). The sets themselves depend on who runs the test (root
 * has them all, a user none): it prints relations that hold for both:
 * the effective set within the permitted one, the version handshake
 * (an unknown version: EINVAL and the kernel's preferred version written
 * back), the same sets through our own pid, the sets set back unchanged,
 * and the errors. */
#include "../common/lx.h"
#include "../common/more.h"

#define V1 0x19980330
#define V2 0x20071026
#define V3 0x20080522

struct hdr {
	unsigned version;
	int pid;
};
struct data {
	unsigned effective, permitted, inheritable;
};

int main(void)
{
	lx_init();
	struct hdr h = { V3, 0 };
	struct data d[2], e[2];
	lx_memset(d, 0x55, sizeof d);
	say_ret("capget(v3, pid 0)", sys2(SYS_capget, &h, d));
	say_bool("  effective within permitted",
		 (d[0].effective & ~d[0].permitted) == 0 && (d[1].effective & ~d[1].permitted) == 0);
	say_bool("  the high words hold nothing above the last capability (40)",
		 (d[1].effective | d[1].permitted | d[1].inheritable) >> 9 == 0);
	h.pid = (int)sys0(SYS_getpid);
	lx_memset(e, 0x55, sizeof e);
	say_ret("capget(v3, our pid)", sys2(SYS_capget, &h, e));
	say_bool("  the same sets", lx_memeq(d, e, sizeof d));
	h.pid = (int)sys0(SYS_gettid);
	say_ret("capget(v3, our tid)", sys2(SYS_capget, &h, e));

	say_text("-- versions");
	h = (struct hdr){ 0, 0 };
	say_ret("capget(version 0)", sys2(SYS_capget, &h, d));
	say_hex("  the version written back", h.version);
	h = (struct hdr){ 0x12345678, 0 };
	say_ret("capget(an unknown version, no data)", sys2(SYS_capget, &h, 0));
	say_hex("  the version written back", h.version);
	h = (struct hdr){ V3, 0 };
	say_ret("capget(v3, no data): the version check alone", sys2(SYS_capget, &h, 0));
	struct data one;
	lx_memset(&one, 0x55, sizeof one);
	h = (struct hdr){ V1, 0 };
	say_ret("capget(v1: one structure)", sys2(SYS_capget, &h, &one));
	say_bool("  its sets are v3's low words", one.effective == d[0].effective && one.permitted == d[0].permitted);
	h = (struct hdr){ V2, 0 };
	say_ret("capget(v2)", sys2(SYS_capget, &h, e));

	say_text("-- capset");
	h = (struct hdr){ V3, 0 };
	sys2(SYS_capget, &h, d);
	say_ret("capset(the same sets)", sys2(SYS_capset, &h, d));
	struct data more[2];
	lx_memcpy(more, d, sizeof d);
	more[0].effective = more[0].permitted | 1;
	more[0].permitted &= ~1u;
	say_ret("capset(an effective one not permitted)", sys2(SYS_capset, &h, more));
	h.pid = 1;
	say_ret("capset(another process)", sys2(SYS_capset, &h, d));

	say_text("-- errors");
	h = (struct hdr){ V3, 0x3ffffff0 };
	say_ret("capget(no such pid)", sys2(SYS_capget, &h, d));
	h = (struct hdr){ V3, -1 };
	say_ret("capget(pid -1)", sys2(SYS_capget, &h, d));
	say_ret("capget(bad header)", sys2(SYS_capget, 1, d));
	h = (struct hdr){ V3, 0 };
	say_ret("capget(bad data)", sys2(SYS_capget, &h, 1));
	say_ret("capset(bad data)", sys2(SYS_capset, &h, 1));
	say_text("caps_test: ok");
	return 0;
}
