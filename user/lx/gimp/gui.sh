# GIMP's window (ROADMAP LD step 9, scenario lx_gimp): Ubuntu 26.04's GIMP
# 3.2 on the session's Wayland display, opening in.png; the screen check
# closes its welcome dialog and quits it (Ctrl+Q). Its output (GTK's and
# GIMP's warnings, with times and pids) goes to $HOME/gimp.log; the
# transcript (user/lx/sh/lx_gimp.txt) is its exit status.
DIR=${DIR-/lx/gimp}
cd "$HOME" || exit 1
gimp-3.2 -n -s -f "$DIR/in.png" >gimp.log 2>&1
echo "gimp-3.2: exit status $?"
