# Screenshots

The README screenshots are generated from **invented** demo data, never from real chats.

```bash
python3 scripts/screenshots/make-demo.py                    # fake home in /tmp/cove-demo-home
scripts/screenshots/demo-shot.sh out.png                    # overview
scripts/screenshots/demo-shot.sh out.png $'\e[1;3B'         # keys are raw escape sequences
THEME=light scripts/screenshots/demo-shot.sh light.png      # light theme
```

Needs `tmux`, `chromium`, `imagemagick` and a built `cove` on the PATH. The demo runs with `HOME` pointed at the fake folder, so nothing from your own machine can appear.

The animated demo (`assets/demo.gif`) is recorded the same way: `scripts/screenshots/demo-gif.sh out.gif` (also needs `ffmpeg`).
