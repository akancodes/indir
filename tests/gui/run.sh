#!/bin/sh
# indir's window checks under Xvfb with openbox, inside a Debian image with Rust,
# libwebkit2gtk-4.1-dev, xvfb, xdotool, imagemagick and dbus-x11: mount the build's target at /target and this folder at /gui, then `sh /gui/run.sh`.
# Prints one line per check: PASS/FAIL <what> <detail>.
set -u
BIN=/target/debug/indir
SHOTS=/gui/shots
mkdir -p "$SHOTS"
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq openbox wmctrl x11-utils >/dev/null 2>&1

Xvfb :99 -screen 0 1920x1080x24 >/dev/null 2>&1 &
export DISPLAY=:99
sleep 1
eval "$(dbus-launch --sh-syntax)"
openbox >/dev/null 2>&1 &
sleep 1

# Test files: two of 6 MB, served at 150 KB/s per connection.
mkdir -p /files
head -c 6000000 /dev/urandom > /files/one.bin
head -c 6000000 /dev/urandom > /files/two.bin
python3 /gui/serve.py /files 8899 150000 &

STATE=/root/.config/dev.akan.indir
DATA=/root/.local/share/dev.akan.indir
rm -rf "$STATE" "$DATA"

check() { if [ "$2" = "$3" ]; then echo "PASS $1 ($2)"; else echo "FAIL $1: got '$2', want '$3'"; fi; }

# The visible indir windows as "id WxH+X+Y", one per line.
windows() {
  for w in $(xdotool search --onlyvisible --name '^indir$' 2>/dev/null); do
    eval "$(xdotool getwindowgeometry --shell "$w")"
    echo "$w ${WIDTH}x${HEIGHT}+${X}+${Y}"
  done
}
main_win() { windows | grep -v ' 320x90+' | head -1; }
mini_win() { windows | grep ' 320x90+' | head -1; }
wait_for() { i=0; while [ $i -lt 40 ]; do [ -n "$($1)" ] && return 0; sleep 0.25; i=$((i + 1)); done; return 1; }
quit_app() { w=$(main_win | cut -d' ' -f1); [ -n "$w" ] && wmctrl -i -c "$(printf '0x%08x' "$w")"; i=0; while pgrep -x indir >/dev/null && [ $i -lt 40 ]; do sleep 0.25; i=$((i + 1)); done; }
launch() { $BIN >/gui/app.log 2>&1 & wait_for main_win; sleep 1.5; }
key() { xdotool key --clearmodifiers "$@"; sleep 0.3; }

# 1. First launch: 1000x680, centred.
launch
geo=$(main_win | cut -d' ' -f2)
size=${geo%%+*}
check "first launch size" "$size" "1000x680"
x=$(echo "$geo" | cut -d+ -f2); y=$(echo "$geo" | cut -d+ -f3)
centred=$([ $((x - 460)) -ge -20 ] && [ $((x - 460)) -le 20 ] && echo yes || echo no)
check "first launch centred (x=$x y=$y)" "$centred" "yes"
import -window root "$SHOTS/1-first.png"

# 2. Moved and resized, quit, launched: the same place and size.
w=$(main_win | cut -d' ' -f1)
wmctrl -i -r "$(printf '0x%08x' "$w")" -e 0,200,150,1200,700
sleep 1
before=$(main_win | cut -d' ' -f2)
quit_app
check "quits when the main window closes" "$(pgrep -x indir >/dev/null && echo running || echo gone)" "gone"
launch
after=$(main_win | cut -d' ' -f2)
check "place and size remembered" "$after" "$before"

# 3. A saved place off every screen: centred instead.
quit_app
f=$(ls "$STATE"/.window-state* 2>/dev/null | head -1)
if [ -n "$f" ]; then
  python3 - "$f" <<'EOF'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["main"]["x"] = 5000
d["main"]["y"] = 3000
json.dump(d, open(p, "w"))
EOF
  launch
  geo=$(main_win | cut -d' ' -f2)
  x=$(echo "$geo" | cut -d+ -f2)
  check "off-screen place not restored (x=$x)" "$([ "$x" -lt 1920 ] && echo on || echo off)" "on"
else
  echo "FAIL no window-state file in $STATE: $(ls -la "$STATE" 2>&1)"
  launch
fi

# 4. Two downloads, then Ctrl+M: the strip on top, the main window hidden.
w=$(main_win | cut -d' ' -f1)
xdotool windowactivate --sync "$w" 2>/dev/null; sleep 0.3
key ctrl+l
xdotool type --delay 5 'http://127.0.0.1:8899/one.bin http://127.0.0.1:8899/two.bin'
key Return
sleep 2
import -window root "$SHOTS/4-downloading.png"
key ctrl+m
wait_for mini_win
check "strip shown" "$(mini_win | cut -d' ' -f2 | cut -d+ -f1)" "320x90"
check "main hidden" "$(main_win | wc -l)" "0"
above=$(xprop -id "$(mini_win | cut -d' ' -f1)" _NET_WM_STATE 2>/dev/null | grep -c ABOVE)
check "strip stays on top" "$above" "1"
import -window root "$SHOTS/4-strip.png"

# (That the percentage never falls is pinned by tests/ui/logic.test.cjs; here, a later look.)
sleep 6
import -window root "$SHOTS/4-strip-later.png"

# 5. Drag the strip; a click brings the main window back.
m=$(mini_win)
mw=$(echo "$m" | cut -d' ' -f1)
xdotool mousemove --window "$mw" 160 45; sleep 0.2
xdotool mousedown 1; sleep 0.2
# A hand moves in small steps; the window manager takes over once the drag starts.
for d in 2 4 6 8 10 20 40 80 160 240; do xdotool mousemove --window "$mw" $((160 + d)) $((45 + d)); sleep 0.1; done
sleep 0.4
xdotool mouseup 1; sleep 0.6
m2=$(mini_win | cut -d' ' -f2)
check "strip moved by a drag" "$([ "$m2" != "$(echo "$m" | cut -d' ' -f2)" ] && echo moved || echo still)" "moved"
xdotool mousemove --window "$(mini_win | cut -d' ' -f1)" 160 45; sleep 0.2
xdotool click 1; sleep 1
check "click on the strip shows main" "$(main_win | wc -l)" "1"
check "click on the strip hides it" "$(mini_win | wc -l)" "0"

# 6. When done → Quit, strip shown: the main window comes back with the countdown.
# Re-add two downloads (the first pair may have finished), choose quit in the menu by keyboard.
w=$(main_win | cut -d' ' -f1)
xdotool windowactivate --sync "$w" 2>/dev/null; sleep 0.3
key ctrl+l
xdotool type --delay 5 'http://127.0.0.1:8899/one.bin'
key Return
sleep 1
geo=$(main_win | cut -d' ' -f2); ww=${geo%%x*}; wh=$(echo "${geo%%+*}" | cut -dx -f2)
# The footer's menu button, from the window's own corner (xdotool's Y counts the title bar).
xdotool mousemove --window "$w" $((ww - 28)) $((wh - 24)); sleep 0.2; xdotool click 1; sleep 0.5
key Down; key Return; sleep 0.3
key Down; key Down; key Down; key Down; key Return
sleep 0.5
import -window root "$SHOTS/6-armed.png"
key ctrl+m
wait_for mini_win
# Wait for the download to end and the countdown to start: the main window shows itself.
i=0; while [ -z "$(main_win)" ] && [ $i -lt 120 ]; do sleep 0.5; i=$((i + 1)); done
check "countdown brings main back" "$(main_win | wc -l)" "1"
check "countdown hides the strip" "$(mini_win | wc -l)" "0"
sleep 1
import -window root "$SHOTS/6-countdown.png"
# The mini shortcut must not hide the countdown.
key ctrl+m; sleep 1
check "mini shortcut keeps the countdown in view" "$(main_win | wc -l)" "1"
w=$(main_win | cut -d' ' -f1); [ -n "$w" ] || { xdotool mousemove --window "$(mini_win | cut -d' ' -f1)" 160 45; xdotool click 1; sleep 1; w=$(main_win | cut -d' ' -f1); }
xdotool windowactivate --sync "$w" 2>/dev/null; sleep 0.3
key Escape
sleep 2
check "Escape cancels the countdown (app still running)" "$(pgrep -x indir >/dev/null && echo running || echo gone)" "running"

# 7. The strip keeps its place: shown again, and after a relaunch.
w=$(main_win | cut -d' ' -f1); xdotool windowactivate --sync "$w" 2>/dev/null
key ctrl+m; wait_for mini_win
mw=$(mini_win | cut -d' ' -f1)
xdotool mousemove --window "$mw" 160 45; sleep 0.2; xdotool mousedown 1; sleep 0.2
for d in 2 4 6 8 10 20 40 80 160 300; do xdotool mousemove --window "$mw" $((160 + d)) $((45 + d)); sleep 0.1; done
sleep 0.4; xdotool mouseup 1; sleep 0.8
before=$(mini_win | cut -d' ' -f2)
xdotool mousemove --window "$mw" 160 45; sleep 0.2; xdotool click 1; sleep 1
w=$(main_win | cut -d' ' -f1); xdotool windowactivate --sync "$w" 2>/dev/null
key ctrl+m; wait_for mini_win
check "strip shown again where it was dragged" "$(mini_win | cut -d' ' -f2)" "$before"
key ctrl+m; sleep 0.5
quit_app; launch
w=$(main_win | cut -d' ' -f1); xdotool windowactivate --sync "$w" 2>/dev/null
key ctrl+m; wait_for mini_win
check "strip place remembered after a relaunch" "$(mini_win | cut -d' ' -f2)" "$before"
# Back to the main window, which is the one that quits: a click, since a window manager may
# not hand the strip the keyboard.
xdotool mousemove --window "$(mini_win | cut -d' ' -f1)" 160 45; sleep 0.2; xdotool click 1; wait_for main_win
quit_app
# 8. A file from start to end, paused on the way: its bytes equal the source's.
rm -rf "$DATA" /root/Downloads /root/three.bin
head -c 9000000 /dev/urandom > /files/three.bin
launch
w=$(main_win | cut -d' ' -f1); xdotool windowactivate --sync "$w" 2>/dev/null
key ctrl+l
xdotool type --delay 5 'http://127.0.0.1:8899/three.bin'
key Return
sleep 2
# The card's buttons show on hover: pause is the one left of remove, at the card's right edge.
# Two columns between 880 and 1300 px, one below: the first card's right edge follows from the width.
ww=$(main_win | cut -d' ' -f2); ww=${ww%%x*}
if [ "$ww" -ge 880 ]; then card_right=$((16 + (ww - 32 - 10) / 2)); else card_right=$((ww - 16)); fi
pause_x=$((card_right - 15 - 34 - 6 - 17))
xdotool mousemove --window "$w" 200 111; sleep 0.4
xdotool mousemove --window "$w" "$pause_x" 111; sleep 0.3
xdotool click 1; sleep 1.5
status=$(python3 -c "import json;print(json.load(open('$DATA/downloads.json'))[0]['status'])" 2>/dev/null)
check "pause from the card" "$status" "paused"
import -window root "$SHOTS/8-paused.png"
# The same place is now Resume.
xdotool click 1; sleep 0.5
# Downloads go to the system's Downloads folder, or home when there is none.
got=""; i=0
while [ -z "$got" ] && [ $i -lt 120 ]; do
  got=$(find /root -maxdepth 2 -name three.bin 2>/dev/null | head -1); sleep 0.5; i=$((i + 1))
done
sleep 1
check "file arrives whole ($got)" "$(sha256sum < "$got" 2>/dev/null)" "$(sha256sum < /files/three.bin)"
quit_app

echo "--- app.log"; tail -5 /gui/app.log
