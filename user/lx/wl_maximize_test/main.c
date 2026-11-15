/* wl_maximize_test: a toplevel that asks to be maximized before its first
 * commit (xdg_toplevel.set_maximized, as Inkscape does) gets a configure
 * with the maximized state and a size (ROADMAP LD: gala dropped the
 * request before the window existed, and Inkscape opened at its natural
 * size). Sizes differ between compositors: only relations are printed.
 * The protocol code is wl_test's. */
#define _GNU_SOURCE
#include "../common/lx.h"
#include "../common/more.h"

#define AF_UNIX_ 1
#define SOCK_STREAM_ 1
#define SOCK_CLOEXEC_ 02000000
#define SOL_SOCKET_ 1
#define SCM_RIGHTS_ 1
#define MSG_CMSG_CLOEXEC_ 0x40000000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MFD_CLOEXEC 1

typedef unsigned int u32;

struct iovec_ { void *base; u64 len; };
struct msghdr_ {
	void *name; u32 namelen; struct iovec_ *iov; u64 iovlen;
	void *control; u64 controllen; int flags;
};
struct cmsg_fd { u64 len; int level; int type; int fd; int pad; };

/* ---- Objects: what each id is, for the events. ---- */

enum kind { NONE, DISPLAY, REGISTRY, CALLBACK, COMPOSITOR, SHM, WM_BASE, POOL, BUFFER,
	    SURFACE, XDG_SURFACE, TOPLEVEL };
static enum kind objects[64];
static u32 next_id = 2;

static u32 new_object(enum kind k)
{
	objects[next_id] = k;
	return next_id++;
}

/* ---- Requests ---- */

static int sock;
static u32 out[1024];
static u32 out_words;

static void begin(u32 object, u32 opcode)
{
	out[0] = object;
	out[1] = opcode;
	out_words = 2;
}

static void word(u32 w) { out[out_words++] = w; }

static void string(const char *s)
{
	u32 n = (u32)lx_strlen(s) + 1;
	word(n);
	lx_memset(&out[out_words], 0, (n + 3) & ~3u);
	lx_memcpy(&out[out_words], s, n);
	out_words += (n + 3) / 4;
}

/* Sends the message built since begin(), with `fd` beside it (-1: none). */
static void send(int fd)
{
	out[1] |= (out_words * 4) << 16;
	struct iovec_ iov = { out, out_words * 4 };
	struct cmsg_fd c = { 20, SOL_SOCKET_, SCM_RIGHTS_, fd, 0 };
	struct msghdr_ m = { 0, 0, &iov, 1, fd >= 0 ? &c : 0, fd >= 0 ? 24 : 0, 0 };
	i64 r = sys3(SYS_sendmsg, sock, &m, 0);
	if (r != (i64)(out_words * 4)) {
		say_ret("sendmsg", r);
		lx_exit(1);
	}
}

/* ---- Events ---- */

static unsigned char in[16384];
static u64 in_len;

/* State the events fill in. */
static u32 globals_name[8], globals_version[8];
/* wl_seat is left out: weston's headless backend has no input devices,
 * so it offers none (gala's server does: user/lx/wl_input_test checks it). */
static const char *const wanted[] = { "wl_compositor", "wl_shm", "xdg_wm_base", "wl_output" };
#define WANTED 4
static int formats_argb, formats_xrgb;
static u32 done_callback;
static u32 configure_serial;
static int toplevel_configured;
static int top_w, top_h, top_max;
static int released;
static u32 deleted[16];
static int deleted_n;
static int error_seen;
static u32 error_object, error_code;

static u32 rd(const unsigned char *p) { u32 v; lx_memcpy(&v, p, 4); return v; }

static void event(u32 object, u32 opcode, const unsigned char *a, u32 len)
{
	enum kind k = object < 64 ? objects[object] : NONE;
	(void)len;
	if (k == DISPLAY && opcode == 0) {
		error_seen = 1;
		error_object = rd(a);
		error_code = rd(a + 4);
	} else if (k == DISPLAY && opcode == 1) {
		if (deleted_n < 16)
			deleted[deleted_n++] = rd(a);
	} else if (k == REGISTRY && opcode == 0) {
		u32 name = rd(a), slen = rd(a + 4);
		const char *iface = (const char *)a + 8;
		u32 version = rd(a + 8 + ((slen + 3) & ~3u));
		for (int i = 0; i < WANTED; i++)
			if (lx_streq(iface, wanted[i])) {
				globals_name[i] = name;
				globals_version[i] = version;
			}
	} else if (k == CALLBACK && opcode == 0) {
		done_callback = object;
	} else if (k == SHM && opcode == 0) {
		u32 f = rd(a);
		if (f == 0)
			formats_argb = 1;
		if (f == 1)
			formats_xrgb = 1;
	} else if (k == WM_BASE && opcode == 0) {
		u32 serial = rd(a);
		begin(object, 3); /* pong */
		word(serial);
		send(-1);
	} else if (k == XDG_SURFACE && opcode == 0) {
		configure_serial = rd(a);
	} else if (k == TOPLEVEL && opcode == 0) {
		toplevel_configured = 1;
		top_w = (int)rd(a);
		top_h = (int)rd(a + 4);
		u32 n = rd(a + 8);
		top_max = 0;
		for (u32 i = 0; i + 4 <= n; i += 4)
			if (rd(a + 12 + i) == 1) /* XDG_TOPLEVEL_STATE_MAXIMIZED */
				top_max = 1;
	} else if (k == BUFFER && opcode == 0) {
		released = 1;
	}
}

/* Reads once (blocking) and handles every whole message; 0 at end of file. */
static i64 dispatch(void)
{
	unsigned char control[256];
	struct iovec_ iov = { in + in_len, sizeof in - in_len };
	struct msghdr_ m = { 0, 0, &iov, 1, control, sizeof control, 0 };
	i64 r = sys3(SYS_recvmsg, sock, &m, MSG_CMSG_CLOEXEC_);
	if (r <= 0)
		return r;
	/* Descriptors the compositor sent (none expected here): closed. */
	for (u64 at = 0; at + 16 <= m.controllen;) {
		struct cmsg_fd *c = (struct cmsg_fd *)(control + at);
		if (c->level == SOL_SOCKET_ && c->type == SCM_RIGHTS_)
			for (u64 i = 0; 16 + 4 * i < c->len; i++)
				sys1(NR_close, ((int *)(control + at + 16))[i]);
		at += (c->len + 7) & ~7ul;
	}
	in_len += (u64)r;
	u64 at = 0;
	while (in_len - at >= 8) {
		u32 object = rd(in + at), second = rd(in + at + 4);
		u32 size = second >> 16;
		if (size < 8 || in_len - at < size)
			break;
		event(object, second & 0xffff, in + at + 8, size - 8);
		at += size;
	}
	lx_memcpy(in, in + at, in_len - at);
	in_len -= at;
	return r;
}

/* wl_display.sync, then events until its callback is done. */
static void roundtrip(void)
{
	u32 cb = new_object(CALLBACK);
	begin(1, 0);
	word(cb);
	send(-1);
	done_callback = 0;
	while (done_callback != cb)
		if (dispatch() <= 0) {
			say_text("roundtrip: the connection ended");
			lx_exit(1);
		}
}

static int connect_display(const char *dir, const char *display)
{
	struct { unsigned short family; char path[108]; } addr = { AF_UNIX_, { 0 } };
	u64 n = lx_strlen(dir);
	lx_memcpy(addr.path, dir, n);
	addr.path[n] = '/';
	lx_memcpy(addr.path + n + 1, display, lx_strlen(display));
	int s = (int)sys3(SYS_socket, AF_UNIX_, SOCK_STREAM_ | SOCK_CLOEXEC_, 0);
	i64 r = sys3(SYS_connect, s, &addr, sizeof addr);
	say_ret("connect", r);
	return s;
}

static const char *env(char **envp, const char *name)
{
	u64 n = lx_strlen(name);
	for (; *envp; envp++)
		if (lx_memeq(*envp, name, n) && (*envp)[n] == '=')
			return *envp + n + 1;
	return 0;
}

int main(int argc, char **argv, char **envp)
{
	(void)argc;
	(void)argv;
	lx_init();
	const char *dir = env(envp, "XDG_RUNTIME_DIR"), *display = env(envp, "WAYLAND_DISPLAY");
	if (!dir || !display) {
		say_text("XDG_RUNTIME_DIR or WAYLAND_DISPLAY is not set");
		lx_exit(1);
	}
	sock = connect_display(dir, display);
	objects[1] = DISPLAY;

	/* The registry and its globals. */
	u32 registry = new_object(REGISTRY);
	begin(1, 1);
	word(registry);
	send(-1);
	roundtrip();
	for (int i = 0; i < WANTED; i++) {
		put("global ");
		put(wanted[i]);
		put(globals_name[i] ? ": yes" : ": no");
		end_line();
	}

	/* Bind wl_compositor 4, wl_shm 1, xdg_wm_base 1 (at most what is
	 * offered). */
	static const u32 want_version[3] = { 4, 1, 1 };
	static const enum kind kinds[3] = { COMPOSITOR, SHM, WM_BASE };
	u32 bound[3];
	for (int i = 0; i < 3; i++) {
		u32 v = globals_version[i] < want_version[i] ? globals_version[i] : want_version[i];
		bound[i] = new_object(kinds[i]);
		begin(registry, 0);
		word(globals_name[i]);
		string(wanted[i]);
		word(v);
		word(bound[i]);
		send(-1);
		put("bind ");
		put(wanted[i]);
		put(" version ");
		put_dec(v);
		end_line();
	}
	roundtrip();
	say_bool("shm argb8888", formats_argb);
	say_bool("shm xrgb8888", formats_xrgb);

	/* A 64x64 XRGB8888 buffer in a memfd pool. */
	int memfd = (int)sys2(SYS_memfd_create, "wl_test", MFD_CLOEXEC);
	say_ret("ftruncate", sys2(SYS_ftruncate, memfd, 64 * 64 * 4));
	u32 *pixels = (u32 *)sys6(NR_mmap, 0, 64 * 64 * 4, PROT_READ | PROT_WRITE, MAP_SHARED,
				   memfd, 0);
	for (int y = 0; y < 64; y++)
		for (int x = 0; x < 64; x++)
			pixels[y * 64 + x] = y < 32 ? 0x00336699 : 0x00ff8800;
	u32 pool = new_object(POOL);
	begin(bound[1], 0); /* wl_shm.create_pool(id, fd, size) */
	word(pool);
	word(64 * 64 * 4);
	send(memfd);
	sys1(NR_close, memfd);
	u32 buffer = new_object(BUFFER);
	begin(pool, 0); /* create_buffer(id, offset, width, height, stride, format) */
	word(buffer);
	word(0);
	word(64);
	word(64);
	word(256);
	word(1); /* XRGB8888 */
	send(-1);
	begin(pool, 1); /* destroy: the buffer keeps the memory */
	send(-1);

	/* A toplevel surface. */
	u32 surface = new_object(SURFACE);
	begin(bound[0], 0); /* create_surface */
	word(surface);
	send(-1);
	u32 xdg_surface = new_object(XDG_SURFACE);
	begin(bound[2], 2); /* get_xdg_surface(id, surface) */
	word(xdg_surface);
	word(surface);
	send(-1);
	u32 toplevel = new_object(TOPLEVEL);
	begin(xdg_surface, 1); /* get_toplevel */
	word(toplevel);
	send(-1);
	begin(toplevel, 2); /* set_title */
	string("wl_maximize_test");
	send(-1);
	begin(toplevel, 9); /* set_maximized, before the first commit */
	send(-1);
	begin(surface, 6); /* commit, without a buffer */
	send(-1);
	while (!configure_serial)
		if (dispatch() <= 0) {
			say_text("configure: the connection ended");
			lx_exit(1);
		}
	say_bool("xdg_surface configure", 1);
	say_bool("toplevel configure before it", toplevel_configured);
	say_bool("  with the maximized state", top_max);
	say_bool("  and a size", top_w > 0 && top_h > 0);
	say_text("wl_maximize_test: ok");
	return 0;
}
