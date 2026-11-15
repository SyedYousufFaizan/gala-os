/* altstack_test: sigaltstack(2) and handlers run on the alternate
 * signal stack (SA_ONSTACK) (ROADMAP LD: Rust's runtime sets one up in
 * every thread for its stack-overflow handler: Ubuntu 26.04's glycin
 * loaders and uutils; ENOSYS in gala).
 *
 * Only relations are printed: a handler's stack inside the alternate
 * stack or not, the flags read back. */
#include "../common/lx.h"
#include "../common/more.h"

#define SS_ONSTACK 1
#define SS_DISABLE 2
#define SA_ONSTACK 0x08000000
#define SIGUSR1 10
#define SIGUSR2 12

struct stack {
	u64 sp;
	int flags;
	int pad;
	u64 size;
};

static char alt[65536] __attribute__((aligned(16)));
static volatile int on_alt, flags_inside, change_inside;

static void handler(int sig)
{
	(void)sig;
	char here;
	on_alt = (u64)&here >= (u64)alt && (u64)&here < (u64)alt + sizeof alt;
	struct stack cur;
	sys2(SYS_sigaltstack, 0, &cur);
	flags_inside = cur.flags;
	struct stack other = { (u64)alt, 0, 0, sizeof alt };
	change_inside = (int)sys2(SYS_sigaltstack, &other, 0);
}

static void say_stack(const char *label, struct stack *s)
{
	put(label);
	put(": flags ");
	put_dec(s->flags);
	put(", ours ");
	put(s->sp == (u64)alt && s->size == sizeof alt ? "yes" : "no");
	put(", none ");
	put(s->sp == 0 && s->size == 0 ? "yes" : "no");
	end_line();
}

int main(void)
{
	lx_init();
	struct stack old, s;
	lx_memset(&old, 0x55, sizeof old);
	say_ret("sigaltstack(NULL, &old)", sys2(SYS_sigaltstack, 0, &old));
	say_stack("  at start", &old);
	s = (struct stack){ (u64)alt, 0, 0, sizeof alt };
	say_ret("sigaltstack(64 KiB)", sys2(SYS_sigaltstack, &s, 0));
	sys2(SYS_sigaltstack, 0, &old);
	say_stack("  then", &old);

	set_action(SIGUSR1, handler, SA_ONSTACK, 0);
	set_action(SIGUSR2, handler, 0, 0);
	sys2(SYS_kill, sys0(SYS_getpid), SIGUSR1);
	say_bool("a SA_ONSTACK handler: on the alternate stack", on_alt);
	say("  sigaltstack's flags in it", flags_inside);
	say_ret("  changing it in it", change_inside);
	sys2(SYS_kill, sys0(SYS_getpid), SIGUSR2);
	say_bool("a handler without SA_ONSTACK: on it", on_alt);
	say("  the flags in it", flags_inside);

	i64 child = lx_fork();
	if (child == 0) {
		struct stack c;
		sys2(SYS_sigaltstack, 0, &c);
		lx_exit(c.sp == (u64)alt && c.size == sizeof alt && c.flags == 0 ? 7 : 1);
	}
	say_status("a fork's child keeps it (exit 7)", wait_for(child, 0));

	s = (struct stack){ 0, SS_DISABLE, 0, 0 };
	say_ret("SS_DISABLE", sys2(SYS_sigaltstack, &s, &old));
	say_stack("  the old one", &old);
	sys2(SYS_sigaltstack, 0, &old);
	say("  now its flags", old.flags);
	sys2(SYS_kill, sys0(SYS_getpid), SIGUSR1);
	say_bool("a SA_ONSTACK handler with none: on it", on_alt);

	say_text("-- errors");
	s = (struct stack){ (u64)alt, 0, 0, 1000 };
	say_ret("a size below MINSIGSTKSZ", sys2(SYS_sigaltstack, &s, 0));
	s = (struct stack){ (u64)alt, 4, 0, sizeof alt };
	say_ret("flags 4", sys2(SYS_sigaltstack, &s, 0));
	say_ret("a bad pointer", sys2(SYS_sigaltstack, 1, 0));
	say_ret("a bad old pointer", sys2(SYS_sigaltstack, 0, 1));
	say_text("altstack_test: ok");
	return 0;
}
