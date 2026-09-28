# Usage

```bash
cove                      # open the app
cove "query"              # open with a search typed
cove --agent claude       # only Claude Code chats
cove --directory myproj   # only chats from folders matching "myproj"
cove --list               # print a table instead of opening the app
cove --json --limit 10    # machine-readable output (stable JSON)
cove --yolo               # resume with auto-approve flags where supported
cove --theme dark|light   # force a theme (also COVE_THEME)
```

## Search

Type to filter titles, messages and folders. Extra filters:

- `agent:claude`, `agent:!codex`
- `dir:project`
- `date:today`, `date:yesterday`, `date:week`, `date:month`

`Tab` cycles the agent filter. You can also click an agent tab.

## Workspaces

The list on the left has **All chats**, your **workspaces**, **Unsorted** (chats in no workspace) and **Deleted**.

- Drag a chat onto a workspace, or press `F3` and pick one.
- `F4` or the **+ New workspace** button creates one; type its name and press `Enter`.
- Double-click any name, or press `F2`, to rename it. Renames are only shown in Cove; the chat keeps its real title in the agent.
- `←` from the chat list focuses the workspace list, `→` or `Enter` goes back.

## Deleting and restoring

- `F8`, `Delete` or the ✕ on a row moves a chat to **Deleted**. Deleting a workspace moves it there with its chats.
- Open **Deleted** to browse. A deleted workspace can be opened without restoring it.
- In **Deleted**: `F8` or ↺ restores; `Delete`, `F9` or the red ✕ erases for good after a confirmation.
- Erasing for good hides the chat from Cove forever. **Cove never modifies your agents' own files.**

## Scratch chats

`Ctrl+I` (or `Alt+I`, see below) opens a brand-new session — not a resume — with an agent, for a quick aside you don't want cluttering the list. Cove leaves its own screen entirely while the agent runs, exactly as it does for `Enter`, and comes back the moment it exits.

It picks the agent from the selected chat, or the agent with the most indexed history if nothing is selected, or Claude Code as a last resort. When the agent CLI exits, Cove rescans and hides (does not delete) whatever new session it just wrote, the same as pressing `F9` on it. The chat's own file is untouched on disk; it just never shows up in Cove.

`Ctrl+I` needs a terminal that reports the Kitty keyboard protocol (kitty, WezTerm, foot, recent Alacritty, ghostty, …) to tell it apart from a bare Tab; Cove enables that automatically when the terminal supports it. `Alt+I` works everywhere, with no such requirement.

## Everything else

`F1` shows every shortcut. `Ctrl+P` toggles the preview, `Ctrl+O` the workspace list, `Ctrl+Y` copies the resume command, `Esc` goes back and then quits.
