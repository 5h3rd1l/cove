# Installation

## With npm (one command)

```bash
npm install --global cove-cli
```

Installs the `cove` and `cv` commands with the native binary for your machine: macOS (Apple Silicon and Intel) and Linux (x64 and arm64, glibc 2.34 or newer).

### `EACCES: permission denied` on Linux

Debian, Ubuntu and their derivatives (Kali, Mint, ...) ship an `npm` that installs global packages into a root-owned folder, so this happens with any global npm package, not just Cove's. Point npm's global installs at your home directory instead, once:

```bash
mkdir -p ~/.npm-global && npm config set prefix ~/.npm-global
echo 'export PATH="$HOME/.npm-global/bin:$PATH"' >> ~/.bashrc && source ~/.bashrc
npm install --global cove-cli
```

(`sudo npm install --global cove-cli` also works, but leaves future global packages root-owned, which causes the same kind of problem again later.)

## From source

Cove is a single Rust binary.

```bash
git clone https://github.com/5h3rd1l/cove.git && cd cove
cargo build --release
ln -sf "$PWD/target/release/cove" ~/.local/bin/cove
ln -sf "$PWD/target/release/cove" ~/.local/bin/cv    # optional short form
```

Requires Rust 1.88 or newer (`rustup toolchain install stable`).

## Where things live

| What | Where |
|---|---|
| Search index (rebuildable) | `~/.cache/cove/` |
| Workspaces, names, trash | `~/.local/state/cove/state.json` |

Set `COVE_STATE_FILE` to keep the state file somewhere else, and `XDG_CACHE_HOME` / `XDG_STATE_HOME` to move the folders.

## Upgrading from `frw`

The first launch copies your workspaces from `~/.local/state/frw/state.json` to the new place. The old file is not modified.

## Supported agents

Claude Code, Codex, opencode, Pi, Cursor, Copilot, Crush, Kimi, Vibe, Grok and Antigravity. Cove reads their session stores read-only.
