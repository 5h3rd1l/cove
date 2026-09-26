#!/bin/bash
# Records the README demo GIF on the INVENTED demo home. usage: demo-gif.sh out.gif
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd); OUT=${1:-demo.gif}
[ -d /tmp/cove-demo-home ] || python3 "$D/make-demo.py" >/dev/null
W=$D/gif-work; rm -rf "$W"; mkdir -p "$W"; cp "$D/demo-state.json" "$D/demo-run.json"
tmux kill-session -t demo 2>/dev/null || true
tmux new-session -d -s demo -x 150 -y 38 "HOME=/tmp/cove-demo-home XDG_STATE_HOME=/tmp/cove-demo-home/.state XDG_CACHE_HOME=/tmp/cove-demo-home/.cache COVE_STATE_FILE=$D/demo-run.json cove --no-images; sleep 120"
sleep 6
n=0
send(){ tmux send-keys -t demo -l "$1"; sleep 0.45; }
snap(){ n=$((n+1)); f=$(printf %02d $n); tmux capture-pane -t demo -p -e > "$W/f$f.txt"; echo "$1" >> "$W/dur.txt"; }

send $'\e[1;5C'                       # first escape sequence after launch is swallowed
snap 1.8                              # overview
send "/";                snap 0.7     # jump to search
send "web";              snap 0.7
send "hook";             snap 1.6     # one match
send $'\x15'; send $'\e'; snap 0.9    # clear, back to the list
# drag the unsorted "Regex to match ISO 8601 dates" chat onto "Side projects"
send $'\e[<0;41;14M'                  # press on the chat
send $'\e[<32;30;13M';   snap 0.5
send $'\e[<32;18;12M';   snap 0.5
send $'\e[<32;8;11M';    snap 0.9     # hovering the workspace
send $'\e[<0;8;11m';     snap 1.8     # dropped: "moved to Side projects"
# rename it
send $'\eOQ';            snap 0.9     # F2: edit in place
send $'\x15'; send "ISO date regex"; snap 0.9
send $'\r';              snap 1.6
# delete it, look in Deleted, restore it
send $'\e[3~';           snap 1.6     # Delete: moved to Deleted
send $'\e[1;3A';         snap 1.8     # Alt+Up: the Deleted view
send $'\e[19~';          snap 1.6     # F8: restored
send $'\e[1;3B'; send $'\e[1;3B'; snap 2.4     # Deleted -> Hackathon (deleted) -> All chats
tmux kill-session -t demo

# render every frame, then build the GIF
for t in "$W"/f*.txt; do FRAME=1 TITLE=cove python3 "$D/ansi2png.py" "$t" "${t%.txt}.png" "#0f1117" >/dev/null; done
{ i=0; while read -r d; do i=$((i+1)); printf "file '%s/f%02d.png'\nduration %s\n" "$W" "$i" "$d"; done < "$W/dur.txt"; printf "file '%s/f%02d.png'\n" "$W" "$i"; } > "$W/list.txt"
ffmpeg -y -loglevel error -f concat -safe 0 -i "$W/list.txt" \
  -vf "scale=1100:-1:flags=lanczos,split[a][b];[a]palettegen=reserve_transparent=1:stats_mode=diff[p];[b][p]paletteuse=alpha_threshold=128:dither=bayer:bayer_scale=4" -loop 0 "$OUT"
ls -la "$OUT" | awk '{print "gif:", $5/1024/1024 " MB"}'; echo "frames: $n"
