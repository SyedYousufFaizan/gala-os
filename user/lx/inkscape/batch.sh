# Inkscape in batch mode (ROADMAP LD, scenario lx_inkscape_batch): Ubuntu
# 26.04's Inkscape 1.4 (LINUX_ROOTS "inkscape") renders in.svg to a PNG,
# and the file's SHA-256 is printed: the transcript
# (user/lx/sh/lx_inkscape_batch.txt) is this script's run on Ubuntu 26.04
# with the same packages. Inkscape's stderr (GTK's warnings, with a pid and
# a time) goes to $HOME/inkscape.log. In gala the busybox applets come
# from /lx/busybox; on the host run it with BB= and DIR=<this directory>.
BB=${BB-/lx/busybox}
DIR=${DIR-/lx/inkscape}
cd "$HOME" || exit 1
inkscape --export-type=png --export-filename=out.png "$DIR/in.svg" 2>inkscape.log
echo "inkscape: exit status $?"
$BB sha256sum out.png
