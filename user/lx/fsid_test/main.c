/* fsid_test: setfsuid(2) and setfsgid(2) (ROADMAP LD: Ubuntu 26.04's
 * xterm calls both; ENOSYS in gala). Each returns the previous
 * filesystem id and sets it only to an id the caller has (or any, with
 * CAP_SETUID/CAP_SETGID): only relations to geteuid() and getegid() are
 * printed, the same for root and for a user. */
#include "../common/lx.h"
#include "../common/more.h"

int main(void)
{
	lx_init();
	i64 euid = sys0(SYS_geteuid), egid = sys0(SYS_getegid);
	say_bool("setfsuid(-1) is geteuid() (a query)", sys1(SYS_setfsuid, -1) == euid);
	say_bool("setfsuid(geteuid()) is geteuid()", sys1(SYS_setfsuid, euid) == euid);
	say_bool("  then setfsuid(-1) is geteuid()", sys1(SYS_setfsuid, -1) == euid);
	say_bool("setfsgid(-1) is getegid() (a query)", sys1(SYS_setfsgid, -1) == egid);
	say_bool("setfsgid(getegid()) is getegid()", sys1(SYS_setfsgid, egid) == egid);
	say_bool("  then setfsgid(-1) is getegid()", sys1(SYS_setfsgid, -1) == egid);
	say_text("fsid_test: ok");
	return 0;
}
