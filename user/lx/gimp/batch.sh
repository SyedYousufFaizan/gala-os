# GIMP in batch mode (ROADMAP LD step 9, scenario lx_gimp_batch): the
# console GIMP of Ubuntu 26.04 (LINUX_ROOTS "gimp", GIMP 3.2) opens in.png,
# blurs and inverts it and saves out.png (batch.scm), and the file's
# SHA-256 is printed: the transcript (user/lx/sh/lx_gimp_batch.txt) is this
# script's run on Ubuntu 26.04 with the same packages, so the hash is real
# Linux's. In gala the busybox applets come from /lx/busybox; on the host
# run it with BB= (the host's coreutils) and DIR=<this directory>.
BB=${BB-/lx/busybox}
DIR=${DIR-/lx/gimp}
cd "$HOME" || exit 1
$BB cp "$DIR/in.png" in.png || exit 1
# GIMP writes its gimprc back at the end: the user's copy, writable (cp
# keeps the source's mode, and gala's /lx files are read-only, 0555).
$BB cp "$DIR/gimprc" gimprc || exit 1
$BB chmod 644 gimprc || exit 1
gimp-console-3.2 -n -i -f -s --gimprc=gimprc \
    --batch-interpreter=plug-in-script-fu-eval \
    -b "(load \"$DIR/batch.scm\")" -b '(gimp-quit 0)'
echo "gimp-console-3.2: exit status $?"
$BB sha256sum out.png
