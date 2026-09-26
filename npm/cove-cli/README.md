# ◆ cove

**Your coding-agent chats, in order.**

Cove is a terminal app that finds every chat you have had with Claude Code, Codex, opencode and other coding agents, lets you **group them into workspaces**, **rename them**, and jump straight back into any of them.

```bash
npm install --global cove-cli
cove        # or the short form: cv
```

- **Workspaces**: one per project; drag a chat onto one, or press `F3`.
- **Your own names**: double-click a chat (or `F2`) to rename it.
- **Instant search** across every agent, with a live preview.
- **A safe trash**: deleted chats and workspaces can be browsed and restored.
- **Nothing is modified**: Cove only reads your agents' history. Workspaces, names and the trash live in `~/.local/state/cove/state.json`.

Press `F1` inside the app for every shortcut.

## Supported platforms

macOS (Apple Silicon and Intel) and Linux (glibc, x64 and arm64). The package installs the native binary for your machine; do not install with `--omit=optional`.

## Credits

Built on [fast-resume](https://github.com/angristan/fast-resume) by Stanislas Lange (MIT). See `NOTICE.md`.
