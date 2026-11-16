# Inkscape's window (ROADMAP LD, scenario lx_inkscape): Ubuntu 26.04's
# Inkscape 1.4 on the session's Wayland display, opening a copy of in.svg
# in $HOME; the screen check (INKSCAPE_SCREEN) sees the drawing and quits
# it (Ctrl+Q). Its output (GTK's warnings, with times and pids) goes to
# $HOME/inkscape.log; the transcript (user/lx/sh/lx_inkscape.txt) is its
# exit status, recorded on Ubuntu 26.04 with the same keys.
BB=${BB-/lx/busybox}
DIR=${DIR-/lx/inkscape}
cd "$HOME" || exit 1
$BB cp "$DIR/in.svg" in.svg || exit 1
inkscape in.svg >inkscape.log 2>&1
echo "inkscape: exit status $?"
