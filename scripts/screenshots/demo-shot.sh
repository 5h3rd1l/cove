#!/bin/bash
# usage: demo-shot.sh out.png [raw-key-sequences...]
# Runs cove on the INVENTED demo home (/tmp/cove-demo-home) and screenshots it.
# env: THEME=light  COLS ROWS  TITLE
D=$(cd "$(dirname "$0")" && pwd); OUT=$1; shift
[ -d /tmp/cove-demo-home ] || python3 "$D/make-demo.py" >/dev/null
cp "$D/demo-state.json" "$D/demo-run.json"
BG="#0f1117"; FLAGS="--no-images"; [ "$THEME" = light ] && { BG="#fafafd"; FLAGS="--no-images --theme light"; }
tmux kill-session -t demo 2>/dev/null
tmux new-session -d -s demo -x ${COLS:-150} -y ${ROWS:-38} "HOME=/tmp/cove-demo-home XDG_STATE_HOME=/tmp/cove-demo-home/.state XDG_CACHE_HOME=/tmp/cove-demo-home/.cache COVE_STATE_FILE=$D/demo-run.json cove $FLAGS; sleep 60"
sleep 6
tmux send-keys -t demo -l $'\e[1;5C'; sleep 0.6     # the first escape sequence after launch is swallowed
for k in "$@"; do tmux send-keys -t demo -l "$k"; sleep 0.6; done
tmux capture-pane -t demo -p -e > "$D/demo-cap.txt"
FRAME=1 TITLE=cove THEME=$THEME python3 "$D/ansi2png.py" "$D/demo-cap.txt" "$OUT" "$BG"
tmux kill-session -t demo
